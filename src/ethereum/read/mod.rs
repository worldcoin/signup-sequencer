use self::{rpc_logger::RpcLogger, transport::Transport};
use crate::contracts::confirmed_log_query::{ConfirmedLogQuery, Error as CachingLogQueryError};
use anyhow::{anyhow, Result as AnyhowResult};
use chrono::{Duration as ChronoDuration, Utc};
use clap::Parser;
use ethers::{
    abi::{Error as AbiError, RawLog},
    contract::EthEvent,
    providers::{Middleware, Provider, ProviderError},
    types::{BlockId, BlockNumber, Chain, Filter, Log as EthLog, U256, U64},
};
use futures::{try_join, FutureExt, Stream, StreamExt, TryStreamExt};
use std::{num::ParseIntError, str::FromStr, time::Duration};
use thiserror::Error;
use tracing::{error, info};
use url::Url;

pub mod rpc_logger;
pub mod transport;

type InnerProvider = Provider<RpcLogger<Transport>>;

// TODO: Log and metrics for signer / nonces.
#[derive(Clone, Debug, PartialEq, Eq, Parser)]
#[group(skip)]
pub struct Options {
    /// Ethereum API Provider
    #[clap(long, env, default_value = "http://localhost:8545")]
    pub ethereum_provider: Url,

    /// Maximum number of blocks to pull events from in one request.
    #[clap(long, env, default_value = "100000")]
    pub max_log_blocks: usize,

    /// Minimum number of blocks to pull events from in one request.
    #[clap(long, env, default_value = "1000")]
    pub min_log_blocks: usize,

    /// Maximum amount of wait time before request is retried (seconds).
    #[clap(long, env, value_parser=duration_from_str, default_value="32")]
    pub max_backoff_time: Duration,

    /// Minimum number of blocks before events are considered confirmed.
    #[clap(long, env, default_value = "35")]
    pub confirmation_blocks_delay: usize,
}

fn duration_from_str(value: &str) -> Result<Duration, ParseIntError> {
    Ok(Duration::from_secs(u64::from_str(value)?))
}

#[allow(clippy::module_name_repetitions)]
#[derive(Clone, Debug)]
pub struct ReadProvider {
    inner:                     InnerProvider,
    pub chain_id:              U256,
    pub legacy:                bool,
    max_log_blocks:            usize,
    min_log_blocks:            usize,
    max_backoff_time:          Duration,
    confirmation_blocks_delay: usize,
}

impl ReadProvider {
    pub async fn new(options: Options) -> AnyhowResult<Self> {
        // Connect to the Ethereum provider
        // TODO: Allow multiple providers with failover / broadcast.
        // TODO: Requests don't seem to process in parallel. Check if this is
        // a limitation client side or server side.
        // TODO: Does the WebSocket impl handle dropped connections by
        // reconnecting? What is the timeout on stalled connections? What is
        // the retry policy?
        let (provider, chain_id, eip1559) = {
            info!(
                provider = %options.ethereum_provider,
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
            max_log_blocks: options.max_log_blocks,
            min_log_blocks: options.min_log_blocks,
            max_backoff_time: options.max_backoff_time,
            confirmation_blocks_delay: options.confirmation_blocks_delay,
        })
    }

    pub async fn confirmed_block_number(&self) -> Result<U64, EventError> {
        self.inner
            .provider()
            .get_block_number()
            .await
            .map(|num| num.saturating_sub(U64::from(self.confirmation_blocks_delay)))
            .map_err(|e| EventError::Fetching(CachingLogQueryError::LoadLastBlock(e)))
    }

    pub fn fetch_events_raw(
        &self,
        filter: &Filter,
    ) -> impl Stream<Item = Result<EthLog, EventError>> + '_ {
        ConfirmedLogQuery::new(self.clone(), filter)
            .with_start_page_size(self.max_log_blocks as u64)
            .with_min_page_size(self.min_log_blocks as u64)
            .with_max_backoff_time(self.max_backoff_time)
            .with_blocks_delay(self.confirmation_blocks_delay as u64)
            .into_stream()
            .map_err(Into::into)
    }

    pub fn fetch_events<T: EthEvent>(
        &self,
        filter: &Filter,
    ) -> impl Stream<Item = Result<Log<T>, EventError>> + '_ {
        self.fetch_events_raw(filter).map(|res| {
            res.and_then(|log| {
                let event = T::decode_log(&RawLog {
                    topics: log.topics.clone(),
                    data:   log.data.to_vec(),
                })
                .map_err(EventError::from)?;

                Ok(Log {
                    block_index: log.block_number.ok_or(EventError::EmptyBlockIndex)?,
                    transaction_index: log
                        .transaction_index
                        .ok_or(EventError::EmptyTransactionIndex)?,
                    log_index: log.log_index.ok_or(EventError::EmptyLogIndex)?,
                    raw_log: serde_json::to_string(&log).map_err(EventError::Serialize)?,
                    event,
                })
            })
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

pub struct Log<Event: EthEvent> {
    pub block_index:       U64,
    pub transaction_index: U64,
    pub log_index:         U256,
    pub raw_log:           String,
    pub event:             Event,
}

#[derive(Debug, Error)]
pub enum EventError {
    #[error("Error fetching log event: {0}")]
    Fetching(#[from] CachingLogQueryError<ProviderError>),
    #[error("Error parsing log event: {0}")]
    Parsing(#[from] AbiError),
    #[error("couldn't serialize log to json: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("empty block index")]
    EmptyBlockIndex,
    #[error("empty transaction index")]
    EmptyTransactionIndex,
    #[error("empty log index")]
    EmptyLogIndex,
}
