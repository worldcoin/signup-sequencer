mod estimator;
mod gas_oracle_logger;
mod rpc_logger;
mod transport;

use self::{
    estimator::Estimator, gas_oracle_logger::GasOracleLogger, rpc_logger::RpcLogger,
    transport::Transport,
};
use chrono::{Duration as ChronoDuration, Utc};
use ethers::{
    core::k256::ecdsa::SigningKey,
    middleware::{
        gas_oracle::{
            Cache, EthGasStation, Etherchain, GasNow, GasOracle, GasOracleMiddleware, Median,
            Polygon, ProviderOracle,
        },
        NonceManagerMiddleware, SignerMiddleware,
    },
    providers::{Middleware, Provider},
    signers::{LocalWallet, Signer, Wallet},
    types::{BlockId, BlockNumber, Chain, H160, H256},
};
use eyre::{eyre, Result as EyreResult};
use futures::{try_join, FutureExt};
use reqwest::Client as ReqwestClient;
use std::{sync::Arc, time::Duration};
use structopt::StructOpt;
use tracing::{error, info, instrument};
use url::Url;

const PENDING: Option<BlockId> = Some(BlockId::Number(BlockNumber::Pending));

#[derive(Clone, Debug, PartialEq, StructOpt)]
pub struct Options {
    /// Ethereum API Provider
    #[structopt(long, env, default_value = "http://localhost:8545")]
    pub ethereum_provider: Url,

    /// Private key used for transaction signing
    #[structopt(
        long,
        env,
        default_value = "ee79b5f6e221356af78cf4c36f4f7885a11b67dfcc81c34d80249947330c0f82"
    )]
    // NOTE: We abuse `Hash` here because it has the right `FromStr` implementation.
    pub signing_key: H256,

    /// If this module is being run with EIP-1559 support, useful in some places
    /// where EIP-1559 is not yet supported
    #[structopt(
        short,
        parse(try_from_str),
        default_value = "true",
        env = "USE_EIP1559"
    )]
    pub eip1559: bool,
}

// Code out the provider stack in types
// Needed because of <https://github.com/gakonst/ethers-rs/issues/592>
type Provider0 = Provider<RpcLogger<Transport>>;
type Provider1 = SignerMiddleware<Provider0, Wallet<SigningKey>>;
type Provider2 = NonceManagerMiddleware<Provider1>;
type Provider3 = Estimator<Provider2>;
type Provider4 = GasOracleMiddleware<Arc<Provider3>, Box<dyn GasOracle>>;
pub type ProviderStack = Provider4;

#[derive(Clone, Debug)]
pub struct Ethereum {
    provider: Arc<ProviderStack>,
    address:  H160,
}

impl Ethereum {
    #[instrument(level = "debug", skip_all)]
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
            let provider = { NonceManagerMiddleware::new(provider, address) };

            // Log wallet info.
            let (next_nonce, balance) = try_join!(
                provider.initialize_nonce(PENDING),
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
                    .map_err(|err| err.into())
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

            // Add a logging, caching and abstract the type.
            let logger = GasOracleLogger::new(median);
            let cache = Cache::new(Duration::from_secs(5), logger);
            let gas_oracle: Box<dyn GasOracle> = Box::new(cache);

            // Wrap in a middleware
            GasOracleMiddleware::new(provider, gas_oracle)
        };

        let provider = Arc::new(provider);
        Ok(Self { provider, address })
    }

    #[must_use]
    pub const fn provider(&self) -> &Arc<ProviderStack> {
        &self.provider
    }

    #[must_use]
    pub const fn address(&self) -> H160 {
        self.address
    }
}
