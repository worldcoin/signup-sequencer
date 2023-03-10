use std::str::FromStr;

use data::transactions::{RelayerTransactionBase, SendBaseTransactionRequest, Status};
use reqwest::{IntoUrl, Url};
use typed_request_builder::TypedRequestBuilder;

pub mod data;
pub mod typed_request_builder;

#[derive(Debug)]
pub struct OzApi {
    client:  reqwest::Client,
    api_url: Url,
}

impl OzApi {
    pub fn new<U>(api_url: U) -> reqwest::Result<Self>
    where
        U: IntoUrl,
    {
        Ok(Self {
            client:  reqwest::Client::new(),
            api_url: api_url.into_url()?,
        })
    }

    pub fn send_transaction(
        &self,
        tx: SendBaseTransactionRequest,
    ) -> TypedRequestBuilder<RelayerTransactionBase> {
        self.client
            .post(format!("{}/txs", self.api_url))
            .json(&tx)
            .into()
    }

    pub fn list_transactions(
        &self,
        status: Option<Status>,
        limit: Option<usize>,
    ) -> TypedRequestBuilder<Vec<RelayerTransactionBase>> {
        let url = format!("{}/txs", self.api_url);
        let mut url = Url::from_str(&url).unwrap();

        let query = [opt_to_string(status), opt_to_string(limit)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let query = if query.is_empty() {
            None
        } else {
            Some(query.join("&"))
        };

        url.set_query(query.as_deref());

        self.client.get(url).into()
    }

    pub fn query_transaction(&self, tx_id: &str) -> TypedRequestBuilder<RelayerTransactionBase> {
        self.client
            .get(format!("{}/txs/{}", self.api_url, tx_id))
            .into()
    }
}

fn opt_to_string<S: ToString>(opt: Option<S>) -> Option<String> {
    opt.map(|s| s.to_string())
}
