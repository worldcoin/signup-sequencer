use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;

use crate::identity_tree::Hash;
use crate::prover::identity::Identity;

pub struct LatestInsertionEntry {
    pub timestamp: DateTime<Utc>,
}

pub struct LatestDeletionEntry {
    pub timestamp: DateTime<Utc>,
}

#[derive(Hash, PartialEq, Eq, FromRow)]
pub struct DeletionEntry {
    #[sqlx(try_from = "i64")]
    pub leaf_index: usize,
    pub commitment: Hash,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Hash, PartialEq, Eq, FromRow)]
pub struct UnprocessedIdentityEntry {
    pub commitment: Hash,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Copy, Clone, sqlx::Type, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[sqlx(type_name = "VARCHAR", rename_all = "PascalCase")]
pub enum BatchType {
    #[default]
    Insertion,
    Deletion,
}

impl From<String> for BatchType {
    fn from(s: String) -> Self {
        match s.as_str() {
            "Insertion" => BatchType::Insertion,
            "Deletion" => BatchType::Deletion,
            _ => BatchType::Insertion,
        }
    }
}

impl std::fmt::Display for BatchType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            BatchType::Insertion => write!(f, "insertion"),
            BatchType::Deletion => write!(f, "deletion"),
        }
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct BatchEntry {
    pub id: i64,
    pub next_root: Hash,
    // In general prev_root is present all the time except the first row (head of the batches
    // chain)
    pub prev_root: Option<Hash>,
    pub created_at: DateTime<Utc>,
    pub batch_type: BatchType,
    pub data: sqlx::types::Json<BatchEntryData>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchEntryData {
    pub identities: Vec<Identity>,
    pub indexes: Vec<usize>,
}
