use std::{
    cmp::{max, min},
    sync::Arc,
};
use async_stream::try_stream;
use ethers::{
    providers::{Middleware, ProviderError},
    types::{Filter, Log, U64},
};
use futures::Stream;
use serde_json::value::RawValue;
use thiserror::Error;
use crate::ethereum::ProviderStack;
use crate::database::{Database, DatabaseError};

pub struct CachingLogQuery {
    provider:  Arc<ProviderStack>,
    filter:    Filter,
    page_size: u64,
    database: Option<Arc<Database>>,
}

#[derive(Error, Debug)]
pub enum CachingLogQueryError<ProviderError> {
    #[error(transparent)]
    LoadLastBlockError(ProviderError),
    #[error(transparent)]
    LoadLogsError(ProviderError),
    #[error(transparent)]
    DatabaseError(#[from] DatabaseError),
    #[error(transparent)]
    ParseError(serde_json::Error),
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
    pub fn with_page_size(mut self, page_size: u64) -> Self {
        self.page_size = page_size;
        self
    }

    pub fn with_database(mut self, database: Option<Arc<Database>>) -> Self {
        self.database = database;
        self
    }

    pub fn into_stream(
        self,
    ) -> impl Stream<Item = Result<Log, CachingLogQueryError<ProviderError>>> {
        try_stream! {
            let provider = self.provider.provider();
            let last_eth_block = provider.get_block_number().await.map_err(CachingLogQueryError::LoadLastBlockError)?;
            let last_db_block: u64;
            if let Some(database) = &self.database {
                last_db_block = database.get_block_number().await? as u64;
                let db_logs: Vec<Box<RawValue>> = database.load_logs().await?;
                for log in db_logs {
                    yield serde_json::from_str(log.get()).map_err(CachingLogQueryError::ParseError)?;
                }
            } else {
                last_db_block = 0;
            }

            let mut from_block = max(U64([last_db_block + 1]), self.filter.get_from_block().unwrap_or(U64::default()));
            let to_block = self.filter.get_to_block().unwrap_or(last_eth_block);

            while from_block <= to_block {
                let page_end = min(last_eth_block, from_block + self.page_size);

                let page_filter = self.filter.clone()
                    .from_block(from_block)
                    .to_block(page_end);


                let data: Result<Vec<Box<RawValue>>, ProviderError> = provider
                    .request("eth_getLogs", [&page_filter])
                    .await;
                let result = data.map_err(CachingLogQueryError::LoadLogsError)?;
                if let Some(database) = &self.database {
                    database.save_logs(from_block.as_u64() as i64, page_end.as_u64() as i64, &result).await.map_err(CachingLogQueryError::DatabaseError)?;
                }
                for log in result {
                    yield serde_json::from_str(log.get()).map_err(CachingLogQueryError::ParseError)?;
                }

                from_block = page_end + 1;
            }
        }
    }
}
