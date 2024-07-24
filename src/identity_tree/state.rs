use semaphore::Field;

use crate::identity_tree::{
    Canonical, InclusionProof, Intermediate, Latest, TreeItem, TreeVersion, TreeVersionReadOps,
};

#[derive(Clone)]
pub struct TreeState {
    processed: TreeVersion<Canonical>,
    batching:  TreeVersion<Intermediate>,
    latest:    TreeVersion<Latest>,
}

impl TreeState {
    #[must_use]
    pub const fn new(
        processed: TreeVersion<Canonical>,
        batching: TreeVersion<Intermediate>,
        latest: TreeVersion<Latest>,
    ) -> Self {
        Self {
            processed,
            batching,
            latest,
        }
    }

    pub fn latest_tree(&self) -> &TreeVersion<Latest> {
        &self.latest
    }

    #[must_use]
    pub fn get_latest_tree(&self) -> TreeVersion<Latest> {
        self.latest.clone()
    }

    #[must_use]
    pub fn get_processed_tree(&self) -> TreeVersion<Canonical> {
        self.processed.clone()
    }

    pub fn processed_tree(&self) -> &TreeVersion<Canonical> {
        &self.processed
    }

    #[must_use]
    pub fn get_batching_tree(&self) -> TreeVersion<Intermediate> {
        self.batching.clone()
    }

    pub fn batching_tree(&self) -> &TreeVersion<Intermediate> {
        &self.batching
    }

    #[must_use]
    pub fn get_proof_for(&self, item: &TreeItem) -> (Field, InclusionProof) {
        let (leaf, root, proof) = self.latest.get_leaf_and_proof(item.leaf_index);

        let proof = InclusionProof {
            root:    Some(root),
            proof:   Some(proof),
            message: None,
        };

        (leaf, proof)
    }
}
