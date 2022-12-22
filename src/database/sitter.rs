use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("a transaction with this id has already been received")]
    DuplicateTransactionId,
}

impl super::Database {}
