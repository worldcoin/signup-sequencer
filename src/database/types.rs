use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Decode, Encode, Postgres, Type};
use sqlx::database::{HasArguments, HasValueRef};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::prelude::FromRow;

use crate::identity_tree::{Hash, Status, UnprocessedStatus};

pub struct UnprocessedCommitment {
    pub commitment:            Hash,
    pub status:                UnprocessedStatus,
    pub created_at:            DateTime<Utc>,
    pub processed_at:          Option<DateTime<Utc>>,
    pub error_message:         Option<String>,
    pub eligibility_timestamp: DateTime<Utc>,
}

#[derive(FromRow)]
pub struct RecoveryEntry {
    pub existing_commitment: Hash,
    pub new_commitment:      Hash,
}

pub struct LatestDeletionEntry {
    pub timestamp: DateTime<Utc>,
}

#[derive(Hash, PartialEq, Eq)]
pub struct DeletionEntry {
    pub leaf_index: usize,
    pub commitment: Hash,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CommitmentHistoryEntry {
    pub commitment: Hash,
    pub leaf_index: Option<usize>,
    // Only applies to buffered entries
    // set to true if the eligibility timestamp is in the future
    pub held_back:  bool,
    pub status:     Status,
}

#[derive(Debug, Copy, Clone, sqlx::Type, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[sqlx(type_name = "batch_type_enum", rename_all = "PascalCase")]
pub enum BatchType {
    #[default]
    Insertion,
    Deletion,
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
    pub next_root:   Hash,
    // In general prev_root is present all the time except the first row (head of the batches
    // chain)
    pub prev_root:   Option<Hash>,
    pub created_at:  DateTime<Utc>,
    pub batch_type:  BatchType,
    pub commitments: Commitments,
}

#[derive(Debug, Clone, FromRow)]
pub struct TransactionEntry {
    pub batch_next_root: Hash,
    pub transaction_id:  String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commitments(pub Vec<Hash>);

impl Encode<'_, Postgres> for Commitments {
    fn encode_by_ref(&self, buf: &mut <Postgres as HasArguments<'_>>::ArgumentBuffer) -> IsNull {
        let commitments = &self
            .0
            .iter()
            .map(|c| c.to_be_bytes()) // Why be not le?
            .collect::<Vec<[u8; 32]>>();

        <&Vec<[u8; 32]> as Encode<Postgres>>::encode(commitments, buf)
    }
}

impl Decode<'_, Postgres> for Commitments {
    fn decode(value: <Postgres as HasValueRef<'_>>::ValueRef) -> Result<Self, BoxDynError> {
        let value = <Vec<[u8; 32]> as Decode<Postgres>>::decode(value)?;

        let res = value.iter().map(|v| Hash::from_be_bytes(*v)).collect();

        Ok(Commitments(res))
    }
}

impl Type<Postgres> for Commitments {
    fn type_info() -> <Postgres as sqlx::Database>::TypeInfo {
        <&Vec<&[u8]> as Type<Postgres>>::type_info()
    }

    fn compatible(ty: &<Postgres as sqlx::Database>::TypeInfo) -> bool {
        <&Vec<&[u8]> as Type<Postgres>>::compatible(ty)
    }
}
