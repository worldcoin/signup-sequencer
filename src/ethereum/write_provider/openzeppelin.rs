use std::fmt::Debug;
use std::time::Duration;

use ethers::types::transaction::eip2718::TypedTransaction;
use once_cell::sync::Lazy;
use oz_api::data::transactions::{RelayerTransactionBase, SendBaseTransactionRequest, Status};
use oz_api::OzApi;
use prometheus::{register_int_counter_vec, IntCounterVec};
use tokio::time::timeout;
use tracing::{error, info, info_span, Instrument};

use super::error::Error;
use super::inner::{Inner, TransactionResult};
use crate::config::OzDefenderConfig;
use crate::ethereum::write::TransactionId;
use crate::ethereum::TxError;

static TX_COUNT: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!("eth_tx_count", "The transaction count by bytes4.", &[
        "bytes4"
    ])
    .unwrap()
});

#[derive(Debug)]
pub struct OzRelay {
    oz_api:               OzApi,
    transaction_validity: chrono::Duration,
    send_timeout:         Duration,
    mine_timeout:         Duration,
    gas_limit:            Option<u64>,
}

impl OzRelay {
    pub async fn new(options: &OzDefenderConfig) -> anyhow::Result<Self> {
        let oz_api = if options.oz_api_key.is_empty() && options.oz_api_secret.is_empty() {
            tracing::warn!(
                "OpenZeppelin Defender API Key and Secret are empty. Connection will operate \
                 without authentication headers. Use only in development."
            );

            OzApi::without_auth(&options.oz_api_url)?
        } else {
            OzApi::new(
                &options.oz_api_url,
                &options.oz_api_key,
                &options.oz_api_secret,
            )
            .await?
        };

        Ok(Self {
            oz_api,
            transaction_validity: chrono::Duration::from_std(options.oz_transaction_validity)?,
            send_timeout: options.oz_send_timeout,
            mine_timeout: options.oz_mine_timeout,
            gas_limit: options.oz_gas_limit,
        })
    }

    async fn query(&self, tx_id: &str) -> Result<RelayerTransactionBase, Error> {
        let tx = self.oz_api.query_transaction(tx_id).await?;

        Ok(tx)
    }

    async fn list_recent_transactions(&self) -> Result<Vec<RelayerTransactionBase>, Error> {
        let transactions = self.oz_api.list_transactions(None, Some(10)).await?;

        Ok(transactions)
    }

    async fn mine_transaction_id_unchecked(
        &self,
        id: &str,
    ) -> Result<RelayerTransactionBase, TxError> {
        loop {
            let transaction = self.query(id).await.map_err(|error| {
                error!(?error, "Failed to get transaction status");
                TxError::Send(error.into())
            })?;

            let status = transaction.status;

            // Terminal failure. The transaction won't be retried by OpenZeppelin. No reason
            // provided
            match status {
                Status::Failed => return Err(TxError::Failed(None)),
                Status::Mined | Status::Confirmed => return Ok(transaction),
                _ => {
                    info!("waiting 5 s to mine");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn mine_transaction_id(&self, id: &str) -> Result<RelayerTransactionBase, TxError> {
        timeout(self.mine_timeout, self.mine_transaction_id_unchecked(id))
            .await
            .map_err(|_| TxError::ConfirmationTimeout)?
    }

    async fn send_oz_transaction<T: Into<TypedTransaction> + Send + Sync>(
        &self,
        tx: T,
    ) -> Result<String, Error> {
        let tx: TypedTransaction = tx.into();
        let api_tx = SendBaseTransactionRequest {
            to:          tx.to(),
            value:       tx.value(),
            gas_limit:   tx.gas(),
            data:        tx.data(),
            valid_until: Some(chrono::Utc::now() + self.transaction_validity),
        };

        let tx = self.oz_api.send_transaction(api_tx).await?;

        Ok(tx.transaction_id)
    }

    /// When `only_once` is set to true, this method tries to be idempotent.
    ///
    /// Before submiting a transaction, it'll query `OpenZepellin` for the list
    /// of 10 most recent transactions to see if it's not processing already
    ///
    /// `OpenZeppelin` doesn't provide guarantees on how fast transactions will
    /// show up on the list of recent transactions ("order of seconds to be
    /// safe"). Don't rely on `only_once` option in high frequency code.
    /// This is mostly useful to recover from timeouts or app crashes that
    /// take multiple seconds to restart.
    pub async fn send_transaction(
        &self,
        mut tx: TypedTransaction,
        only_once: bool,
    ) -> Result<TransactionId, TxError> {
        if let Some(gas_limit) = self.gas_limit {
            tx.set_gas(gas_limit);
        }

        if only_once {
            info!("checking if can resubmit");

            let existing_transactions = self.list_recent_transactions().await.map_err(|e| {
                error!(?e, "error occurred");
                TxError::Send(e.into())
            })?;

            let existing_transaction =
                existing_transactions
                    .iter()
                    .find(|el| match (&el.data, tx.data()) {
                        (Some(a), Some(b)) => a == b,
                        _ => false,
                    });

            if let Some(existing_transaction) = existing_transaction {
                info!(only_once, "mining previously submitted transaction");

                let transaction_id = existing_transaction.transaction_id.clone();

                self.mine_transaction_id(&transaction_id).await?;

                return Ok(TransactionId(transaction_id));
            }
        }

        info!(?tx, gas_limit=?tx.gas(), "Sending transaction.");
        let bytes4: u32 = tx.data().map_or(0, |data| {
            let mut buffer = [0; 4];
            buffer.copy_from_slice(&data.as_ref()[..4]); // TODO: Don't panic.
            u32::from_be_bytes(buffer)
        });
        let bytes4 = format!("{bytes4:8x}");
        TX_COUNT.with_label_values(&[&bytes4]).inc();

        // Send TX to OZ Relay
        let tx_id = timeout(self.send_timeout, self.send_oz_transaction(tx.clone()))
            .instrument(info_span!("Send TX to mempool"))
            .await
            .map_err(|elapsed| {
                error!(?elapsed, "Send transaction timed out");
                TxError::SendTimeout
            })?
            .map_err(|error| {
                error!(?error, "Failed to send transaction");
                TxError::Send(error.into())
            })?;

        info!(?tx_id, "Transaction submitted to OZ Relay");

        Ok(TransactionId(tx_id))
    }

    pub async fn mine_transaction(
        &self,
        tx_id: TransactionId,
    ) -> Result<RelayerTransactionBase, TxError> {
        self.mine_transaction_id(tx_id.0.as_str()).await
    }

    pub async fn fetch_pending_transactions(&self) -> Result<Vec<TransactionId>, TxError> {
        let recent_pending_txs = self
            .list_recent_transactions()
            .await
            .map_err(|err| TxError::Fetch(Box::new(err)))?;

        let pending_txs = recent_pending_txs
            .into_iter()
            .map(|tx| TransactionId(tx.transaction_id))
            .collect();

        Ok(pending_txs)
    }
}

#[async_trait::async_trait]
impl Inner for OzRelay {
    async fn send_transaction(
        &self,
        tx: TypedTransaction,
        only_once: bool,
    ) -> Result<TransactionId, TxError> {
        self.send_transaction(tx, only_once).await
    }

    async fn fetch_pending_transactions(&self) -> Result<Vec<TransactionId>, TxError> {
        self.fetch_pending_transactions().await
    }

    async fn mine_transaction(&self, tx: TransactionId) -> Result<TransactionResult, TxError> {
        let transaction = self.mine_transaction(tx).await?;

        Ok(TransactionResult {
            transaction_id: transaction.transaction_id,
            hash:           transaction.hash,
        })
    }
}
