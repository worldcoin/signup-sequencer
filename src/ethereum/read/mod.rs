use anyhow::anyhow;
use chrono::{Duration as ChronoDuration, Utc};
use ethers::abi::Error as AbiError;
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{BlockId, BlockNumber, Chain, U256};
use futures::{try_join, FutureExt};
use thiserror::Error;
use tracing::{error, info};
use url::Url;

use self::rpc_logger::RpcLogger;

pub mod rpc_logger;

type InnerProvider = Provider<RpcLogger<Http>>;

#[derive(Clone, Debug)]
pub struct ReadProvider {
    inner:        InnerProvider,
    pub chain_id: U256,
    pub legacy:   bool,
}

impl ReadProvider {
    pub async fn new(url: Url) -> anyhow::Result<Self> {
        // Connect to the Ethereum provider
        // TODO: Allow multiple providers with failover / broadcast.
        // TODO: Requests don't seem to process in parallel. Check if this is
        // a limitation client side or server side.
        // TODO: Does the WebSocket impl handle dropped connections by
        // reconnecting? What is the timeout on stalled connections? What is
        // the retry policy?
        let (provider, chain_id, eip1559) = {
            info!(
                provider = %url,
                "Connecting to provider"
            );
            let transport = Http::new(url);
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
                .ok_or_else(|| anyhow!("Failed to get latest block from Ethereum provider"))?;
            let block_hash = latest_block
                .hash
                .ok_or_else(|| anyhow!("Could not read latest block hash"))?;
            let block_number = latest_block
                .number
                .ok_or_else(|| anyhow!("Could not read latest block number"))?;
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

        Ok(Self {
            inner: provider,
            chain_id,
            legacy: !eip1559,
        })
    }
}

impl Middleware for ReadProvider {
    type Error = <InnerProvider as Middleware>::Error;
    type Inner = InnerProvider;
    type Provider = <InnerProvider as Middleware>::Provider;

    fn inner(&self) -> &Self::Inner {
        &self.inner
    }
}

#[derive(Debug, Error)]
pub enum EventError {
    #[error("Error parsing log event: {0}")]
    Parsing(#[from] AbiError),
}
