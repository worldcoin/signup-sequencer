use crate::app::App;
use crate::identity_tree::{Hash, InclusionProof, ProcessedStatus, RootItem, Status};
use crate::prover::{ProverConfig, ProverType};
use chrono::Utc;
use hyper::StatusCode;
use semaphore_rs::protocol::compression::CompressedProof;
use semaphore_rs::protocol::Proof;
use semaphore_rs::Field;
use serde::{Deserialize, Serialize};

use crate::server::api_v1::error::Error;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct InclusionProofResponse {
    pub status: Status,
    pub root: Option<Field>,
    pub proof: Option<semaphore_rs::poseidon_tree::Proof>,
    pub message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListBatchSizesResponse(pub Vec<ProverConfig>);

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifySemaphoreProofResponse {
    pub root: Field,
    pub status: ProcessedStatus,
    pub pending_valid_as_of: chrono::DateTime<Utc>,
    pub mined_valid_as_of: Option<chrono::DateTime<Utc>>,
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
pub struct VerifyCompressedSemaphoreProofRequest {
    pub root: Field,
    pub signal_hash: Field,
    pub nullifier_hash: Field,
    pub external_nullifier_hash: Field,
    pub proof: CompressedProof,
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

impl VerifySemaphoreProofRequest {
    pub fn is_proof_padded(&self) -> bool {
        App::is_proof_padded(&self.proof)
    }
}

impl VerifyCompressedSemaphoreProofRequest {
    pub fn decompress(self) -> Result<VerifySemaphoreProofRequest, Error> {
        let Self {
            root,
            signal_hash,
            nullifier_hash,
            external_nullifier_hash,
            proof,
        } = self;

        let proof = semaphore_rs::protocol::compression::decompress_proof(proof)
            .ok_or_else(|| Error::InvalidProof)?;

        Ok(VerifySemaphoreProofRequest {
            root,
            signal_hash,
            nullifier_hash,
            external_nullifier_hash,
            proof,
        })
    }
}

impl From<InclusionProof> for InclusionProofResponse {
    fn from(value: InclusionProof) -> Self {
        Self {
            status: match value.status {
                Status::Processed(ProcessedStatus::Processed) => {
                    Status::Processed(ProcessedStatus::Pending)
                }
                v => v,
            },
            root: value.root,
            proof: value.proof,
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
        Self {
            root: value.root,
            status: match value.status {
                ProcessedStatus::Processed => ProcessedStatus::Pending,
                v => v,
            },
            pending_valid_as_of: value.pending_valid_as_of,
            mined_valid_as_of: value.mined_valid_as_of,
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
