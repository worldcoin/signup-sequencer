use clap::Parser;
use ethers::types::{U256, U64};
use eyre::ErrReport;
use serde_json::value::RawValue;
use thiserror::Error;
use tracing::instrument;

#[derive(Clone, Debug, PartialEq, Eq, Parser)]
pub struct Options {}

#[derive(Error, Debug)]
pub enum DatabaseError {}

pub struct Database {}

impl Database {
    #[instrument(skip_all)]
    pub async fn new(_options: Options) -> Result<Self, ErrReport> {
        Ok(Self {})
    }

    pub async fn get_block_number(&self) -> Result<i64, DatabaseError> {
        panic!("you need to enable unstable_db feature to cache events")
    }

    pub async fn load_logs(&self) -> Result<Vec<Box<RawValue>>, DatabaseError> {
        panic!("you need to enable unstable_db feature to cache events")
    }

    pub async fn save_log(
        &self,
        _block_index: U64,
        _transaction_index: U64,
        _log_index: U256,
        _log: Box<RawValue>,
    ) -> Result<(), DatabaseError> {
        panic!("you need to enable unstable_db feature to cache events")
    }
}
