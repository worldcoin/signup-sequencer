use crate::identity_tree::Hash;

pub struct UnprocessedCommitment {
    pub commitment:    Hash,
    pub status:        String,
    pub created_at:    String,
    pub processed_at:  String,
    pub error_message: String,
}
