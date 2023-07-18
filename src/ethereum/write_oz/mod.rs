use std::num::ParseIntError;
use std::str::FromStr;
use std::time::Duration;

use anyhow::Result as AnyhowResult;
use async_trait::async_trait;
use clap::Parser;
use ethers::providers::Middleware;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{Address, H160, U64};
use tracing::{info, warn};

use self::openzeppelin::OzRelay;
use super::write::{TransactionId, WriteProvider};
use super::{ReadProvider, TxError};

mod error;
mod openzeppelin;

fn duration_from_str(value: &str) -> Result<Duration, ParseIntError> {
    Ok(Duration::from_secs(u64::from_str(value)?))
}

// TODO: Log and metrics for signer / nonces.
#[derive(Clone, Debug, Eq, PartialEq, Parser)]
#[group(skip)]
pub struct Options {
    #[clap(long, env, default_value = "https://api.defender.openzeppelin.com")]
    pub oz_api_url: String,

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

    #[clap(long, env, value_parser=duration_from_str, default_value="60")]
    pub oz_send_timeout: Duration,

    #[clap(long, env, value_parser=duration_from_str, default_value="60")]
    pub oz_mine_timeout: Duration,

    #[clap(long, env)]
    pub oz_gas_limit: Option<u64>,
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

    async fn mine_transaction(&self, tx: TransactionId) -> Result<bool, TxError> {
        let oz_transaction_result = self.inner.mine_transaction(tx.clone()).await;

        if let Err(TxError::Failed(_)) = oz_transaction_result {
            warn!(?tx, "Transaction failed in OZ Relayer");

            return Ok(false);
        }

        let oz_transaction = oz_transaction_result?;

        let tx_hash = oz_transaction.hash.ok_or_else(|| {
            TxError::Fetch(From::from(format!(
                "Failed to get tx hash for transaction id {}",
                oz_transaction.transaction_id
            )))
        })?;

        info!(?tx_hash, "Waiting for transaction to be mined");

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

        if tx.status == Some(U64::from(1u64)) {
            Ok(true)
        } else {
            warn!(?tx, "Transaction failed");

            Ok(false)
        }
    }

    fn address(&self) -> Address {
        self.address
    }
}
