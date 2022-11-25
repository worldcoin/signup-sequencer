use crate::{
    database::{Database, Error as DatabaseError},
    ethereum::ProviderStack,
};
use async_stream::try_stream;
use core::fmt::Debug;
use ethers::{
    providers::{LogQueryError, Middleware, ProviderError},
    types::{Filter, Log, U64},
};
use futures::{Stream, StreamExt};
use std::{cmp::max, num::TryFromIntError, sync::Arc, time::Duration};
use thiserror::Error;
use tokio::time::sleep;
use tracing::{error, info};

pub struct CachingLogQuery {
    provider:                  Arc<ProviderStack>,
    filter:                    Filter,
    start_page_size:           u64,
    min_page_size:             u64,
    max_backoff_time:          Duration,
    confirmation_blocks_delay: u64,
    database:                  Option<Arc<Database>>,
}

#[derive(Error, Debug)]
pub enum Error<ProviderError> {
    #[error("couldn't load last block number: {0}")]
    LoadLastBlock(#[source] ProviderError),
    #[error("error loading logs")]
    LoadLogs(#[from] LogQueryError<ProviderError>),
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error("couldn't parse log json: {0}")]
    Parse(#[source] serde_json::Error),
    #[error("couldn't serialize log to json: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("empty block index")]
    EmptyBlockIndex,
    #[error("empty transaction index")]
    EmptyTransactionIndex,
    #[error("empty log index")]
    EmptyLogIndex,
    #[error("block index out of range: {0}")]
    BlockIndexOutOfRange(String),
    #[error("transaction index out of range: {0}")]
    TransactionIndexOutOfRange(String),
    #[error("log index out of range: {0}")]
    LogIndexOutOfRange(String),
}

impl CachingLogQuery {
    pub fn new(provider: Arc<ProviderStack>, filter: &Filter) -> Self {
        Self {
            provider,
            filter: filter.clone(),
            start_page_size: 10000,
            min_page_size: 1000,
            max_backoff_time: Duration::from_secs(32),
            confirmation_blocks_delay: 0,
            database: None,
        }
    }

    /// set page size for pagination
    pub const fn with_start_page_size(mut self, page_size: u64) -> Self {
        self.start_page_size = page_size;
        self
    }

    pub const fn with_min_page_size(mut self, page_size: u64) -> Self {
        self.min_page_size = page_size;
        self
    }

    pub const fn with_max_backoff_time(mut self, time: Duration) -> Self {
        self.max_backoff_time = time;
        self
    }

    pub const fn with_blocks_delay(mut self, confirmation_blocks_delay: u64) -> Self {
        self.confirmation_blocks_delay = confirmation_blocks_delay;
        self
    }

    pub fn with_database(mut self, database: Arc<Database>) -> Self {
        self.database = Some(database);
        self
    }

    pub fn into_stream(self) -> impl Stream<Item = Result<Log, Error<ProviderError>>> {
        try_stream! {
            let last_block = self.get_block_number().await?;

            let cached_events = self.load_db_logs(
                self.filter.get_from_block().unwrap().as_u64(),
                self.filter.get_to_block().map(|num| num.as_u64())
            ).await?;

            if cached_events.len() > 0 {
                info!("Reading MemberAdded events from cache");
            }

            for log in cached_events {
                yield serde_json::from_str(&log).map_err(Error::Parse)?;
            }

            let mut retry_status = RetryStatus::new(self.start_page_size, self.min_page_size, self.max_backoff_time);

            'restart: loop {
                let filter = self.filter.clone().from_block(max(
                    max(retry_status.last_block + 1, last_block.db + 1),
                    self.filter.get_from_block().unwrap_or_default(),
                ));

                info!(
                    page_size = retry_status.page_size,
                    from_block = filter.get_from_block().unwrap_or_default().as_u64(),
                    "Reading MemberAdded events from chains"
                );

                let mut stream = self.provider
                    .get_logs_paginated(&filter, retry_status.page_size);

                while let Some(log) = stream.next().await {
                    let log = match self.handle_retriable_err(log, &mut retry_status).await {
                        RetriableResult::Ok(log) => log,
                        RetriableResult::Restart => continue 'restart,
                        RetriableResult::Err(e) => {
                            // Yield an error before finishing the stream
                            Err(e)?;
                            return
                        }
                    };

                    // If we need to decrease the page size later, don't process older blocks twice
                    retry_status.update_last_block(log.block_number);

                    // get_logs_paginated ignores to_block filter. Check again if the block is confirmed
                    if self.is_confirmed(&log, last_block) {
                        let raw_log = serde_json::to_string(&log).map_err(Error::Serialize)?;
                        self.cache_log(raw_log, &log).await?;
                    }

                    yield log;
                }

                // We've iterated over all events, we can kill the restart loop
                break;
            }
        }
    }

    async fn handle_retriable_err(
        &self,
        log: Result<Log, LogQueryError<ProviderError>>,
        stats: &mut RetryStatus,
    ) -> RetriableResult<Log, Error<ProviderError>> {
        match log {
            Err(e) if e.to_string().contains("Query timeout exceeded") => {
                stats.attempt_restart(Error::LoadLogs(e)).await
            }
            Err(e) => RetriableResult::Err(Error::LoadLogs(e)),
            Ok(log) => RetriableResult::Ok(log),
        }
    }

    async fn get_block_number(&self) -> Result<LastBlock, Error<ProviderError>> {
        let provider = self.provider.provider();
        let last_eth_block = provider
            .get_block_number()
            .await
            .map_err(Error::LoadLastBlock)?;
        let last_db_block: u64 = match &self.database {
            Some(database) => database.get_block_number().await?,
            None => 0,
        };

        Ok(LastBlock {
            eth: last_eth_block,
            db:  U64([last_db_block]),
        })
    }

    async fn load_db_logs(
        &self,
        from_block: u64,
        to_block: Option<u64>,
    ) -> Result<Vec<String>, Error<ProviderError>> {
        let from: i64 = from_block.try_into().map_err(|e: TryFromIntError| {
            Error::<ProviderError>::BlockIndexOutOfRange(e.to_string())
        })?;
        let to: Option<i64> = match to_block {
            Some(num) => Some(
                i64::try_from(num)
                    .map_err(|e| Error::<ProviderError>::BlockIndexOutOfRange(e.to_string()))?,
            ),
            None => None,
        };
        match &self.database {
            Some(database) => database.load_logs(from, to).await.map_err(Error::Database),
            None => Ok(vec![]),
        }
    }

    fn is_confirmed(&self, log: &Log, last_block: LastBlock) -> bool {
        log.block_number.map_or(false, |block| {
            block + self.confirmation_blocks_delay <= last_block.eth
        })
    }

    async fn cache_log(&self, raw_log: String, log: &Log) -> Result<(), Error<ProviderError>> {
        if let Some(database) = &self.database {
            database
                .save_log(
                    log.block_number
                        .ok_or(Error::<ProviderError>::EmptyBlockIndex)?
                        .try_into()
                        .map_err(|e: &str| {
                            Error::<ProviderError>::BlockIndexOutOfRange(e.into())
                        })?,
                    log.transaction_index
                        .ok_or(Error::<ProviderError>::EmptyTransactionIndex)?
                        .try_into()
                        .map_err(|e: &str| {
                            Error::<ProviderError>::TransactionIndexOutOfRange(e.into())
                        })?,
                    log.log_index
                        .ok_or(Error::<ProviderError>::EmptyLogIndex)?
                        .try_into()
                        .map_err(|e: &str| Error::<ProviderError>::LogIndexOutOfRange(e.into()))?,
                    raw_log,
                )
                .await
                .map_err(Error::Database)?;
        }

        Ok(())
    }
}

#[derive(Copy, Clone)]
struct LastBlock {
    eth: U64,
    db:  U64,
}

struct RetryStatus {
    last_block:       U64,
    page_size:        u64,
    min_page_size:    u64,
    backoff_time:     Duration,
    max_backoff_time: Duration,
}

impl RetryStatus {
    fn new(page_size: u64, min_page_size: u64, max_backoff_time: Duration) -> Self {
        Self {
            last_block: U64::default(),
            page_size,
            min_page_size,
            backoff_time: Duration::from_secs(1),
            max_backoff_time,
        }
    }

    fn update_last_block(&mut self, last_block: Option<U64>) {
        self.last_block = last_block.unwrap_or(self.last_block);
    }

    async fn attempt_restart<T, E>(&mut self, error: E) -> RetriableResult<T, E>
    where
        E: Debug + Sync + Send,
    {
        if self.page_size >= self.min_page_size {
            error!(?error, "Retriable error, decreasing page size");
            self.page_size /= 2;
            RetriableResult::Restart
        } else if self.backoff_time <= self.max_backoff_time {
            error!(?error, "Retriable error, backoff");
            sleep(self.backoff_time).await;
            self.backoff_time *= 2;
            RetriableResult::Restart
        } else {
            error!(?error, "Retriable error, max number of attempts reached");
            RetriableResult::Err(error)
        }
    }
}

enum RetriableResult<T, E> {
    Ok(T),
    Restart,
    Err(E),
}
