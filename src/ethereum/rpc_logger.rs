use ::prometheus::{register_histogram, register_int_counter_vec, Histogram, IntCounterVec};
use async_trait::async_trait;
use ethers::providers::{JsonRpcClient, PubsubClient};
use once_cell::sync::Lazy;
use serde::{de::DeserializeOwned, Serialize};
use std::fmt::Debug;
use tracing::instrument;

static REQUESTS: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "eth_rpc_requests",
        "Number of Ethereum provider requests made by method.",
        &["method"]
    )
    .unwrap()
});
static LATENCY: Lazy<Histogram> = Lazy::new(|| {
    register_histogram!(
        "eth_rpc_latency_seconds",
        "The Ethereum provider latency in seconds."
    )
    .unwrap()
});

#[derive(Debug, Clone)]
pub struct RpcLogger<Inner> {
    inner: Inner,
}

impl<Inner> RpcLogger<Inner> {
    pub const fn new(inner: Inner) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl<Inner> JsonRpcClient for RpcLogger<Inner>
where
    Inner: JsonRpcClient + 'static,
    <Inner as JsonRpcClient>::Error: Sync + Send + 'static,
{
    type Error = Inner::Error;

    #[instrument(name = "eth_rpc", level = "debug", skip(self))]
    async fn request<T, R>(&self, method: &str, params: T) -> Result<R, Self::Error>
    where
        T: Debug + Serialize + Send + Sync,
        R: DeserializeOwned,
    {
        REQUESTS.with_label_values(&[method]).inc();
        let timer = LATENCY.start_timer();
        let result = self.inner.request(method, params).await;
        timer.observe_duration();
        result
    }
}

impl<Inner> PubsubClient for RpcLogger<Inner>
where
    Inner: PubsubClient + 'static,
    <Inner as JsonRpcClient>::Error: Sync + Send + 'static,
{
    type NotificationStream = Inner::NotificationStream;

    fn subscribe<T: Into<ethers::types::U256>>(&self, id: T) -> Result<Self::NotificationStream, Self::Error> {
        self.inner.subscribe(id)
    }

    fn unsubscribe<T: Into<ethers::types::U256>>(&self, id: T) -> Result<(), Self::Error> {
        self.inner.unsubscribe(id)
    }
}