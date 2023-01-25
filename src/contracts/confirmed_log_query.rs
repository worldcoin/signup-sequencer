use crate::ethereum::ReadProvider;
use async_stream::try_stream;
use core::fmt::Debug;
use ethers::{
    providers::{LogQueryError, Middleware, ProviderError},
    types::{Filter, Log, U64},
};
use futures::{Stream, StreamExt};
use std::{cmp::max, sync::Arc, time::Duration};
use thiserror::Error;
use tokio::time::sleep;
use tracing::{error, info};

pub struct ConfirmedLogQuery {
    provider:                  ReadProvider,
    filter:                    Filter,
    start_page_size:           u64,
    min_page_size:             u64,
    max_backoff_time:          Duration,
    confirmation_blocks_delay: u64,
}

#[derive(Error, Debug)]
pub enum Error<ProviderError> {
    #[error("couldn't load last block number: {0}")]
    LoadLastBlock(#[source] ProviderError),
    #[error("error loading logs")]
    LoadLogs(#[from] LogQueryError<ProviderError>),
}

impl ConfirmedLogQuery {
    pub fn new(provider: ReadProvider, filter: &Filter) -> Self {
        Self {
            provider,
            filter: filter.clone(),
            start_page_size: 10000,
            min_page_size: 1000,
            max_backoff_time: Duration::from_secs(32),
            confirmation_blocks_delay: 0,
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

    pub fn into_stream(self) -> impl Stream<Item = Result<Log, Error<ProviderError>>> {
        try_stream! {
            let last_block = self.get_block_number().await?;

            let mut retry_status = RetryStatus::new(self.start_page_size, self.min_page_size, self.max_backoff_time);

            'restart: loop {
                let filter = self.filter.clone().from_block(max(
                    retry_status.last_block + 1,
                    self.filter.get_from_block().unwrap_or_default(),
                ));

                info!(
                    page_size = retry_status.page_size,
                    from_block = filter.get_from_block().unwrap_or_default().as_u64(),
                    "Reading MemberAdded events"
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

                    let log_block = match log.block_number {
                        Some(block) => block,
                        None => continue,
                    };
                    let to_filter = self.filter.get_to_block().expect("filter's to_block must be set");

                    // get_logs_paginated ignores to_block filter. Check again if the block is confirmed
                    let is_confirmed = log_block + self.confirmation_blocks_delay <= last_block && log_block <= to_filter;
                    if is_confirmed {
                        yield log;
                    }
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

    async fn get_block_number(&self) -> Result<U64, Error<ProviderError>> {
        let provider = self.provider.provider();
        provider
            .get_block_number()
            .await
            .map_err(Error::LoadLastBlock)
    }
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
