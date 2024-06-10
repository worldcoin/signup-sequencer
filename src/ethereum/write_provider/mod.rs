use std::fmt;
use std::sync::Arc;

use ethers::providers::Middleware;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{Address, U64};
use tracing::{info, warn};

use self::inner::Inner;
use self::openzeppelin::OzRelay;
use self::tx_sitter::TxSitter;
use super::{ReadProvider, TxError};
use crate::config::RelayerConfig;
use crate::identity::transaction_manager::TransactionId;

mod error;
mod inner;
mod openzeppelin;
mod tx_sitter;

pub struct WriteProvider {
    read_provider: ReadProvider,
    inner:         Arc<dyn Inner>,
    address:       Address,
}

impl fmt::Debug for WriteProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteProvider")
            .field("read_provider", &self.read_provider)
            .field("inner", &"<REDACTED>")
            .field("address", &self.address)
            .finish()
    }
}

impl WriteProvider {
    pub async fn new(read_provider: ReadProvider, config: &RelayerConfig) -> anyhow::Result<Self> {
        let address = config.address();

        let inner: Arc<dyn Inner> = match config {
            RelayerConfig::OzDefender(oz_config) => {
                tracing::info!("Initializing OZ Relayer");
                Arc::new(OzRelay::new(oz_config).await?)
            }
            RelayerConfig::TxSitter(tx_sitter_config) => {
                tracing::info!("Initializing TxSitter");
                Arc::new(TxSitter::new(tx_sitter_config))
            }
        };

        Ok(Self {
            read_provider,
            inner,
            address,
        })
    }

    pub async fn send_transaction(
        &self,
        tx: TypedTransaction,
        only_once: bool,
    ) -> Result<TransactionId, TxError> {
        self.inner.send_transaction(tx, only_once).await
    }

    pub async fn fetch_pending_transactions(&self) -> Result<Vec<TransactionId>, TxError> {
        self.inner.fetch_pending_transactions().await
    }

    pub async fn mine_transaction(&self, tx: TransactionId) -> Result<bool, TxError> {
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

    pub fn address(&self) -> Address {
        self.address
    }
}
