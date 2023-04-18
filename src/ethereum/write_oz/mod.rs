use std::time::Duration;

use anyhow::Result as AnyhowResult;
use async_trait::async_trait;
use clap::Parser;
use ethers::{
    providers::Middleware,
    types::{transaction::eip2718::TypedTransaction, Address, H160, U64},
};

use self::openzeppelin::OzRelay;
use super::{
    read::duration_from_str,
    write::{TransactionId, WriteProvider},
    ReadProvider, TxError,
};

mod error;
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
    #[clap(long, env)]
    pub oz_address: H160,

    /// For how long OpenZeppelin should track and retry the transaction (in
    /// seconds) Default: 7 days (7 * 24 * 60 * 60 = 604800 seconds)
    #[clap(long, env, value_parser=duration_from_str, default_value="604800")]
    pub oz_transaction_validity: Duration,
}

#[derive(Debug)]
pub struct Provider {
    read_provider: ReadProvider,
    inner:         OzRelay,
    address:       Address,
}

impl Provider {
    pub async fn new(read_provider: ReadProvider, options: &Options) -> AnyhowResult<Self> {
        let relay = OzRelay::new(options).await?;

        Ok(Self {
            read_provider,
            inner: relay,
            address: options.oz_address,
        })
    }
}

#[async_trait]
impl WriteProvider for Provider {
    async fn send_transaction(
        &self,
        tx: TypedTransaction,
        only_once: bool,
    ) -> Result<TransactionId, TxError> {
        self.inner.send_transaction(tx, only_once).await
    }

    async fn fetch_pending_transactions(&self) -> Result<Vec<TransactionId>, TxError> {
        self.inner.fetch_pending_transactions().await
    }

    async fn mine_transaction(&self, tx: TransactionId) -> Result<(), TxError> {
        let oz_transaction = self.inner.mine_transaction(tx).await?;

        let tx_hash = oz_transaction.hash.ok_or_else(|| {
            TxError::Fetch(From::from(format!(
                "Failed to get tx hash for transaction id {}",
                oz_transaction.transaction_id
            )))
        })?;

        let tx = self
            .read_provider
            .get_transaction_receipt(tx_hash)
            .await
            .map_err(|err| TxError::Fetch(err.into()))?;

        let tx = tx.ok_or_else(|| {
            TxError::Fetch(From::from(format!(
                "Failed to get transaction receipt for transaction id {}",
                oz_transaction.transaction_id
            )))
        })?;

        if tx.status != Some(U64::from(1u64)) {
            return Err(TxError::Failed(Some(tx)));
        }

        Ok(())
    }

    fn address(&self) -> Address {
        self.address
    }
}
