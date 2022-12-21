use crate::timed_rw_lock::TimedRwLock;
use semaphore::{
    merkle_tree::Hasher,
    poseidon_tree::{PoseidonHash, PoseidonTree},
    Field,
};
use std::sync::Arc;

pub type Hash = <PoseidonHash as Hasher>::Hash;

pub struct TreeState {
    pub next_leaf:   usize,
    pub merkle_tree: PoseidonTree,
}

pub type SharedTreeState = Arc<TimedRwLock<TreeState>>;

impl TreeState {
    #[must_use]
    pub fn new(tree_depth: usize, initial_leaf: Field) -> Self {
        Self {
            next_leaf:   0,
            merkle_tree: PoseidonTree::new(tree_depth, initial_leaf),
        }
    }
}
