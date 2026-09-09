use alloy::network::Ethereum;
use alloy::primitives::U256;
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::client::RpcClient;
use alloy::rpc::types::BlockNumberOrTag;
use alloy::transports::http::Http;
use anyhow::anyhow;
use chrono::{Duration as ChronoDuration, Utc};
use futures::try_join;
use tracing::{error, info};
use url::Url;

use self::rpc_logger::RpcLogger;

pub mod rpc_logger;

type InnerProvider = RootProvider;

#[derive(Clone, Debug)]
pub struct ReadProvider {
    inner: InnerProvider,
    pub chain_id: U256,
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
        let (provider, chain_id) = {
            info!(
                provider = %url,
                "Connecting to provider"
            );
            let transport = Http::new(url);
            let logger = RpcLogger::new(transport);
            let provider: RootProvider = RootProvider::new(RpcClient::new(logger, false));

            // Fetch state of the chain.
            let (version, chain_id, latest_block) = try_join!(
                async { provider.get_client_version().await },
                async { provider.get_chain_id().await },
                async { provider.get_block_by_number(BlockNumberOrTag::Latest).await },
            )?;

            // Identify chain.
            let chain = chain_id.to_string();

            // Log chain state.
            let latest_block = latest_block
                .ok_or_else(|| anyhow!("Failed to get latest block from Ethereum provider"))?;
            let block_hash = latest_block.header.hash;
            let block_number = latest_block.header.number;
            let block_time =
                chrono::DateTime::from_timestamp(latest_block.header.timestamp.try_into()?, 0)
                    .ok_or_else(|| anyhow!("Invalid block timestamp"))?;
            info!(%version, %chain_id, %chain, %block_number, ?block_hash, %block_time, "Connected to Ethereum provider");

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

        Ok(Self {
            inner: provider,
            chain_id: U256::from(chain_id),
        })
    }
}

impl Provider<Ethereum> for ReadProvider {
    fn root(&self) -> &RootProvider<Ethereum> {
        self.inner.root()
    }
}
