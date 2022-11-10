/// TODO: Upstream most of these to ethers-rs
mod estimator;
mod gas_oracle_logger;
mod min_gas_fees;
mod rpc_logger;
mod transport;

use self::{
    estimator::Estimator, gas_oracle_logger::GasOracleLogger, min_gas_fees::MinGasFees,
    rpc_logger::RpcLogger, transport::Transport,
};
use chrono::{Duration as ChronoDuration, Utc};
use clap::Parser;
use ethers::{
    abi::{Error as AbiError, RawLog},
    contract::EthEvent,
    core::k256::ecdsa::SigningKey,
    middleware::{
        gas_oracle::{
            Cache, EthGasStation, Etherchain, GasNow, GasOracle, GasOracleMiddleware, Median,
            Polygon, ProviderOracle,
        },
        SignerMiddleware,
    },
    providers::{LogQueryError, Middleware, Provider, ProviderError},
    signers::{LocalWallet, Signer, Wallet},
    types::{
        transaction::eip2718::TypedTransaction, u256_from_f64_saturating, Address, BlockId,
        BlockNumber, Chain, Filter, Log, TransactionReceipt, H160, H256, U64,
    },
};
use eyre::{eyre, Result as EyreResult};
use futures::{try_join, FutureExt, Stream, StreamExt, TryStreamExt};
use once_cell::sync::Lazy;
use prometheus::{
    exponential_buckets, register_counter, register_gauge, register_histogram,
    register_int_counter_vec, Counter, Gauge, Histogram, IntCounterVec,
};
use reqwest::Client as ReqwestClient;
use std::{error::Error, sync::Arc, time::Duration};
use thiserror::Error;
use tokio::time::timeout;
use tracing::{debug_span, error, info, info_span, instrument, warn, Instrument};
use url::Url;

const PENDING: Option<BlockId> = Some(BlockId::Number(BlockNumber::Pending));

static TX_COUNT: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!("eth_tx_count", "The transaction count by bytes4.", &[
        "bytes4"
    ])
    .unwrap()
});
static TX_LATENCY: Lazy<Histogram> = Lazy::new(|| {
    register_histogram!(
        "eth_tx_latency_seconds",
        "The transaction inclusion latency in seconds.",
        exponential_buckets(0.1, 1.5, 25).unwrap()
    )
    .unwrap()
});
static TX_GAS_FRACTION: Lazy<Histogram> = Lazy::new(|| {
    register_histogram!(
        "eth_tx_gas_fraction",
        "The fraction of the gas_limit used by the transaction.",
        vec![
            0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.75, 0.8, 0.85, 0.9, 0.95, 0.975, 0.99, 0.999, 1.0
        ]
    )
    .unwrap()
});
static TX_GAS_PRICE: Lazy<Gauge> = Lazy::new(|| {
    register_gauge!(
        "eth_tx_gas_price",
        "Effective gas price for mined transaction."
    )
    .unwrap()
});
static TX_GAS_USED: Lazy<Counter> = Lazy::new(|| {
    register_counter!("eth_tx_gas_used", "Cumulative gas used for transactions.").unwrap()
});
static TX_WEI_USED: Lazy<Counter> = Lazy::new(|| {
    register_counter!("eth_tx_wei_used", "Cumulative wei used for transactions.").unwrap()
});

// TODO: Log and metrics for signer / nonces.

#[derive(Clone, Debug, PartialEq, Parser)]
#[group(skip)]
pub struct Options {
    /// Ethereum API Provider
    #[clap(long, env, default_value = "http://localhost:8545")]
    pub ethereum_provider: Url,

    /// Private key used for transaction signing
    #[clap(
        long,
        env,
        default_value = "ee79b5f6e221356af78cf4c36f4f7885a11b67dfcc81c34d80249947330c0f82"
    )]
    // NOTE: We abuse `Hash` here because it has the right `FromStr` implementation.
    pub signing_key: H256,

    /// Maximum number of blocks to pull events from in one request.
    #[clap(long, env, default_value = "1000")]
    pub max_log_blocks: usize,

    /// Minimum `max_fee_per_gas` to use in GWei. The default is for Polygon
    /// mainnet.
    #[clap(long, env, default_value = "1250.0")]
    pub min_max_fee: f64,

    /// Minimum `priority_fee_per_gas` to use in GWei. The default is for
    /// Polygon mainnet.
    #[clap(long, env, default_value = "31.0")]
    pub min_priority_fee: f64,

    /// Multiplier on `priority_fee_per_gas`.
    #[clap(long, env, default_value = "100")]
    pub priority_fee_multiplier_percentage: u64,

    /// Timeout for sending transactions to mempool (seconds).
    #[clap(long, env, default_value = "30")]
    pub send_timeout: u64,

    /// Timeout for mining transaction (seconds).
    #[clap(long, env, default_value = "300")]
    pub mine_timeout: u64,
}

// Code out the provider stack in types
// Needed because of <https://github.com/gakonst/ethers-rs/issues/592>
type Provider0 = Provider<RpcLogger<Transport>>;
type Provider1 = Estimator<Provider0>;
type Provider2 = GasOracleMiddleware<Arc<Provider1>, Box<dyn GasOracle>>;
type Provider3 = SignerMiddleware<Provider2, Wallet<SigningKey>>;
// type Provider4 = NonceManagerMiddleware<Provider3>;
pub type ProviderStack = Provider3;

#[derive(Debug, Error)]
pub enum TxError {
    #[error("Error filling transaction: {0}")]
    Fill(Box<dyn Error + Send + Sync + 'static>),

    #[error("Timeout while sending transaction")]
    SendTimeout,

    #[error("Error sending transaction: {0}")]
    Send(Box<dyn Error + Send + Sync + 'static>),

    #[error("Timeout while waiting for confirmations")]
    ConfirmationTimeout,

    #[error("Error waiting for confirmations: {0}")]
    Confirmation(ProviderError),

    #[error("Transaction dropped from mempool.")]
    Dropped(H256),

    #[error("Transaction failed.")]
    Failed(Box<TransactionReceipt>),
}

#[derive(Debug, Error)]
pub enum EventError {
    #[error("Error fetching log event: {0}")]
    Fetching(#[from] LogQueryError<ProviderError>),

    #[error("Error parsing log event: {0}")]
    Parsing(#[from] AbiError),
}

#[derive(Clone, Debug)]
pub struct Ethereum {
    provider:       Arc<ProviderStack>,
    address:        H160,
    legacy:         bool,
    max_log_blocks: usize,
    send_timeout:   Duration,
    mine_timeout:   Duration,
}

impl Ethereum {
    #[instrument(name = "Ethereum::new", level = "debug", skip_all)]
    pub async fn new(options: Options) -> EyreResult<Self> {
        // Connect to the Ethereum provider
        // TODO: Allow multiple providers with failover / broadcast.
        // TODO: Requests don't seem to process in parallel. Check if this is
        // a limitation client side or server side.
        // TODO: Does the WebSocket impl handle dropped connections by
        // reconnecting? What is the timeout on stalled connections? What is
        // the retry policy?
        let (provider, chain_id, eip1559) = {
            info!(
                provider = %&options.ethereum_provider,
                "Connecting to Ethereum"
            );
            let transport = Transport::new(options.ethereum_provider).await?;
            let logger = RpcLogger::new(transport);
            let provider = Provider::new(logger);

            // Fetch state of the chain.
            let (version, chain_id, latest_block, eip1559) = try_join!(
                provider.client_version(),
                provider.get_chainid(),
                provider.get_block(BlockId::Number(BlockNumber::Latest)),
                provider
                    .fee_history(1, BlockNumber::Latest, &[])
                    .map(|r| Ok(r.is_ok()))
            )?;

            // Identify chain.
            let chain = Chain::try_from(chain_id)
                .map_or_else(|_| "Unknown".to_string(), |chain| chain.to_string());

            // Log chain state.
            let latest_block = latest_block
                .ok_or_else(|| eyre!("Failed to get latest block from Ethereum provider"))?;
            let block_hash = latest_block
                .hash
                .ok_or_else(|| eyre!("Could not read latest block hash"))?;
            let block_number = latest_block
                .number
                .ok_or_else(|| eyre!("Could not read latest block number"))?;
            let block_time = latest_block.time()?;
            info!(%version, %chain_id, %chain, %eip1559, %block_number, ?block_hash, %block_time, "Connected to Ethereum provider");

            // Sanity check the block timestamp
            let now = Utc::now();
            let block_age = now - block_time;
            let block_age_abs = if block_age < ChronoDuration::zero() {
                -block_age
            } else {
                block_age
            };
            if block_age_abs > ChronoDuration::minutes(30) {
                // Log an error, but proceed anyway since this doesn't technically block us.
                error!(%now, %block_time, %block_age, "Block time is more than 30 minutes from now.");
            }
            (provider, chain_id, eip1559)
        };

        // Add a gas estimator with 10% and 10k gas bonus over provider.
        // TODO: Use local EVM evaluation?
        let provider = Estimator::new(provider, 1.10, 10e3);

        // Add a gas oracle.
        let provider = {
            // Start with a medianizer
            let mut median = Median::new();

            // Construct a fallback oracle
            let provider = Arc::new(provider);
            median.add_weighted(0.1, ProviderOracle::new(provider.clone()));

            // Utility to get a Reqwest client with 30s timeout.
            let client = || -> EyreResult<ReqwestClient> {
                ReqwestClient::builder()
                    .timeout(Duration::from_secs(30))
                    .build()
                    .map_err(Into::into)
            };

            // Add more oracles to the median based on the chain we are on.
            if let Ok(chain) = Chain::try_from(chain_id) {
                match chain {
                    Chain::Mainnet => {
                        let client = client()?;
                        median.add(EthGasStation::with_client(client.clone(), None));
                        median.add(Etherchain::with_client(client.clone()));
                        median.add(GasNow::with_client(client));
                    }
                    Chain::Polygon | Chain::PolygonMumbai => {
                        median.add(Polygon::with_client(client()?, chain)?);
                    }
                    _ => {}
                }
            }

            // Add minimum gas fees.
            let min_max_fee = u256_from_f64_saturating(options.min_max_fee * 1e9);
            let min_priority_fee = u256_from_f64_saturating(options.min_priority_fee * 1e9);
            let oracle = MinGasFees::new(
                median,
                min_max_fee,
                min_priority_fee,
                options.priority_fee_multiplier_percentage.into(),
            );

            // Add a logging, caching and abstract the type.
            let oracle = GasOracleLogger::new(oracle);
            let oracle = Cache::new(Duration::from_secs(5), oracle);
            let oracle: Box<dyn GasOracle> = Box::new(oracle);

            // Sanity check. fetch current prices.
            let legacy_fee = oracle.fetch().await?;
            if eip1559 {
                let (max_fee, priority_fee) = oracle.estimate_eip1559_fees().await?;
                info!(%legacy_fee, %max_fee, %priority_fee, "Fetched gas prices");
            } else {
                info!(%legacy_fee, "Fetched gas price (no eip1559)");
            };

            // Wrap in a middleware
            GasOracleMiddleware::new(provider, oracle)
        };

        // Construct a local key signer
        let (provider, address) = {
            // Create signer
            let signing_key = SigningKey::from_bytes(options.signing_key.as_bytes())?;
            let signer = LocalWallet::from(signing_key);
            let address = signer.address();

            // Create signer middleware for provider.
            let chain_id: u64 = chain_id.try_into().map_err(|e| eyre!("{}", e))?;
            let signer = signer.with_chain_id(chain_id);
            let provider = SignerMiddleware::new(provider, signer);

            // Create local nonce manager.
            // TODO: This is state full. There may be unsettled TXs in the mempool.
            // let provider = { NonceManagerMiddleware::new(provider, address) };
            //
            // // Log wallet info.
            // let (next_nonce, balance) = try_join!(
            //     provider.initialize_nonce(PENDING),
            //     provider.get_balance(address, PENDING)
            // )?;
            // info!(?address, %next_nonce, %balance, "Constructed wallet");

            // Log wallet info.
            let (next_nonce, balance) = try_join!(
                provider.get_transaction_count(address, PENDING),
                provider.get_balance(address, PENDING)
            )?;
            info!(?address, %next_nonce, %balance, "Constructed wallet");

            // Sanity check the balance
            if balance.is_zero() {
                // Log an error, but try proceeding anyway.
                error!(?address, "Wallet has no funds.");
            }
            (provider, address)
        };
        // TODO: Check signer balance regularly and keep the metric as a gauge.

        let provider = Arc::new(provider);
        Ok(Self {
            provider,
            address,
            legacy: !eip1559,
            max_log_blocks: options.max_log_blocks,
            send_timeout: Duration::from_secs(options.send_timeout),
            mine_timeout: Duration::from_secs(options.mine_timeout),
        })
    }

    #[must_use]
    pub const fn provider(&self) -> &Arc<ProviderStack> {
        &self.provider
    }

    #[must_use]
    pub const fn address(&self) -> Address {
        self.address
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn send_transaction(&self, tx: TypedTransaction) -> Result<TransactionReceipt, TxError> {
        self.send_transaction_unlogged(tx).await.map_err(|e| {
            error!(?e, "Transaction failed");
            e
        })
    }

    #[instrument(level = "info", skip(self))]
    #[allow(clippy::option_if_let_else)] // Less readable
    #[allow(clippy::cast_precision_loss)]
    async fn send_transaction_unlogged(
        &self,
        tx: TypedTransaction,
    ) -> Result<TransactionReceipt, TxError> {
        // Convert to legacy transaction if required
        let mut tx = if self.legacy {
            TypedTransaction::Legacy(match tx {
                TypedTransaction::Legacy(tx) => tx,
                TypedTransaction::Eip1559(tx) => tx.into(),
                TypedTransaction::Eip2930(tx) => tx.tx,
            })
        } else {
            tx
        };

        // Fill in transaction
        self.provider
            .fill_transaction(&mut tx, None)
            .instrument(debug_span!("Fill in transaction"))
            .await
            .map_err(|error| {
                error!(?error, "Failed to fill transaction");
                TxError::Fill(Box::new(error))
            })?;
        let nonce = tx.nonce().unwrap().as_u64();
        let gas_limit = tx.gas().unwrap().as_u128() as f64;
        let gas_price = tx.gas_price().unwrap().as_u128() as f64;

        // Log transaction
        info!(?tx, ?nonce, ?gas_limit, ?gas_price, "Sending transaction.");
        let bytes4: u32 = tx.data().map_or(0, |data| {
            let mut buffer = [0; 4];
            buffer.copy_from_slice(&data.as_ref()[..4]); // TODO: Don't panic.
            u32::from_be_bytes(buffer)
        });
        let bytes4 = format!("{bytes4:8x}");
        TX_COUNT.with_label_values(&[&bytes4]).inc();

        // Send TX to mempool
        let pending = timeout(
            self.send_timeout,
            self.provider.send_transaction(tx.clone(), None),
        )
        .instrument(info_span!("Send TX to mempool"))
        .await
        .map_err(|elapsed| {
            error!(?elapsed, "Send transaction timed out");
            TxError::SendTimeout
        })?
        .map_err(|error| {
            error!(?nonce, ?error, "Failed to send transaction");
            TxError::Send(Box::new(error))
        })?;
        let tx_hash: H256 = *pending;
        info!(?nonce, ?tx_hash, "Transaction in mempool");

        // Wait for TX to be mined
        let timer = TX_LATENCY.start_timer();
        let receipt = timeout(self.mine_timeout, pending)
            .instrument(info_span!("Wait for TX to be mined"))
            .await
            .map_err(|elapsed| {
                error!(?elapsed, "Waiting for transaction confirmation timed out");
                TxError::ConfirmationTimeout
            })?
            .map_err(|err| {
                error!(?nonce, ?tx_hash, ?err, "Transaction failed to confirm");

                TxError::Confirmation(err)
            })?
            .ok_or_else(|| {
                error!(?nonce, ?tx_hash, "Transaction dropped");
                TxError::Dropped(tx_hash)
            })?;
        timer.observe_duration();
        info!(?nonce, ?tx_hash, ?receipt, "Transaction mined");

        // Check receipt for gas used
        if let Some(gas_price) = receipt.effective_gas_price {
            TX_GAS_PRICE.set(gas_price.as_u128() as f64);
        } else {
            error!(
                ?nonce,
                ?tx,
                ?receipt,
                "Receipt did not include effective gas price."
            );
        }
        if let Some(gas_used) = receipt.gas_used {
            TX_GAS_USED.inc_by(gas_used.as_u128() as f64);
            if let Some(gas_limit) = tx.gas() {
                let gas_fraction = gas_used.as_u128() as f64 / gas_limit.as_u128() as f64;
                TX_GAS_FRACTION.observe(gas_fraction);
                if gas_fraction > 0.9 {
                    warn!(
                        ?nonce,
                        %gas_used,
                        %gas_limit,
                        %gas_fraction,
                        "Transaction used more than 90% of the gas limit."
                    );
                }
            }
            if let Some(gas_price) = receipt.effective_gas_price {
                let cost_wei = gas_used * gas_price;
                TX_WEI_USED.inc_by(cost_wei.as_u128() as f64);
            }
        } else {
            error!(?nonce, ?tx, ?receipt, "Receipt did not include gas used.");
        }

        // Check receipt status for success
        if receipt.status != Some(U64::from(1_u64)) {
            return Err(TxError::Failed(Box::new(receipt)));
        }
        Ok(receipt)
    }

    pub fn fetch_events_raw(
        &self,
        filter: &Filter,
    ) -> impl Stream<Item = Result<Log, EventError>> + '_ {
        self.provider
            .get_logs_paginated(filter, self.max_log_blocks as u64)
            .map_err(Into::into)
    }

    pub fn fetch_events<T: EthEvent>(
        &self,
        filter: &Filter,
    ) -> impl Stream<Item = Result<T, EventError>> + '_ {
        // TODO: Add `Log` struct for blocknumber and other metadata.
        self.fetch_events_raw(filter).map(|res| {
            res.and_then(|log| {
                T::decode_log(&RawLog {
                    topics: log.topics,
                    data:   log.data.to_vec(),
                })
                .map_err(Into::into)
            })
        })
    }
}
