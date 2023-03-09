use std::{fmt::Debug, time::Duration};

use anyhow::Result as AnyhowResult;
use ethers::{abi::AbiDecode, types::transaction::eip2718::TypedTransaction};
use hyper::StatusCode;
use once_cell::sync::Lazy;
use oz_api::{
    data::transactions::{RelayerTransactionBase, SendBaseTransactionRequest, Status},
    OzApi,
};
use prometheus::{register_int_counter_vec, IntCounterVec};
use tokio::{
    sync::{Mutex, MutexGuard},
    time::timeout,
};
use tracing::{error, info, info_span, Instrument};

use super::{error::Error, expiring_headers::ExpiringHeaders, Options};
use crate::{
    contracts::abi::RegisterIdentitiesCall,
    ethereum::{write::TransactionId, TxError},
};

const OZ_DEFENDER_RELAYER_URL: &str = "https://api.defender.openzeppelin.com";

static TX_COUNT: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!("eth_tx_count", "The transaction count by bytes4.", &[
        "bytes4"
    ])
    .unwrap()
});

#[derive(Debug)]
pub struct OzRelay {
    oz_api:               OzApi,
    expiring_headers:     Mutex<ExpiringHeaders>,
    api_key:              String,
    api_secret:           String,
    transaction_validity: chrono::Duration,
    send_timeout:         Duration,
    mine_timeout:         Duration,
}

impl OzRelay {
    pub async fn new(options: &Options) -> AnyhowResult<Self> {
        let api_key = options.oz_api_key.to_string();
        let api_secret = options.oz_api_secret.to_string();

        let expiring_headers = ExpiringHeaders::refresh(&api_key, &api_secret).await?;
        let expiring_headers = Mutex::new(expiring_headers);

        let oz_api = OzApi::new(OZ_DEFENDER_RELAYER_URL)?;

        Ok(Self {
            oz_api,
            expiring_headers,
            api_key,
            api_secret,
            transaction_validity: chrono::Duration::from_std(options.oz_transaction_validity)?,
            send_timeout: Duration::from_secs(60),
            mine_timeout: Duration::from_secs(60),
        })
    }

    async fn query(&self, tx_id: &str) -> Result<RelayerTransactionBase, Error> {
        let headers = self.headers().await.map_err(|_| Error::Authentication)?;

        let res = self
            .oz_api
            .query_transaction(tx_id)
            .map(|builder| headers.apply(builder))
            .send()
            .await
            .map_err(|_| Error::RequestFailed)?;

        let status = res.as_ref().status();
        let item = res.json().await.map_err(|e| {
            error!(?status, ?e, "error occurred, unknown response format");
            Error::UnknownResponseFormat
        })?;

        Ok(item)
    }

    async fn list_recent_pending_transactions(&self) -> Result<Vec<RelayerTransactionBase>, Error> {
        let headers = self.headers().await.map_err(|_| Error::Authentication)?;

        let res = self
            .oz_api
            .list_transactions(Some(Status::Pending), Some(10))
            .map(|builder| headers.apply(builder))
            .send()
            .await
            .map_err(|_| Error::RequestFailed)?;

        let status = res.as_ref().status();
        let items = res.json().await.map_err(|e| {
            error!(?status, ?e, "error occurred, unknown response format");
            Error::UnknownResponseFormat
        })?;

        Ok(items)
    }

    async fn mine_transaction_id_unchecked(
        &self,
        id: &str,
    ) -> Result<RelayerTransactionBase, TxError> {
        loop {
            let transaction = self.query(id).await.map_err(|error| {
                error!(?error, "Failed to get transaction status");
                TxError::Send(Box::new(error))
            })?;

            let status = transaction.status;

            // Terminal failure. The transaction won't be retried by OpenZeppelin. No reason
            // provided
            if status == Status::Failed {
                return Err(TxError::Failed(None));
            }

            match status {
                Status::Mined | Status::Confirmed => {
                    return Ok(transaction);
                }
                Status::Failed => {
                    return Err(TxError::Failed(None));
                }
                status => {
                    info!("Transaction status is {status} waiting 5 s to mine");
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

        let headers = self.headers().await.map_err(|_| Error::Authentication)?;

        let res = self
            .oz_api
            .send_transaction(api_tx)
            .map(|builder| headers.apply(builder))
            .send()
            .await
            .map_err(|_| Error::RequestFailed)?;

        if res.as_ref().status() == StatusCode::OK {
            let tx = res.json().await.map_err(|_| Error::UnknownResponseFormat)?;

            Ok(tx.transaction_id)
        } else {
            let text = res.into_untyped().text().await;
            info!(?text, "response error");

            Err(Error::UnknownResponseFormat)
        }
    }

    pub async fn fetch_pending_transactions(
        &self,
    ) -> Result<Vec<(TransactionId, RegisterIdentitiesCall)>, TxError> {
        let recent_pending_txs = self
            .list_recent_pending_transactions()
            .await
            .map_err(|err| TxError::Fetch(Box::new(err)))?;

        let mut pending_txs = Vec::with_capacity(recent_pending_txs.len());

        for tx in recent_pending_txs {
            let tx_id = tx.transaction_id;
            let tx_id = TransactionId(tx_id);

            let Some(data) = tx.data else { continue; };
            let decoded = match RegisterIdentitiesCall::decode(data) {
                Ok(decoded) => decoded,
                Err(err) => {
                    error!(?err, "Failed to decode transaction data");
                    continue;
                }
            };

            pending_txs.push((tx_id, decoded));
        }

        Ok(pending_txs)
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
        tx: TypedTransaction,
        only_once: bool,
    ) -> Result<TransactionId, TxError> {
        let mut tx = tx.clone();
        tx.set_gas(1_000_000);

        if only_once {
            info!("checking if can resubmit");

            let existing_transactions =
                self.list_recent_pending_transactions().await.map_err(|e| {
                    error!(?e, "error occurred");
                    TxError::Send(Box::new(e))
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
                TxError::Send(Box::new(error))
            })?;

        info!(?tx_id, "Transaction submitted to OZ Relay");

        Ok(TransactionId(tx_id))
    }

    pub async fn mine_transaction(&self, tx_id: TransactionId) -> Result<(), TxError> {
        self.mine_transaction_id(tx_id.0.as_str()).await?;

        Ok(())
    }

    async fn headers(&self) -> Result<MutexGuard<ExpiringHeaders>, Error> {
        let now = chrono::Utc::now().timestamp();

        let mut expiring_headers = self.expiring_headers.lock().await;

        if expiring_headers.expiration_time < now {
            let new_headers = ExpiringHeaders::refresh(&self.api_key, &self.api_secret)
                .await
                .map_err(|_| Error::Authentication)?;

            *expiring_headers = new_headers;
        }

        Ok(expiring_headers)
    }
}
