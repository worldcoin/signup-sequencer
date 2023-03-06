use chrono::Utc;
use semaphore::{
    merkle_tree::Hasher,
    poseidon_tree::{PoseidonHash, PoseidonTree, Proof},
    Field,
};
use serde::Serialize;
use std::{str::FromStr, sync::Arc};
use thiserror::Error;
use tokio::sync::RwLock;

pub type Hash = <PoseidonHash as Hasher>::Hash;

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct TreeUpdate {
    pub leaf_index: usize,
    pub element:    Hash,
}

impl TreeUpdate {
    #[must_use]
    pub const fn new(leaf_index: usize, element: Hash) -> Self {
        Self {
            leaf_index,
            element,
        }
    }
}

struct TreeVersionData {
    tree:      PoseidonTree,
    diff:      Vec<TreeUpdate>,
    next_leaf: usize,
    next:      Option<TreeVersion>,
}

impl TreeVersionData {
    fn empty(tree_depth: usize, initial_leaf: Field) -> Self {
        Self {
            tree:      PoseidonTree::new(tree_depth, initial_leaf),
            diff:      Vec::new(),
            next_leaf: 0,
            next:      None,
        }
    }

    fn get_root(&self) -> Hash {
        self.tree.root()
    }

    fn next_version(&mut self) -> TreeVersion {
        let next = TreeVersion::from(Self {
            tree:      self.tree.clone(),
            diff:      Vec::new(),
            next_leaf: self.next_leaf,
            next:      None,
        });
        self.next = Some(next.clone());
        next
    }

    async fn peek_next_update(&self) -> Option<TreeUpdate> {
        match &self.next {
            Some(next) => {
                let next = next.0.read().await;
                next.diff.first().cloned()
            }
            None => None,
        }
    }

    /// Returns _up to_ `maximum_update_count` updates that are to be applied to
    /// the tree.
    async fn peek_next_updates(&self, maximum_update_count: usize) -> Vec<TreeUpdate> {
        match &self.next {
            Some(next) => {
                let next = next.0.read().await;
                next.diff
                    .iter()
                    .take(maximum_update_count)
                    .cloned()
                    .collect()
            }
            None => vec![],
        }
    }

    async fn apply_next_update(&mut self) {
        if let Some(next) = self.next.clone() {
            let mut next = next.0.write().await;
            if let Some(update) = next.diff.first().cloned() {
                self.update(update.leaf_index, update.element);
                next.diff.remove(0);
            }
        }
    }

    /// Applies the next _up to_ `update_count` updates, returning the merkle
    /// tree proofs obtained after each apply.
    async fn apply_next_updates(&mut self, update_count: usize) -> Vec<Proof> {
        let mut proofs: Vec<Proof> = vec![];
        if let Some(next) = self.next.clone() {
            // Acquire the exclusive write lock on the next version.
            let mut next = next.0.write().await;

            // Get the updates to be applied and apply them sequentially. It is very
            // important that we record the merkle proof after each step as they depend on
            // each other.
            let updates: Vec<&TreeUpdate> = next.diff.iter().take(update_count).collect();
            for update in &updates {
                self.update(update.leaf_index, update.element);
                let proof = self.tree.proof(update.leaf_index).unwrap_or_else(|| {
                    panic!("No proof exists at leaf index {}.", update.leaf_index);
                });
                proofs.push(proof);
            }

            // Remove only the updates that have been consumed, which may be all of them.
            next.diff = if next.diff.len() > updates.len() {
                Vec::from(&next.diff[updates.len()..])
            } else {
                vec![]
            }
        }
        proofs
    }

    fn update(&mut self, leaf_index: usize, element: Hash) {
        self.update_without_diff(leaf_index, element);
        self.diff.push(TreeUpdate {
            leaf_index,
            element,
        });
    }

    fn update_without_diff(&mut self, leaf_index: usize, element: Hash) {
        self.tree.set(leaf_index, element);
        self.next_leaf = leaf_index + 1;
    }
}

#[derive(Clone)]
pub struct TreeVersion(Arc<RwLock<TreeVersionData>>);

impl From<TreeVersionData> for TreeVersion {
    fn from(data: TreeVersionData) -> Self {
        Self(Arc::new(RwLock::new(data)))
    }
}

impl TreeVersion {
    pub async fn get_root(&self) -> Hash {
        let data = self.0.read().await;
        data.get_root()
    }

    pub async fn peek_next_update(&self) -> Option<TreeUpdate> {
        let data = self.0.read().await;
        data.peek_next_update().await
    }

    pub async fn peek_next_updates(&self, maximum_update_count: usize) -> Vec<TreeUpdate> {
        let data = self.0.read().await;
        data.peek_next_updates(maximum_update_count).await
    }

    pub async fn apply_next_update(&self) {
        let mut data = self.0.write().await;
        data.apply_next_update().await;
    }

    pub async fn apply_next_updates(&self, update_count: usize) -> Vec<Proof> {
        let mut data = self.0.write().await;
        data.apply_next_updates(update_count).await
    }

    pub async fn update(&self, leaf_index: usize, element: Hash) {
        let mut data = self.0.write().await;
        data.update(leaf_index, element);
    }

    pub async fn next_version(&self) -> Self {
        let mut data = self.0.write().await;
        data.next_version()
    }

    pub async fn append_many_fresh(&self, updates: &[TreeUpdate]) {
        let mut data = self.0.write().await;
        let next_leaf = data.next_leaf;
        updates
            .iter()
            .filter(|update| update.leaf_index >= next_leaf)
            .for_each(|update| {
                data.update(update.leaf_index, update.element);
            });
    }

    pub async fn next_leaf(&self) -> usize {
        let data = self.0.read().await;
        data.next_leaf
    }

    pub async fn get_proof(&self, leaf: usize) -> (Hash, Proof) {
        let tree = self.0.read().await;
        (
            tree.tree.root(),
            tree.tree
                .proof(leaf)
                .expect("impossible, tree depth mismatch between database and runtime"),
        )
    }
}

#[derive(Debug)]
pub struct TreeItem {
    pub status:     Status,
    pub leaf_index: usize,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Pending,
    Mined,
}

#[derive(Debug, Error)]
#[error("unknown status")]
pub struct UnknownStatus;

impl FromStr for Status {
    type Err = UnknownStatus;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "mined" => Ok(Self::Mined),
            _ => Err(UnknownStatus),
        }
    }
}

impl From<Status> for &str {
    fn from(scope: Status) -> Self {
        match scope {
            Status::Pending => "pending",
            Status::Mined => "mined",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RootItem {
    pub root:                Field,
    pub status:              Status,
    pub pending_valid_as_of: chrono::DateTime<Utc>,
    pub mined_valid_as_of:   Option<chrono::DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InclusionProof {
    pub status: Status,
    pub root:   Field,
    pub proof:  Proof,
}

#[derive(Clone)]
pub struct TreeState {
    mined:    TreeVersion,
    batching: TreeVersion,
    latest:   TreeVersion,
}

impl TreeState {
    #[must_use]
    pub const fn new(mined: TreeVersion, batching: TreeVersion, latest: TreeVersion) -> Self {
        Self {
            mined,
            batching,
            latest,
        }
    }

    #[must_use]
    pub fn get_latest_tree(&self) -> TreeVersion {
        self.latest.clone()
    }

    #[must_use]
    pub fn get_mined_tree(&self) -> TreeVersion {
        self.mined.clone()
    }

    #[must_use]
    pub fn get_batching_tree(&self) -> TreeVersion {
        self.batching.clone()
    }

    pub async fn get_proof_for(&self, item: &TreeItem) -> InclusionProof {
        let tree = match item.status {
            Status::Pending => &self.latest,
            Status::Mined => &self.mined,
        };
        let (root, proof) = tree.get_proof(item.leaf_index).await;
        InclusionProof {
            status: item.status,
            root,
            proof,
        }
    }
}

pub struct CanonicalTreeBuilder(TreeVersionData);

impl CanonicalTreeBuilder {
    #[must_use]
    pub fn new(tree_depth: usize, initial_leaf: Field) -> Self {
        Self(TreeVersionData::empty(tree_depth, initial_leaf))
    }

    pub fn append(&mut self, update: &TreeUpdate) {
        self.0
            .update_without_diff(update.leaf_index, update.element);
    }

    #[must_use]
    pub fn seal(self) -> TreeVersion {
        self.0.into()
    }
}
