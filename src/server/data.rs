use hyper::StatusCode;
use semaphore::protocol::Proof;
use semaphore::Field;
use serde::{Deserialize, Serialize};

use crate::identity_tree::{
    Hash, InclusionProof, ProcessedStatus, RootItem, Status, UnprocessedStatus,
};
use crate::prover::{ProverConfig, ProverType};

#[derive(Serialize)]
#[serde(transparent)]
pub struct InclusionProofResponse(pub InclusionProof);

#[derive(Serialize)]
#[serde(transparent)]
pub struct ListBatchSizesResponse(pub Vec<ProverConfig>);

#[derive(Serialize)]
#[serde(transparent)]
pub struct VerifySemaphoreProofResponse(pub RootItem);

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct IdentityHistoryResponse {
    pub history: Vec<IdentityHistoryEntry>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct IdentityHistoryEntry {
    pub kind:   IdentityHistoryEntryKind,
    pub status: IdentityHistoryEntryStatus,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub enum IdentityHistoryEntryKind {
    Insertion,
    Deletion,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub enum IdentityHistoryEntryStatus {
    // Present in the unprocessed identities or deletions table
    Buffered,
    // Present in the unprocessed identities table but not eligible for processing
    Queued,
    // Present in the pending tree (not mined on chain yet)
    Pending,
    // Present in the batching tree (transaction sent but not confirmed yet)
    Batched,
    // Present in the processed tree (mined on chain)
    Mined,
    // Present in the batching tree (mined on chain)
    Bridged,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct InsertCommitmentRequest {
    pub identity_commitment: Hash,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct AddBatchSizeRequest {
    /// The URL of the prover for the provided batch size.
    pub url:             String,
    /// The batch size to add.
    pub batch_size:      usize,
    /// The timeout for communications with the prover service.
    pub timeout_seconds: u64,
    // TODO: add docs
    pub prover_type:     ProverType,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct RemoveBatchSizeRequest {
    /// The batch size to remove from the prover map.
    pub batch_size:  usize,
    // TODO: add docs
    pub prover_type: ProverType,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct InclusionProofRequest {
    pub identity_commitment: Hash,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct IdentityHistoryRequest {
    pub identity_commitment: Hash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct VerifySemaphoreProofRequest {
    pub root:                    Field,
    pub signal_hash:             Field,
    pub nullifier_hash:          Field,
    pub external_nullifier_hash: Field,
    pub proof:                   Proof,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct VerifySemaphoreProofQuery {
    #[serde(default)]
    pub max_root_age_seconds: Option<i64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct DeletionRequest {
    /// The identity commitment to delete.
    pub identity_commitment: Hash,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct RecoveryRequest {
    /// The leaf index of the identity commitment to delete.
    pub previous_identity_commitment: Hash,
    /// The new identity commitment to insert.
    pub new_identity_commitment:      Hash,
}

impl InclusionProofResponse {
    #[must_use]
    pub fn hide_processed_status(mut self) -> Self {
        self.0.status = if self.0.status == Status::Processed(ProcessedStatus::Processed) {
            Status::Processed(ProcessedStatus::Pending)
        } else {
            self.0.status
        };

        self
    }
}

impl From<InclusionProof> for InclusionProofResponse {
    fn from(value: InclusionProof) -> Self {
        Self(value)
    }
}

impl ToResponseCode for InclusionProofResponse {
    fn to_response_code(&self) -> StatusCode {
        match self.0.status {
            Status::Unprocessed(UnprocessedStatus::New)
            | Status::Processed(ProcessedStatus::Pending) => StatusCode::ACCEPTED,
            Status::Processed(ProcessedStatus::Mined | ProcessedStatus::Processed) => {
                StatusCode::OK
            }
        }
    }
}

impl From<Vec<ProverConfig>> for ListBatchSizesResponse {
    fn from(value: Vec<ProverConfig>) -> Self {
        Self(value)
    }
}

impl ToResponseCode for ListBatchSizesResponse {
    fn to_response_code(&self) -> StatusCode {
        StatusCode::OK
    }
}

impl VerifySemaphoreProofResponse {
    #[must_use]
    pub fn hide_processed_status(mut self) -> Self {
        self.0.status = if self.0.status == ProcessedStatus::Processed {
            ProcessedStatus::Pending
        } else {
            self.0.status
        };

        self
    }
}

impl ToResponseCode for VerifySemaphoreProofResponse {
    fn to_response_code(&self) -> StatusCode {
        StatusCode::OK
    }
}

impl IdentityHistoryEntryKind {
    #[must_use]
    pub fn is_insertion(&self) -> bool {
        matches!(self, IdentityHistoryEntryKind::Insertion)
    }

    #[must_use]
    pub fn is_deletion(&self) -> bool {
        matches!(self, IdentityHistoryEntryKind::Deletion)
    }
}

pub trait ToResponseCode {
    fn to_response_code(&self) -> StatusCode;
}

impl ToResponseCode for () {
    fn to_response_code(&self) -> StatusCode {
        StatusCode::OK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_history_entry_status_ordering() {
        let expected = vec![
            IdentityHistoryEntryStatus::Buffered,
            IdentityHistoryEntryStatus::Queued,
            IdentityHistoryEntryStatus::Pending,
            IdentityHistoryEntryStatus::Batched,
            IdentityHistoryEntryStatus::Mined,
            IdentityHistoryEntryStatus::Bridged,
        ];

        let mut statuses = expected.clone();

        statuses.sort();

        assert_eq!(expected, statuses);
    }
}
