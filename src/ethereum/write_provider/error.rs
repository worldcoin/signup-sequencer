use ethers::providers::{JsonRpcError, ProviderError, RpcError};
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

impl RpcError for Error {
    fn as_error_response(&self) -> Option<&JsonRpcError> {
        match self {
            Error::Transport(err) => err.as_error_response(),
            _ => None,
        }
    }

    fn as_serde_error(&self) -> Option<&serde_json::Error> {
        match self {
            Error::Transport(err) => err.as_serde_error(),
            _ => None,
        }
    }
}

impl From<Error> for ProviderError {
    fn from(error: Error) -> Self {
        Self::JsonRpcClientError(Box::new(error))
    }
}

impl From<oz_api::Error> for Error {
    fn from(value: oz_api::Error) -> Self {
        match value {
            oz_api::Error::AuthFailed(_) | oz_api::Error::Unauthorized => Self::Authentication,
            oz_api::Error::Reqwest(_)
            | oz_api::Error::Headers(_)
            | oz_api::Error::UrlParseError(_)
            | oz_api::Error::InvalidResponse(_) => Self::RequestFailed,
            oz_api::Error::ParseError(_) => Self::UnknownResponseFormat,
        }
    }
}
