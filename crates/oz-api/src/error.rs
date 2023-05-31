use cognitoauth::error::CognitoSrpAuthError;
use hyper::header::InvalidHeaderValue;
use hyper::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Authentication failed: {0}")]
    AuthFailed(#[from] CognitoSrpAuthError),

    #[error("Unauthorized")]
    Unauthorized,

    #[error("Request failed: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("Invalid header value: {0}")]
    Headers(#[from] InvalidHeaderValue),

    #[error("Invalid URL: {0}")]
    UrlParseError(#[from] url::ParseError),

    #[error("Parsing error: {0}")]
    ParseError(#[from] serde_json::Error),

    #[error("Invalid response with status code: {0}")]
    InvalidResponse(StatusCode),
}
