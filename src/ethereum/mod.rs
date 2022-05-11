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
        gas_oracle::{Cache, GasOracleMiddleware, Polygon},
        NonceManagerMiddleware, SignerMiddleware,
    },
    providers::{Middleware, Provider},
    signers::{LocalWallet, Signer, Wallet},
    types::{Address, BlockId, BlockNumber, Chain, H160, H256, U256},
};
use eyre::{eyre, Result as EyreResult};
use futures::try_join;
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

type GasOracle = Cache<GasOracleLogger<Polygon>>;

// Code out the provider stack in types
// Needed because of <https://github.com/gakonst/ethers-rs/issues/592>
type Provider0 = Provider<RpcLogger<Transport>>;
type Provider1 = SignerMiddleware<Provider0, Wallet<SigningKey>>;
type Provider2 = NonceManagerMiddleware<Provider1>;
type Provider3 = Estimator<Provider2>;
type Provider4 = GasOracleMiddleware<Provider3, GasOracle>;
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
        let (provider, chain_id) = {
            info!(
                provider = %&options.ethereum_provider,
                "Connecting to Ethereum"
            );
            let transport = Transport::new(options.ethereum_provider).await?;
            let logger = RpcLogger::new(transport);
            let provider = Provider::new(logger);

            // Fetch state of the chain.
            let (chain_id, latest_block) = try_join!(
                provider.get_chainid(),
                provider.get_block(BlockId::Number(BlockNumber::Latest))
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
            info!(%chain_id, %chain, %block_number, ?block_hash, %block_time, "Connected to Ethereum provider");

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
            (provider, chain_id)
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
        let provider = Estimator::new(provider, 1.10, 10e3);

        // Add a gas oracle.
        let provider = {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()?;
            let chain = Chain::try_from(chain_id)?;
            let gas_oracle = Polygon::with_client(client, chain)?;
            let gas_oracle = GasOracleLogger::new(gas_oracle);
            let gas_oracle = Cache::new(Duration::from_secs(5), gas_oracle);
            GasOracleMiddleware::new(provider, gas_oracle)
        };

        let provider = Arc::new(provider);
        Ok(Self { provider, address })
    }

    pub fn provider(&self) -> &Arc<ProviderStack> {
        &self.provider
    }

    pub fn address(&self) -> H160 {
        self.address
    }

    pub async fn send_tx() {
        todo!();
        // let commitment = U256::from(commitment.to_be_bytes());
        // let mut tx = self.semaphore.add_member(group_id.into(), commitment);
        // let pending_tx = if self.eip1559 {
        // self.provider.fill_transaction(&mut tx.tx, None).await?;
        // tx.tx.set_gas(10_000_000_u64); // HACK: ethers-rs estimate is wrong.
        // tx.tx.set_nonce(nonce);
        // info!(?tx, "Sending transaction");
        // self.provider.send_transaction(tx.tx, None).await?
        // } else {
        // Our tests use ganache which doesn't support EIP-1559 transactions
        // yet. tx = tx.legacy();
        // self.provider.fill_transaction(&mut tx.tx, None).await?;
        // tx.tx.set_nonce(nonce);
        //
        // quick hack to ensure tx is so overpriced that it won't get dropped
        // tx.tx.set_gas_price(
        // tx.tx
        // .gas_price()
        // .ok_or(eyre!("no gasPrice set"))?
        // .checked_mul(2_u64.into())
        // .ok_or(eyre!("overflow in gasPrice"))?,
        // );
        // info!(?tx, "Sending transaction");
        // self.provider.send_transaction(tx.tx, None).await?
        // };
        // let receipt = pending_tx
        // .await
        // .map_err(|e| eyre!(e))?
        // .ok_or_else(|| eyre!("tx dropped from mempool"))?;
        // info!(?receipt, "Receipt");
        // if receipt.status != Some(U64::from(1_u64)) {
        // return Err(eyre!("tx failed"));
        // }
    }
}
