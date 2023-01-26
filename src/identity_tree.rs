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
    leaf_index: usize,
    element:    Hash,
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

struct TreeVersion {
    tree:      PoseidonTree,
    diff:      Vec<TreeUpdate>,
    last_leaf: usize,
    next:      Option<Arc<RwLock<TreeVersion>>>,
}

impl TreeVersion {
    fn new(tree_depth: usize, initial_leaf: Field) -> Arc<RwLock<Self>> {
        Arc::new(RwLock::new(Self {
            tree:      PoseidonTree::new(tree_depth, initial_leaf),
            diff:      Vec::new(),
            last_leaf: 0,
            next:      None,
        }))
    }

    fn next_version(&mut self) -> Arc<RwLock<Self>> {
        let next = Arc::new(RwLock::new(TreeVersion {
            tree:      self.tree.clone(),
            diff:      Vec::new(),
            last_leaf: self.last_leaf,
            next:      None,
        }));
        self.next = Some(next.clone());
        next
    }

    async fn peek_next_update(&self) -> Option<TreeUpdate> {
        match &self.next {
            Some(next) => {
                let next = next.read().await;
                next.diff.first().cloned()
            }
            None => None,
        }
    }

    async fn apply_next_update(&mut self) {
        if let Some(next) = self.next.clone() {
            let mut next = next.write().await;
            if let Some(update) = next.diff.first().cloned() {
                self.tree.set(update.leaf_index, update.element);
                self.diff.push(update);
                next.diff.remove(0);
            }
        }
    }

    async fn update(&mut self, leaf_index: usize, element: Hash) {
        self.tree.set(leaf_index, element);
        self.diff.push(TreeUpdate {
            leaf_index,
            element,
        });
        self.last_leaf = leaf_index;
    }
}

pub struct TreeState {
    confirmed: Arc<RwLock<TreeVersion>>,
    minted:    Arc<RwLock<TreeVersion>>,
    latest:    Arc<RwLock<TreeVersion>>,
}

impl TreeState {
    #[must_use]
    pub async fn new(tree_depth: usize, initial_leaf: Field) -> TreeState {
        let confirmed = TreeVersion::new(tree_depth, initial_leaf);
        let minted = confirmed.write().await.next_version();
        let latest = minted.write().await.next_version();
        TreeState {
            confirmed,
            minted,
            latest,
        }
    }

    pub async fn set_leaf(&self, value: Hash, leaf_index: usize) {
        self.latest.write().await.update(leaf_index, value).await;
    }

    pub async fn append_range(&self, updates: &[TreeUpdate]) {
        let mut latest = self.latest.write().await;
        let last_leaf = latest.last_leaf;
        updates
            .iter()
            .filter(|update| update.leaf_index > last_leaf)
            .for_each(|update| {
                latest.update(update.leaf_index, update.element);
            });
    }

    pub async fn last_index(&self) -> usize {
        self.latest.read().await.last_leaf
    }

    pub async fn get_most_stable_proof(&self, leaf_idx: usize, commitment: &Hash) -> Option<()> {
        todo!();
    }
}
