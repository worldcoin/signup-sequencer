use chrono::{DateTime, Utc};

use crate::identity_tree::Hash;

pub struct UnprocessedCommitment {
    pub commitment:    Hash,
    pub status:        String,
    pub created_at:    DateTime<Utc>,
    pub processed_at:  Option<DateTime<Utc>>,
    pub error_message: Option<String>,
}
