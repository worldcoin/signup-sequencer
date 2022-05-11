use ::prometheus::{register_histogram, register_int_counter_vec, Histogram, IntCounterVec};
use async_trait::async_trait;
use ethers::providers::JsonRpcClient;
use once_cell::sync::Lazy;
use serde::{de::DeserializeOwned, Serialize};
use tracing::instrument;

static REQUESTS: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "eth_requests",
        "Number of Ethereum provider requests made by function.",
        &["status_code"]
    )
    .unwrap()
});
static LATENCY: Lazy<Histogram> = Lazy::new(|| {
    register_histogram!(
        "eth_latency_seconds",
        "The Ethereum provider latency in seconds."
    )
    .unwrap()
});

#[derive(Debug, Clone)]
pub struct RpcLogger<Inner> {
    inner: Inner,
}

impl<Inner> RpcLogger<Inner> {
    pub fn new(inner: Inner) -> Self {
        Self { inner }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<Inner> JsonRpcClient for RpcLogger<Inner>
where
    Inner: JsonRpcClient + 'static,
    <Inner as JsonRpcClient>::Error: Sync + Send + 'static,
{
    type Error = Inner::Error;

    #[instrument(level = "trace", skip(self))]
    async fn request<T: Serialize + Send + Sync, R: DeserializeOwned>(
        &self,
        method: &str,
        params: T,
    ) -> Result<R, Self::Error>
    where
        T: std::fmt::Debug + Serialize + Send + Sync,
        R: DeserializeOwned,
    {
        REQUESTS.with_label_values(&[method]).inc();
        let timer = LATENCY.start_timer();
        let result = self.inner.request(method, params).await;
        timer.observe_duration();
        result
    }
}
