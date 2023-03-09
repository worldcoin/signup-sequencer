use std::time::Duration;

use anyhow::Result as AnyhowResult;
use async_trait::async_trait;
use clap::Parser;
use ethers::types::{transaction::eip2718::TypedTransaction, Address, H160};

use self::openzeppelin::OzRelay;
use super::{
    read::duration_from_str,
    write::{TransactionId, WriteProvider},
    TxError,
};
use crate::contracts::abi::RegisterIdentitiesCall;

mod error;
mod expiring_headers;
mod openzeppelin;

// TODO: Log and metrics for signer / nonces.
#[derive(Clone, Debug, Eq, PartialEq, Parser)]
#[group(skip)]
pub struct Options {
    /// OpenZeppelin Defender API Key
    #[clap(long, env)]
    pub oz_api_key: String,

    /// OpenZeppelin Defender API Secret
    #[clap(long, env)]
    pub oz_api_secret: String,

    /// OpenZeppelin Defender API Secret
    #[clap(
        long,
        env,
        default_value = "0x30dcc24131223d4f8af69226e7b11b83e6a68b8b"
    )]
    pub oz_address: H160,

    /// For how long OpenZeppelin should track and retry the transaction (in
    /// seconds) Default: 7 days (7 * 24 * 60 * 60 = 604800 seconds)
    #[clap(long, env, value_parser=duration_from_str, default_value="604800")]
    pub oz_transaction_validity: Duration,
}

#[derive(Debug)]
pub struct Provider {
    inner:   OzRelay,
    address: Address,
}

impl Provider {
    pub async fn new(options: &Options) -> AnyhowResult<Self> {
        let relay = OzRelay::new(options).await?;

        Ok(Self {
            inner:   relay,
            address: options.oz_address,
        })
    }
}

#[async_trait]
impl WriteProvider for Provider {
    async fn fetch_pending_transactions(
        &self,
    ) -> Result<Vec<(TransactionId, RegisterIdentitiesCall)>, TxError> {
        self.inner.fetch_pending_transactions().await
    }

    async fn send_transaction(
        &self,
        tx: TypedTransaction,
        only_once: bool,
    ) -> Result<TransactionId, TxError> {
        self.inner.send_transaction(tx, only_once).await
    }

    async fn mine_transaction(&self, tx: TransactionId) -> Result<(), TxError> {
        self.inner.mine_transaction(tx).await
    }

    fn address(&self) -> Address {
        self.address
    }
}
