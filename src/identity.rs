pub fn initialize_tree() -> (Vec<String>, Option<String>) {
    let mut identity_commitments = vec![];
    // TODO merkle tree
    let mut commitment_tree = None;
    (identity_commitments, commitment_tree)
}

pub fn inclusion_proof_helper() -> bool {
    true
}

pub fn insert_identity_helper(identity_commitment: String, mut identity_commitments: Vec<String>) -> bool {
    identity_commitments.push(identity_commitment);
    true
}