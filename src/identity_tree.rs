use crate::timed_rw_lock::TimedRwLock;
use semaphore::{
    merkle_tree::Hasher,
    poseidon_tree::{PoseidonHash, PoseidonTree},
    Field,
};
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

    pub async fn append_many(&self, updates: &[TreeUpdate]) {
        let mut latest = self.0.write().await;
        let last_leaf = latest.last_leaf;
        updates
            .iter()
            .filter(|update| update.leaf_index > last_leaf)
            .for_each(|update| {
                latest.update(update.leaf_index, update.element);
            });
    }
}

pub struct TreeState {
    confirmed: TreeVersion,
    mined:     TreeVersion,
    latest:    TreeVersion,
}

impl TreeState {
    #[must_use]
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

    pub async fn get_most_stable_proof(&self, leaf_index: usize, commitment: Hash) -> () {}
}
