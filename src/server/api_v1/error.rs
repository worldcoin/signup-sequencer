use crate::database;
use anyhow::Error as EyreError;
use axum::http::StatusCode;
use axum::response::IntoResponse;
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
    #[error("provided identity commitment was deleted")]
    DeletedCommitment,
    #[error("Root mismatch between tree and contract.")]
    RootMismatch,
    #[error("Root provided in semaphore proof is too old.")]
    RootTooOld,
    #[error("Identity is already queued for deletion.")]
    IdentityQueuedForDeletion,
    #[error("Identity has already been deleted.")]
    IdentityAlreadyDeleted,
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
    #[error("The last batch size cannot be removed")]
    CannotRemoveLastBatchSize,
    #[error("Identity Manager had no provers on point of identity insertion.")]
    NoProversOnIdInsert,
    #[error("Identity Manager had no provers on point of identity deletion.")]
    NoProversOnIdDeletion,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error("The tree is uninitialized. Try again in a few moments.")]
    TreeStateUninitialized,
    #[error(transparent)]
    Other(#[from] EyreError),
}

impl Error {
    fn to_status_code(&self) -> StatusCode {
        match self {
            Self::InvalidMethod => StatusCode::METHOD_NOT_ALLOWED,
            Self::InvalidPath | Self::IdentityCommitmentNotFound => StatusCode::NOT_FOUND,
            Self::InvalidContentType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::IndexOutOfBounds | Self::InvalidCommitment | Self::InvalidSerialization(_) => {
                StatusCode::BAD_REQUEST
            }
            Self::IdentityAlreadyDeleted
            | Self::IdentityQueuedForDeletion
            | Self::DuplicateCommitment => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        let status_code = self.to_status_code();

        let body = if let Self::Other(err) = self {
            format!("{err}")
        } else {
            self.to_string()
        };

        (status_code, body).into_response()
    }
}
