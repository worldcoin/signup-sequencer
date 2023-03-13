use auth::ExpiringHeaders;
use data::transactions::{RelayerTransactionBase, SendBaseTransactionRequest, Status};
use reqwest::{IntoUrl, Url};
use tokio::sync::{Mutex, MutexGuard};

mod auth;
pub mod data;
pub mod error;

pub type Result<T> = std::result::Result<T, error::Error>;

#[derive(Debug)]
pub struct OzApi {
    client:           reqwest::Client,
    api_url:          Url,
    expiring_headers: Mutex<ExpiringHeaders>,
    api_key:          String,
    api_secret:       String,
}

impl OzApi {
    pub async fn new<U, S>(api_url: U, api_key: S, api_secret: S) -> Result<Self>
    where
        U: IntoUrl,
        S: ToString,
    {
        let api_key = api_key.to_string();
        let api_secret = api_secret.to_string();

        let expiring_headers = ExpiringHeaders::refresh(&api_key, &api_secret).await?;
        let expiring_headers = Mutex::new(expiring_headers);

        Ok(Self {
            client: reqwest::Client::new(),
            expiring_headers,
            api_url: api_url.into_url()?,
            api_key,
            api_secret,
        })
    }

    pub async fn send_transaction(
        &self,
        tx: SendBaseTransactionRequest<'_>,
    ) -> Result<RelayerTransactionBase> {
        let headers = self.headers().await?;

        let res = headers
            .apply(self.client.post(self.txs_url()))
            .json(&tx)
            .send()
            .await?
            .json()
            .await?;

        Ok(res)
    }

    pub async fn list_transactions(
        &self,
        status: Option<Status>,
        limit: Option<usize>,
    ) -> Result<Vec<RelayerTransactionBase>> {
        let mut url = self.txs_url();

        let mut query_items = vec![];

        if let Some(status) = status {
            query_items.push(format!("status={status}"));
        }

        if let Some(limit) = limit {
            query_items.push(format!("limit={limit}"));
        }

        if !query_items.is_empty() {
            url.set_query(Some(&query_items.join("&")));
        }

        let headers = self.headers().await?;

        let res = headers
            .apply(self.client.get(url))
            .send()
            .await?
            .json()
            .await?;

        Ok(res)
    }

    pub async fn query_transaction(&self, tx_id: &str) -> Result<RelayerTransactionBase> {
        let url = self.txs_url().join("txs/")?.join(tx_id)?;

        let headers = self.headers().await?;

        let res = headers.apply(self.client.get(url)).send().await?;

        let intermediate: serde_json::Value = res.json().await?;

        let concrete = serde_json::from_value(intermediate)?;

        Ok(concrete)
    }

    fn txs_url(&self) -> Url {
        self.api_url.join("txs").unwrap()
    }

    async fn headers(&self) -> Result<MutexGuard<ExpiringHeaders>> {
        let now = chrono::Utc::now().timestamp();

        let mut expiring_headers = self.expiring_headers.lock().await;

        if expiring_headers.expiration_time < now {
            let new_headers = ExpiringHeaders::refresh(&self.api_key, &self.api_secret).await?;

            *expiring_headers = new_headers;
        }

        Ok(expiring_headers)
    }
}
