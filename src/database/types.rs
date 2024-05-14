use chrono::{DateTime, Utc};
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
