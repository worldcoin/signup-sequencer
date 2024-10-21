use hyper::StatusCode;
use semaphore::protocol::Proof;
use semaphore::Field;
use serde::{Deserialize, Serialize};

use crate::identity_tree::{Hash, InclusionProof, RootItem};
use crate::prover::{ProverConfig, ProverType};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct InclusionProofResponse(pub InclusionProof);

#[derive(Debug, Serialize, Deserialize)]
pub struct ListBatchSizesResponse(pub Vec<ProverConfig>);

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifySemaphoreProofResponse(pub RootItem);

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
    pub url: String,
    /// The batch size to add.
    pub batch_size: usize,
    /// The timeout for communications with the prover service.
    pub timeout_seconds: u64,
    // TODO: add docs
    pub prover_type: ProverType,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct RemoveBatchSizeRequest {
    /// The batch size to remove from the prover map.
    pub batch_size: usize,
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
    pub root: Field,
    pub signal_hash: Field,
    pub nullifier_hash: Field,
    pub external_nullifier_hash: Field,
    pub proof: Proof,
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

impl From<InclusionProof> for InclusionProofResponse {
    fn from(value: InclusionProof) -> Self {
        Self(value)
    }
}

impl ToResponseCode for InclusionProofResponse {
    fn to_response_code(&self) -> StatusCode {
        StatusCode::OK
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

impl From<RootItem> for VerifySemaphoreProofResponse {
    fn from(value: RootItem) -> Self {
        Self(value)
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
