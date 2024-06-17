use chrono::Utc;
use hyper::StatusCode;
use semaphore::protocol::Proof;
use semaphore::Field;
use serde::{Deserialize, Serialize};

use crate::identity_tree::{Hash, InclusionProof, RootItem};
use crate::prover::{ProverConfig, ProverType};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct InclusionProofResponse {
    pub root:    Option<Field>,
    pub proof:   Option<semaphore::poseidon_tree::Proof>,
    pub message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListBatchSizesResponse(pub Vec<ListBatchSizesResponseEntry>);

#[derive(Debug, Serialize, Deserialize)]
pub struct ListBatchSizesResponseEntry {
    pub url:         String,
    pub timeout_s:   u64,
    pub batch_size:  usize,
    pub prover_type: ProverType,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifySemaphoreProofResponse {
    pub root:                Field,
    pub pending_valid_as_of: chrono::DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct InclusionProofRequest {
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct DeletionRequest {
    /// The identity commitment to delete.
    pub identity_commitment: Hash,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct RecoveryRequest {
    /// The leaf index of the identity commitment to delete.
    pub previous_identity_commitment: Hash,
    /// The new identity commitment to insert.
    pub new_identity_commitment:      Hash,
}

impl From<InclusionProof> for InclusionProofResponse {
    fn from(value: InclusionProof) -> Self {
        Self {
            root:    value.root,
            proof:   value.proof,
            message: value.message,
        }
    }
}

impl ToResponseCode for InclusionProofResponse {
    fn to_response_code(&self) -> StatusCode {
        StatusCode::OK
    }
}

impl From<Vec<ProverConfig>> for ListBatchSizesResponse {
    fn from(value: Vec<ProverConfig>) -> Self {
        Self(
            value
                .into_iter()
                .map(|v| ListBatchSizesResponseEntry {
                    url:         v.url,
                    timeout_s:   v.timeout_s,
                    batch_size:  v.batch_size,
                    prover_type: v.prover_type,
                })
                .collect(),
        )
    }
}

impl ToResponseCode for ListBatchSizesResponse {
    fn to_response_code(&self) -> StatusCode {
        StatusCode::OK
    }
}

impl From<RootItem> for VerifySemaphoreProofResponse {
    fn from(value: RootItem) -> Self {
        Self {
            root:                value.root,
            pending_valid_as_of: value.pending_valid_as_of,
        }
    }
}

impl ToResponseCode for VerifySemaphoreProofResponse {
    fn to_response_code(&self) -> StatusCode {
        StatusCode::OK
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
