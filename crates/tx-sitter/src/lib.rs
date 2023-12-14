use data::{GetTxResponse, SendTxRequest, SendTxResponse, TxStatus};
use reqwest::Response;

pub mod data;

pub struct TxSitterClient {
    client:  reqwest::Client,
    url:     String,
    api_key: String,
}

impl TxSitterClient {
    pub fn new(url: impl ToString, api_key: impl ToString) -> Self {
        Self {
            client:  reqwest::Client::new(),
            url:     url.to_string(),
            api_key: api_key.to_string(),
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
            let body = response.text().await?;

            return Err(anyhow::anyhow!("{body}"));
        }

        Ok(response)
    }

    pub async fn send_tx(&self, req: &SendTxRequest) -> anyhow::Result<SendTxResponse> {
        self.json_post(&format!("{}/1/api/{}/tx", self.url, self.api_key), req)
            .await
    }

    pub async fn get_tx(&self, tx_id: &str) -> anyhow::Result<GetTxResponse> {
        self.json_get(&format!("{}/1/api/{}/tx/{}", self.url, self.api_key, tx_id))
            .await
    }

    pub async fn get_txs(&self, tx_status: Option<TxStatus>) -> anyhow::Result<Vec<GetTxResponse>> {
        let mut url = format!("{}/1/api/{}/txs", self.url, self.api_key);

        if let Some(tx_status) = tx_status {
            url.push_str(&format!("?status={}", tx_status));
        }

        self.json_get(&url).await
    }
}
