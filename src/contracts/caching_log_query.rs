use crate::{
    database::{Database, DatabaseError},
    ethereum::ProviderStack,
};
use async_stream::try_stream;
use ethers::{
    providers::{Middleware, ProviderError},
    types::{Filter, Log, U64},
};
use futures::Stream;
use serde_json::value::RawValue;
use std::{
    cmp::{max, min},
    sync::Arc,
};
use thiserror::Error;

pub struct CachingLogQuery {
    provider:  Arc<ProviderStack>,
    filter:    Filter,
    page_size: u64,
    database:  Option<Arc<Database>>,
}

#[derive(Error, Debug)]
pub enum Error<ProviderError> {
    #[error(transparent)]
    LoadLastBlock(ProviderError),
    #[error(transparent)]
    LoadLogs(ProviderError),
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error(transparent)]
    Parse(serde_json::Error),
    #[error("empty block index")]
    EmptyBlockIndex,
    #[error("empty transaction index")]
    EmptyTransactionIndex,
    #[error("empty log index")]
    EmptyLogIndex,
}

impl CachingLogQuery {
    pub fn new(provider: Arc<ProviderStack>, filter: &Filter) -> Self {
        Self {
            provider,
            filter: filter.clone(),
            page_size: 10000,
            database: None,
        }
    }

    /// set page size for pagination
    pub const fn with_page_size(mut self, page_size: u64) -> Self {
        self.page_size = page_size;
        self
    }

    #[allow(clippy::missing_const_for_fn)]
    pub fn with_database(mut self, database: Option<Arc<Database>>) -> Self {
        self.database = database;
        self
    }

    pub fn into_stream(self) -> impl Stream<Item = Result<Log, Error<ProviderError>>> {
        try_stream! {
            let last_block = self.get_block_number().await?;

            let cached_events = self.load_db_logs().await?;
            for log in cached_events {
                yield serde_json::from_str(log.get()).map_err(Error::Parse)?;
            }

            let paginator = Paginator::new(last_block, self.filter.clone(), self.page_size);
            for page in paginator {
                let new_events = self.fetch_page(page.filter).await?;
                for raw_log in new_events {
                    let log: Log = serde_json::from_str(raw_log.get()).map_err(Error::Parse)?;
                    self.cache_log(raw_log, &log).await?;
                    yield log;
                }
            }
        }
    }

    async fn get_block_number(&self) -> Result<LastBlock, Error<ProviderError>> {
        let provider = self.provider.provider();
        let last_eth_block = provider
            .get_block_number()
            .await
            .map_err(Error::LoadLastBlock)?;
        let last_db_block: u64 = match &self.database {
            Some(database) => database.get_block_number().await? as u64,
            None => 0,
        };

        Ok(LastBlock {
            eth: last_eth_block,
            db:  U64([last_db_block]),
        })
    }

    async fn load_db_logs(&self) -> Result<Vec<Box<RawValue>>, Error<ProviderError>> {
        match &self.database {
            Some(database) => database.load_logs().await.map_err(Error::Database),
            None => Ok(vec![]),
        }
    }

    async fn fetch_page(&self, filter: Filter) -> Result<Vec<Box<RawValue>>, Error<ProviderError>> {
        let data: Result<Vec<Box<RawValue>>, ProviderError> = self
            .provider
            .provider()
            .request("eth_getLogs", [&filter])
            .await;
        data.map_err(Error::LoadLogs)
    }

    async fn cache_log(
        &self,
        raw_log: Box<RawValue>,
        log: &Log,
    ) -> Result<(), Error<ProviderError>> {
        if let Some(database) = &self.database {
            database
                .save_log(
                    log.block_number
                        .ok_or(Error::<ProviderError>::EmptyBlockIndex)?
                        .as_u64(),
                    log.transaction_index
                        .ok_or(Error::<ProviderError>::EmptyTransactionIndex)?
                        .as_u64(),
                    log.log_index
                        .ok_or(Error::<ProviderError>::EmptyLogIndex)?
                        .into(),
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

struct Page {
    filter: Filter,
}

struct Paginator {
    current_block: U64,
    last_block:    U64,
    filter:        Filter,
    page_size:     u64,
}

impl Paginator {
    fn new(last_block: LastBlock, filter: Filter, page_size: u64) -> Self {
        Self {
            current_block: max(
                last_block.db + 1,
                filter.get_from_block().unwrap_or_default(),
            ),
            last_block: filter.get_to_block().unwrap_or(last_block.eth),
            filter,
            page_size,
        }
    }
}

impl Iterator for Paginator {
    type Item = Page;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_block > self.last_block {
            return None;
        }

        let from_block = self.current_block;
        let to_block = min(self.last_block, self.current_block + self.page_size);
        let filter = self
            .filter
            .clone()
            .from_block(from_block)
            .to_block(to_block);
        let result = Page { filter };

        self.current_block = self.current_block + self.page_size + 1;

        Some(result)
    }
}
