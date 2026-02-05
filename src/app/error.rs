use thiserror::Error;

use crate::database;

#[derive(Debug, Error)]
pub enum InsertIdentityV3Error {
    #[error("provided identity commitment is invalid")]
    InvalidCommitment,
    #[error("provided identity commitment is not in reduced form")]
    UnreducedCommitment,
    #[error("provided identity commitment is already included")]
    AlreadyIncludedCommitment,
    #[error("provided identity commitment was already added for insertion")]
    AddedForInsertionCommitment,
    #[error("provided identity commitment was already added for deletion")]
    AddedForDeletionCommitment,
    #[error(transparent)]
    Database(#[from] database::Error),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

#[derive(Debug, Error)]
pub enum InsertIdentityV2Error {
    #[error("provided identity commitment is invalid")]
    InvalidCommitment,
    #[error("provided identity commitment is not in reduced form")]
    UnreducedCommitment,
    #[error("provided identity commitment is already included")]
    DuplicateCommitment,
    #[error("provided identity commitment was deleted")]
    DeletedCommitment,
    #[error(transparent)]
    Database(#[from] database::Error),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

#[derive(Debug, Error)]
pub enum DeleteIdentityV2Error {
    #[error("provided identity commitment is invalid")]
    InvalidCommitment,
    #[error("provided identity commitment is not in reduced form")]
    UnreducedCommitment,
    #[error("provided identity commitment is not yet added to the tree")]
    UnprocessedCommitment,
    #[error("provided identity commitment was not found")]
    CommitmentNotFound,
    #[error("provided identity commitment was deleted")]
    DeletedCommitment,
    #[error("provided identity commitment already scheduled for deletion")]
    DuplicateCommitmentDeletion,
    #[error(transparent)]
    Database(#[from] database::Error),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

#[derive(Debug, Error)]
pub enum InclusionProofV2Error {
    #[error("provided identity commitment is invalid")]
    InvalidCommitment,
    #[error("provided identity commitment is not yet added to the tree")]
    UnprocessedCommitment,
    #[error("provided identity commitment is not in reduced form")]
    UnreducedCommitment,
    #[error("provided identity commitment was not found")]
    CommitmentNotFound,
    #[error("provided identity commitment was deleted")]
    DeletedCommitment,
    #[error("invalid internal state")]
    InvalidInternalState,
    #[error(transparent)]
    Database(#[from] database::Error),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    AnyhowError(#[from] anyhow::Error),
}

#[derive(Debug, Error)]
pub enum InclusionProofV3Error {
    #[error("provided identity commitment is invalid")]
    InvalidCommitment,
    #[error("provided identity commitment is not in reduced form")]
    UnreducedCommitment,
    #[error("provided identity commitment was not found in the tree")]
    CommitmentNotFound,
    #[error("invalid internal state")]
    InvalidInternalState,
    #[error(transparent)]
    Database(#[from] database::Error),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    AnyhowError(#[from] anyhow::Error),
}

#[derive(Debug, Error)]
pub enum VerifySemaphoreProofV2Error {
    #[error("provided root is invalid")]
    InvalidRoot,
    #[error("provided root is too old")]
    RootTooOld,
    #[error("cannot decompress provided proof")]
    DecompressingProofError,
    #[error("prover error")]
    ProverError,
    #[error("root age checking error")]
    RootAgeCheckingError(anyhow::Error),
    #[error(transparent)]
    Database(#[from] database::Error),
}
