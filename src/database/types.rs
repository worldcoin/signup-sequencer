use chrono::{DateTime, Utc};

use crate::identity_tree::{Hash, PendingStatus};

pub struct UnprocessedCommitment {
    pub commitment:            Hash,
    pub status:                PendingStatus,
    pub created_at:            DateTime<Utc>,
    pub processed_at:          Option<DateTime<Utc>>,
    pub error_message:         Option<String>,
    pub eligibility_timestamp: DateTime<Utc>,
}

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
