use async_trait::async_trait;
use ethers::providers::{Http, Ipc, JsonRpcClient, ProviderError, Ws};
use serde::{de::DeserializeOwned, Serialize};
use std::fmt::Debug;
use thiserror::Error;
use url::Url;

// Todo: Enable IPC or WS based on feature flags

#[derive(Debug, Clone)]
pub enum Transport {
    Http(Http),
    Ws(Ws),
    Ipc(Ipc),
}

#[derive(Debug, Error)]
#[allow(clippy::module_name_repetitions)]
pub enum TransportError {
    #[error("Http error: {0}")]
    Http(<Http as JsonRpcClient>::Error),

    #[error("WebSocket error: {0}")]
    Ws(<Ws as JsonRpcClient>::Error),

    #[error("IPC error: {0}")]
    Ipc(<Ipc as JsonRpcClient>::Error),

    #[error("Unsupported transport: {0}")]
    InvalidScheme(Url),
}

impl Transport {
    pub async fn new(url: Url) -> Result<Self, TransportError> {
        match url.scheme() {
            "http" | "https" => Ok(Self::Http(Http::new(url))),
            "ws" | "wss" => Ok(Self::Ws(
                Ws::connect(url).await.map_err(TransportError::Ws)?,
            )),
            "ipc" if url.host().is_none() => Ok(Self::Ipc(
                Ipc::connect(url.path())
                    .await
                    .map_err(TransportError::Ipc)?,
            )),
            _ => Err(TransportError::InvalidScheme(url)),
        }
    }
}

impl From<TransportError> for ProviderError {
    fn from(error: TransportError) -> Self {
        match error {
            TransportError::Http(error) => Self::from(error),
            TransportError::Ws(error) => Self::from(error),
            TransportError::Ipc(error) => Self::from(error),
            TransportError::InvalidScheme(url) => {
                Self::CustomError(format!("Unsupported transport: {url}"))
            }
        }
    }
}

#[async_trait]
impl JsonRpcClient for Transport {
    type Error = TransportError;

    async fn request<T, R>(&self, method: &str, params: T) -> Result<R, Self::Error>
    where
        T: Debug + Serialize + Send + Sync,
        R: DeserializeOwned,
    {
        match self {
            Self::Http(inner) => inner
                .request(method, params)
                .await
                .map_err(TransportError::Http),
            Self::Ws(inner) => inner
                .request(method, params)
                .await
                .map_err(TransportError::Ws),
            Self::Ipc(inner) => inner
                .request(method, params)
                .await
                .map_err(TransportError::Ipc),
        }
    }
}
