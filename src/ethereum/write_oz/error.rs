use ethers::providers::ProviderError;
use thiserror::Error;

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
