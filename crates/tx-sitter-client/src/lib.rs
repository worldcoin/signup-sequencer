use std::fmt;

use crate::data::ErrorResponseBody;
use anyhow::bail;
use data::{GetTxResponse, SendTxRequest, SendTxResponse, TxStatus};
use reqwest::header::HeaderMap;
use reqwest::{RequestBuilder, Response, StatusCode};
use telemetry_batteries::tracing::trace_to_headers;
use tracing::instrument;

pub mod data;

pub struct TxSitterClient {
    client: reqwest::Client,
    url: String,
}

#[derive(Debug)]
pub struct HttpError {
    pub status: StatusCode,
    pub body: String,
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Response failed with status {} - {}",
            self.status, self.body
        )
    }
}

#[derive(Debug, Clone)]
pub struct ErrorResponse {
    pub status: StatusCode,
    pub body: ErrorResponseBody,
}

impl fmt::Display for ErrorResponse {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Response failed with status {} - got ErrorResponse object (error_key={}, error_message={})",
            self.status, self.body.error_id, self.body.error_message
        )
    }
}

impl TxSitterClient {
    pub fn new(url: impl ToString) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: url.to_string(),
        }
    }

    fn inject_tracing_headers(req_builder: RequestBuilder) -> RequestBuilder {
        let mut headers = HeaderMap::new();

        trace_to_headers(&mut headers);

        req_builder.headers(headers)
    }

    async fn json_post<T, R>(&self, url: &str, body: T) -> anyhow::Result<R>
    where
        T: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        let response = Self::inject_tracing_headers(self.client.post(url))
            .json(&body)
            .send()
            .await?;

        let response = Self::validate_response(response).await?;

        Ok(response.json().await?)
    }

    async fn json_get<R>(&self, url: &str) -> anyhow::Result<R>
    where
        R: serde::de::DeserializeOwned,
    {
        let response = Self::inject_tracing_headers(self.client.get(url))
            .send()
            .await?;

        let response = Self::validate_response(response).await?;

        Ok(response.json().await?)
    }

    async fn validate_response(response: Response) -> anyhow::Result<Response> {
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;

            if let Ok(error_response) = serde_json::from_str::<ErrorResponseBody>(&body) {
                let error = ErrorResponse {
                    status,
                    body: error_response,
                };
                tracing::error!("Error: {}", error);
                bail!(error);
            }

            let error = HttpError { body, status };
            tracing::error!("Error: {}", error);
            bail!(error);
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
        self.json_get(&format!("{}/txs", self.url)).await
    }

    #[instrument(skip(self))]
    pub async fn get_txs_by_status(
        &self,
        tx_status: TxStatus,
    ) -> anyhow::Result<Vec<GetTxResponse>> {
        self.json_get(&format!("{}/txs?status={}", self.url, tx_status))
            .await
    }

    #[instrument(skip(self))]
    pub async fn get_unsent_txs(&self) -> anyhow::Result<Vec<GetTxResponse>> {
        self.json_get(&format!("{}/txs?unsent=true", self.url))
            .await
    }

    pub fn rpc_url(&self) -> String {
        format!("{}/rpc", self.url.clone())
    }
}
