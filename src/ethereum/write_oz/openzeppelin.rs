use std::fmt::Debug;
use std::time::Duration;

use anyhow::{Context, Result as AnyhowResult};
use chrono::{DateTime, Utc};
use cognitoauth::cognito_srp_auth::{auth, CognitoAuthInput};
use ethers::providers::ProviderError;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{Bytes, NameOrAddress, TxHash, U256};
use hyper::StatusCode;
use once_cell::sync::Lazy;
use prometheus::{register_int_counter_vec, IntCounterVec};
use reqwest::header::HeaderValue;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;
use tokio::sync::{Mutex, MutexGuard};
use tokio::time::timeout;
use tracing::{error, info, info_span, Instrument};

use super::Options;
use crate::ethereum::write::TransactionId;
use crate::ethereum::TxError;

// Same for every project, taken from here: https://docs.openzeppelin.com/defender/api-auth
const RELAY_TXS_URL: &str = "https://api.defender.openzeppelin.com/txs";
const CLIENT_ID: &str = "1bpd19lcr33qvg5cr3oi79rdap";
const POOL_ID: &str = "us-west-2_iLmIggsiy";

static TX_COUNT: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!("eth_tx_count", "The transaction count by bytes4.", &[
        "bytes4"
    ])
    .unwrap()
});

#[derive(Debug)]
pub struct OzRelay {
    client:               Mutex<ExpiringClient>,
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

        let client = get_client(&api_key, &api_secret).await?;
        let client = Mutex::new(client);

        Ok(Self {
            client,
            api_key,
            api_secret,
            transaction_validity: chrono::Duration::from_std(options.oz_transaction_validity)?,
            send_timeout: Duration::from_secs(60),
            mine_timeout: Duration::from_secs(60),
        })
    }

    async fn query(&self, tx_id: &str) -> Result<SubmittedTransaction, Error> {
        let url = format!("{RELAY_TXS_URL}/{tx_id}");

        let res = self
            .client()
            .await
            .map_err(|_| Error::Authentication)?
            .as_ref()
            .get(url)
            .send()
            .await
            .map_err(|_| Error::RequestFailed)?;

        let status = res.status();
        let item = res.json::<SubmittedTransaction>().await.map_err(|e| {
            error!(?status, ?e, "error occurred, unknown response format");
            Error::UnknownResponseFormat
        })?;

        Ok(item)
    }

    async fn list_recent_transactions(&self) -> Result<Vec<SubmittedTransaction>, Error> {
        let res = self
            .client()
            .await
            .map_err(|_| Error::Authentication)?
            .as_ref()
            .get(format!("{RELAY_TXS_URL}?limit=10"))
            .send()
            .await
            .map_err(|_| Error::RequestFailed)?;

        let status = res.status();
        let items = res.json::<Vec<SubmittedTransaction>>().await.map_err(|e| {
            error!(?status, ?e, "error occurred, unknown response format");
            Error::UnknownResponseFormat
        })?;

        Ok(items)
    }

    async fn mine_transaction_id_unchecked(
        &self,
        id: &str,
    ) -> Result<SubmittedTransaction, TxError> {
        loop {
            let transaction = self.query(id).await.map_err(|error| {
                error!(?error, "Failed to get transaction status");
                TxError::Send(Box::new(error))
            })?;
            let status = transaction
                .status
                .as_ref()
                .ok_or_else(|| TxError::Dropped(TxHash::default()))?;

            // Transaction statuses documented here:
            // https://docs.openzeppelin.com/defender/relay-api-reference#transaction-status

            // Terminal failure. The transaction won't be retried by OpenZeppelin. No reason
            // provided
            if status == "failed" {
                return Err(TxError::Failed(None));
            }

            // Transaction mined successfully
            if status == "mined" || status == "confirmed" {
                return Ok(transaction);
            }

            info!("waiting 5 s to mine");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    async fn mine_transaction_id(&self, id: &str) -> Result<SubmittedTransaction, TxError> {
        timeout(self.mine_timeout, self.mine_transaction_id_unchecked(id))
            .await
            .map_err(|_| TxError::ConfirmationTimeout)?
    }

    async fn send_oz_transaction<T: Into<TypedTransaction> + Send + Sync>(
        &self,
        tx: T,
    ) -> Result<String, Error> {
        let tx: TypedTransaction = tx.into();
        let api_tx = Transaction {
            to:          tx.to(),
            value:       tx.value(),
            gas_limit:   tx.gas(),
            data:        tx.data(),
            valid_until: Some(chrono::Utc::now() + self.transaction_validity),
        };

        let res = self
            .client()
            .await
            .map_err(|_| Error::Authentication)?
            .as_ref()
            .post(RELAY_TXS_URL)
            .body(json!(api_tx).to_string())
            .send()
            .await
            .map_err(|_| Error::RequestFailed)?;

        if res.status() == StatusCode::OK {
            let obj = res
                .json::<Value>()
                .await
                .map_err(|_| Error::UnknownResponseFormat)?;
            let id = obj
                .get("transactionId")
                .ok_or(Error::MissingTransactionId)?
                .as_str()
                .unwrap();
            Ok(id.to_string())
        } else {
            let text = res.text().await;
            info!(?text, "response error");

            Err(Error::UnknownResponseFormat)
        }
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

            let existing_transactions = self.list_recent_transactions().await.map_err(|e| {
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

                let transaction_id = existing_transaction.transaction_id.clone().unwrap();
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

        self.mine_transaction_id(&tx_id).await?;
        Ok(TransactionId(tx_id))
    }

    async fn client(&self) -> Result<MutexGuard<ExpiringClient>, Error> {
        let now = chrono::Utc::now().timestamp();

        let mut client = self.client.lock().await;

        if client.expiration_time < now {
            let new_client = get_client(&self.api_key, &self.api_secret)
                .await
                .map_err(|_| Error::Authentication)?;
            *client = new_client;
        }

        Ok(client)
    }
}

#[derive(Debug, Clone)]
struct ExpiringClient {
    client:          Client,
    expiration_time: i64,
}

impl AsRef<Client> for ExpiringClient {
    fn as_ref(&self) -> &Client {
        &self.client
    }
}

/// Refreshes or creates a new access token for Defender API and returns it.
async fn get_client(api_key: &str, api_secret: &str) -> AnyhowResult<ExpiringClient> {
    let now = chrono::Utc::now().timestamp();

    let input = CognitoAuthInput {
        client_id:     CLIENT_ID.to_string(),
        pool_id:       POOL_ID.to_string(),
        username:      api_key.to_string(),
        password:      api_secret.to_string(),
        mfa:           None,
        client_secret: None,
    };

    let res = auth(input)
        .await
        .context("Auth request failed")?
        .context("Authentication failed")?;

    let access_token = res.access_token().context("Authentication failed")?;

    let mut auth_value = HeaderValue::from_str(&format!("Bearer {access_token}"))?;
    auth_value.set_sensitive(true);

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::AUTHORIZATION, auth_value);
    headers.insert("X-Api-Key", HeaderValue::from_str(api_key)?);

    let client = Client::builder().default_headers(headers).build()?;

    Ok(ExpiringClient {
        client,
        expiration_time: now + i64::from(res.expires_in()),
    })
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("Transport error")]
    Transport(#[from] ethers::providers::HttpClientError),
    #[error("Authentication error")]
    Authentication,
    #[error("Request failed")]
    RequestFailed,
    #[error("Unknown response format")]
    UnknownResponseFormat,
    #[error("Missing transaction id")]
    MissingTransactionId,
}

impl From<Error> for ProviderError {
    fn from(error: Error) -> Self {
        Self::JsonRpcClientError(Box::new(error))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transaction<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to:          Option<&'a NameOrAddress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value:       Option<&'a U256>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_limit:   Option<&'a U256>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data:        Option<&'a Bytes>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmittedTransaction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to:             Option<NameOrAddress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value:          Option<U256>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_limit:      Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data:           Option<Bytes>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_until:    Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status:         Option<String>,
}
