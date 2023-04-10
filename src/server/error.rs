use crate::database;
use hyper::{Body, StatusCode};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid http method")]
    InvalidMethod,
    #[error("invalid path")]
    InvalidPath,
    #[error("invalid content type")]
    InvalidContentType,
    #[error("invalid group id")]
    InvalidGroupId,
    #[error("invalid root")]
    InvalidRoot,
    #[error("invalid semaphore proof")]
    InvalidProof,
    #[error("provided identity index out of bounds")]
    IndexOutOfBounds,
    #[error("provided identity commitment not found")]
    IdentityCommitmentNotFound,
    #[error("provided identity commitment is invalid")]
    InvalidCommitment,
    #[error("provided identity commitment is not in reduced form")]
    UnreducedCommitment,
    #[error("provided identity commitment is already included")]
    DuplicateCommitment,
    #[error("Root mismatch between tree and contract.")]
    RootMismatch,
    #[error("invalid JSON request: {0}")]
    InvalidSerialization(#[from] serde_json::Error),
    #[error(transparent)]
    Database(#[from] database::Error),
    #[error(transparent)]
    Hyper(#[from] hyper::Error),
    #[error(transparent)]
    Http(#[from] hyper::http::Error),
    #[error("not semaphore manager")]
    NotManager,
    #[error(transparent)]
    Elapsed(#[from] tokio::time::error::Elapsed),
    #[error("prover error")]
    ProverError,
    #[error("Failed to insert identity")]
    FailedToInsert,
    #[error("The provided batch size already exists")]
    BatchSizeAlreadyExists,
    #[error("The requested batch size does not exist")]
    NoSuchBatchSize,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Error {
    #[allow(clippy::enum_glob_use)]
    pub fn to_response(&self) -> hyper::Response<Body> {
        use Error::*;

        let status_code = match self {
            InvalidMethod => StatusCode::METHOD_NOT_ALLOWED,
            InvalidPath => StatusCode::NOT_FOUND,
            InvalidContentType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            IndexOutOfBounds
            | IdentityCommitmentNotFound
            | InvalidCommitment
            | DuplicateCommitment
            | InvalidSerialization(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        hyper::Response::builder()
            .status(status_code)
            .body(hyper::Body::from(self.to_string()))
            .expect("Failed to convert error string into hyper::Body")
    }
}
