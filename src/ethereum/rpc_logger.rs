use async_trait::async_trait;
use ethers::providers::JsonRpcClient;
use serde::{de::DeserializeOwned, Serialize};
use tracing::instrument;

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

    #[instrument(skip(self))]
    async fn request<T: Serialize + Send + Sync, R: DeserializeOwned>(
        &self,
        method: &str,
        params: T,
    ) -> Result<R, Self::Error>
    where
        T: std::fmt::Debug + Serialize + Send + Sync,
        R: DeserializeOwned,
    {
        self.inner.request(method, params).await
    }
}
