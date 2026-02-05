use std::str::FromStr;

use semaphore_rs::poseidon_tree::Proof;
use semaphore_rs::Field;
use serde::{Deserialize, Serialize};

pub enum InclusionProofType {
    Processed,
    Mined,
    Bridged,
}

impl FromStr for InclusionProofType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "processed" => Ok(InclusionProofType::Processed),
            "mined" => Ok(InclusionProofType::Mined),
            "bridged" => Ok(InclusionProofType::Bridged),
            _ => anyhow::bail!("Unknown InclusionProofType"),
        }
    }
}

impl std::fmt::Display for InclusionProofType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            InclusionProofType::Processed => write!(f, "processed"),
            InclusionProofType::Mined => write!(f, "mined"),
            InclusionProofType::Bridged => write!(f, "bridged"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct InclusionProofResponse {
    pub root: Field,
    pub proof: Proof,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct VerifySemaphoreProofRequest {
    pub root: Field,
    pub signal_hash: Field,
    pub nullifier_hash: Field,
    pub external_nullifier_hash: Field,
    pub proof: semaphore_rs::protocol::Proof,
    pub max_root_age_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct VerifySemaphoreProofResponse {
    pub valid: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {
    error_id: String,
    error_message: String,
}

impl ErrorResponse {
    pub fn new<T: Into<String>>(error_id: T, error_message: T) -> Self {
        Self {
            error_id: error_id.into(),
            error_message: error_message.into(),
        }
    }
}
