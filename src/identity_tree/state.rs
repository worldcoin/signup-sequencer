use semaphore_rs::Field;

use crate::identity_tree::{
    Canonical, InclusionProof, Intermediate, Latest, ProcessedStatus, TreeItem, TreeVersion,
    TreeVersionReadOps,
};

#[derive(Clone)]
pub struct TreeState {
    mined: TreeVersion<Canonical>,
    processed: TreeVersion<Intermediate>,
    batching: TreeVersion<Intermediate>,
    latest: TreeVersion<Latest>,
}

impl TreeState {
    #[must_use]
    pub const fn new(
        mined: TreeVersion<Canonical>,
        processed: TreeVersion<Intermediate>,
        batching: TreeVersion<Intermediate>,
        latest: TreeVersion<Latest>,
    ) -> Self {
        Self {
            mined,
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
    pub fn get_batching_tree(&self) -> TreeVersion<Intermediate> {
        self.batching.clone()
    }

    pub fn batching_tree(&self) -> &TreeVersion<Intermediate> {
        &self.batching
    }

    #[must_use]
    pub fn get_processed_tree(&self) -> TreeVersion<Intermediate> {
        self.processed.clone()
    }

    pub fn processed_tree(&self) -> &TreeVersion<Intermediate> {
        &self.processed
    }

    #[must_use]
    pub fn get_mined_tree(&self) -> TreeVersion<Canonical> {
        self.mined.clone()
    }

    pub fn mined_tree(&self) -> &TreeVersion<Canonical> {
        &self.mined
    }

    #[must_use]
    pub fn get_proof_for(&self, item: &TreeItem) -> (Field, InclusionProof) {
        let (leaf, root, proof) = match item.status {
            ProcessedStatus::Pending => self.latest.get_leaf_and_proof(item.leaf_index),
            ProcessedStatus::Processed => self.processed.get_leaf_and_proof(item.leaf_index),
            ProcessedStatus::Mined => self.mined.get_leaf_and_proof(item.leaf_index),
        };

        let proof = InclusionProof {
            status: item.status.into(),
            root: Some(root),
            proof: Some(proof),
            message: None,
        };

        (leaf, proof)
    }
}
