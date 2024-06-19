use data::{GetTxResponse, SendTxRequest, SendTxResponse, TxStatus};
use reqwest::Response;
use tracing::instrument;

pub mod data;

pub struct TxSitterClient {
    client: reqwest::Client,
    url:    String,
}

impl TxSitterClient {
    pub fn new(url: impl ToString) -> Self {
        Self {
            client: reqwest::Client::new(),
            url:    url.to_string(),
        }
    }

    async fn json_post<T, R>(&self, url: &str, body: T) -> anyhow::Result<R>
    where
        T: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        let response = self.client.post(url).json(&body).send().await?;

        let response = Self::validate_response(response).await?;

        Ok(response.json().await?)
    }

    async fn json_get<R>(&self, url: &str) -> anyhow::Result<R>
    where
        R: serde::de::DeserializeOwned,
    {
        let response = self.client.get(url).send().await?;

        let response = Self::validate_response(response).await?;

        Ok(response.json().await?)
    }

    async fn validate_response(response: Response) -> anyhow::Result<Response> {
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;

            tracing::error!("Response failed with status {} - {}", status, body);
            return Err(anyhow::anyhow!(
                "Response failed with status {status} - {body}"
            ));
        }

        Ok(response)
    }

    #[instrument(skip(self))]
    pub async fn send_tx(&self, req: &SendTxRequest) -> anyhow::Result<SendTxResponse> {
        self.json_post(&format!("{}/tx", self.url), req).await
    }

    #[instrument(skip(self))]
    pub async fn get_tx(&self, tx_id: &str) -> anyhow::Result<GetTxResponse> {
        self.json_get(&format!("{}/tx/{}", self.url, tx_id)).await
    }

    #[instrument(skip(self))]
    pub async fn get_txs(&self) -> anyhow::Result<Vec<GetTxResponse>> {
        let url = format!("{}/txs", self.url);

        self.json_get(&url).await
    }

    #[instrument(skip(self))]
    pub async fn get_txs_by_status(
        &self,
        tx_status: TxStatus,
    ) -> anyhow::Result<Vec<GetTxResponse>> {
        let url = format!("{}/txs?status={}", self.url, tx_status);

        self.json_get(&url).await
    }

    #[instrument(skip(self))]
    pub async fn get_unsent_txs(&self) -> anyhow::Result<Vec<GetTxResponse>> {
        let url = format!("{}/txs?unsent=true", self.url);

        self.json_get(&url).await
    }

    pub fn rpc_url(&self) -> String {
        format!("{}/rpc", self.url.clone())
    }
}
