use crate::timed_rw_lock::TimedRwLock;
use semaphore::{
    merkle_tree::Hasher,
    poseidon_tree::{PoseidonHash, PoseidonTree, Proof},
    Field,
};
use std::str::FromStr;

use serde::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;

pub type Hash = <PoseidonHash as Hasher>::Hash;

pub struct OldTreeState {
    pub next_leaf:   usize,
    pub merkle_tree: PoseidonTree,
}

pub type SharedTreeState = Arc<TimedRwLock<OldTreeState>>;

impl OldTreeState {
    #[must_use]
    pub fn new(tree_depth: usize, initial_leaf: Field) -> Self {
        Self {
            next_leaf:   0,
            merkle_tree: PoseidonTree::new(tree_depth, initial_leaf),
        }
    }
}

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct TreeUpdate {
    pub leaf_index: usize,
    pub element:    Hash,
}

impl TreeUpdate {
    #[must_use]
    pub fn new(leaf_index: usize, element: Hash) -> Self {
        Self {
            leaf_index,
            element,
        }
    }
}

struct TreeVersionData {
    tree:      PoseidonTree,
    diff:      Vec<TreeUpdate>,
    last_leaf: usize,
    next:      Option<TreeVersion>,
}

impl TreeVersionData {
    fn new(tree_depth: usize, initial_leaf: Field) -> Self {
        Self {
            tree:      PoseidonTree::new(tree_depth, initial_leaf),
            diff:      Vec::new(),
            last_leaf: 0,
            next:      None,
        }
    }

    fn next_version(&mut self) -> TreeVersion {
        let next = TreeVersion::from(Self {
            tree:      self.tree.clone(),
            diff:      Vec::new(),
            last_leaf: self.last_leaf,
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

    async fn apply_next_update(&mut self) {
        if let Some(next) = self.next.clone() {
            let mut next = next.0.write().await;
            if let Some(update) = next.diff.first().cloned() {
                self.update(update.leaf_index, update.element);
                next.diff.remove(0);
            }
        }
    }

    fn update(&mut self, leaf_index: usize, element: Hash) {
        self.tree.set(leaf_index, element);
        self.diff.push(TreeUpdate {
            leaf_index,
            element,
        });
        self.last_leaf = leaf_index;
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
    fn new(tree_depth: usize, initial_leaf: Field) -> Self {
        Self::from(TreeVersionData::new(tree_depth, initial_leaf))
    }

    pub async fn peek_next_update(&self) -> Option<TreeUpdate> {
        let data = self.0.read().await;
        data.peek_next_update().await
    }

    pub async fn apply_next_update(&self) {
        let mut data = self.0.write().await;
        data.apply_next_update().await;
    }

    pub async fn update(&self, leaf_index: usize, element: Hash) {
        let mut data = self.0.write().await;
        data.update(leaf_index, element);
    }

    async fn next_version(&self) -> TreeVersion {
        let mut data = self.0.write().await;
        data.next_version()
    }

    pub async fn append_many_fresh(&self, updates: &[TreeUpdate]) {
        let mut data = self.0.write().await;
        let last_leaf = data.last_leaf;
        updates
            .iter()
            .filter(|update| update.leaf_index > last_leaf)
            .for_each(|update| {
                data.update(update.leaf_index, update.element);
            });
    }

    pub async fn last_leaf(&self) -> usize {
        let data = self.0.read().await;
        data.last_leaf
    }

    async fn get_proof(&self, leaf: usize) -> (Hash, Proof) {
        let tree = self.0.read().await;
        (
            tree.tree.root(),
            tree.tree
                .proof(leaf)
                .expect("impossible, tree depth mismatch between database and runtime"),
        )
    }
}

pub struct TreeItem {
    pub scope:      ValidityScope,
    pub leaf_index: usize,
}

#[derive(Clone, Copy, Serialize)]
pub enum ValidityScope {
    SequencerOnly,
    MinedOnChain,
    ConfirmedOnChain,
}

pub struct UnknownVariant;

impl FromStr for ValidityScope {
    type Err = UnknownVariant;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::SequencerOnly),
            "mined" => Ok(Self::MinedOnChain),
            "confirmed" => Ok(Self::ConfirmedOnChain),
            _ => Err(UnknownVariant),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InclusionProof {
    pub validity_scope: ValidityScope,
    pub root:           Field,
    pub proof:          Proof,
}

#[derive(Clone)]
pub struct TreeState {
    confirmed: TreeVersion,
    mined:     TreeVersion,
    latest:    TreeVersion,
}

impl TreeState {
    pub async fn new(tree_depth: usize, initial_leaf: Field) -> TreeState {
        let confirmed = TreeVersion::new(tree_depth, initial_leaf);
        let mined = confirmed.next_version().await;
        let latest = mined.next_version().await;
        TreeState {
            confirmed,
            mined,
            latest,
        }
    }

    pub fn get_latest_tree(&self) -> TreeVersion {
        self.latest.clone()
    }

    pub fn get_mined_tree(&self) -> TreeVersion {
        self.mined.clone()
    }

    pub fn get_confirmed_tree(&self) -> TreeVersion {
        self.confirmed.clone()
    }

    pub async fn get_proof(&self, item: &TreeItem) -> InclusionProof {
        let tree = match item.scope {
            ValidityScope::SequencerOnly => &self.latest,
            ValidityScope::MinedOnChain => &self.mined,
            ValidityScope::ConfirmedOnChain => &self.confirmed,
        };
        let (root, proof) = tree.get_proof(item.leaf_index).await;
        InclusionProof {
            validity_scope: item.scope,
            root,
            proof,
        }
    }
}
