use chrono::{DateTime, Utc};

use crate::identity_tree::{Hash, Status};

pub struct UnprocessedCommitment {
    pub commitment: Hash,
    pub status: Status,
    pub created_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
    pub eligibility_timestamp: Option<DateTime<Utc>>,
}

pub struct RecoveryCommitments {
    pub existing_commitment: Hash,
    pub new_commitment: Hash,
}
