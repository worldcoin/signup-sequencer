use std::{
    str::FromStr,
    sync::{Arc, Mutex, MutexGuard},
};

use semaphore::{
    lazy_merkle_tree,
    lazy_merkle_tree::LazyMerkleTree,
    merkle_tree::Hasher,
    poseidon_tree::{PoseidonHash, Proof},
    Field,
};
use serde::Serialize;
use thiserror::Error;

pub type PoseidonTree<Version> = LazyMerkleTree<PoseidonHash, Version>;
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
#[serde(rename_all = "camelCase")]
pub struct InclusionProof {
    pub status: Status,
    pub root:   Field,
    pub proof:  Proof,
}

pub struct CanonicalTreeData {
    flatten_threshold: usize,
    count_since_last_flatten: usize,

}

struct TreeVersionData<V: lazy_merkle_tree::VersionMarker> {
    tree: PoseidonTree<V>,
    diff: Vec<TreeUpdate>,
    next_leaf: usize,
    next: Option<TreeVersion<AnyDerived>>,
    canonical_tree_data: Option<CanonicalTreeData>,
}

impl<V: lazy_merkle_tree::VersionMarker> TreeVersionData<V> {
    // fn empty(tree_depth: usize, initial_leaf: Field) -> Self {
    //     Self {
    //         tree:      PoseidonTree::new(tree_depth, initial_leaf),
    //         diff:      Vec::new(),
    //         next_leaf: 0,
    //         next:      None,
    //     }
    // }

    fn get_root(&self) -> Hash {
        self.tree.root()
    }

    /// Returns _up to_ `maximum_update_count` updates that are to be applied to
    /// the tree.
    fn peek_next_updates(&self, maximum_update_count: usize) -> Vec<TreeUpdate> {
        match &self.next {
            Some(next) => {
                let next = next.get_data();
                next.diff
                    .iter()
                    .take(maximum_update_count)
                    .cloned()
                    .collect()
            }
            None => vec![],
        }
    }

    /// Applies the next _up to_ `update_count` updates, returning the merkle
    /// tree proofs obtained after each apply.
    fn apply_next_updates(&mut self, update_count: usize) -> Vec<Proof> {
        let mut proofs: Vec<Proof> = vec![];
        if let Some(next) = self.next.clone() {
            // Acquire the exclusive write lock on the next version.
            let mut next = next.get_data();

            // Get the updates to be applied and apply them sequentially. It is very
            // important that we record the merkle proof after each step as they depend on
            // each other.
            let updates: Vec<&TreeUpdate> = next.diff.iter().take(update_count).collect();
            for update in &updates {
                self.update(update.leaf_index, update.element);
                let proof = self.tree.proof(update.leaf_index);
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
        self.tree = self.tree.update(leaf_index, &element);
        self.next_leaf = leaf_index + 1;
    }
}

pub trait Version {
    type TreeVersion: lazy_merkle_tree::VersionMarker;
}

pub trait HasNextVersion
where
    Self: Version,
{
}

#[derive(Clone)]
pub struct Canonical;
impl Version for Canonical {
    type TreeVersion = lazy_merkle_tree::Canonical;
}
impl HasNextVersion for Canonical {}

#[derive(Clone)]
pub struct Intermediate;
impl Version for Intermediate {
    type TreeVersion = lazy_merkle_tree::Derived;
}
impl HasNextVersion for Intermediate {}

#[derive(Clone)]
pub struct Latest;
impl Version for Latest {
    type TreeVersion = lazy_merkle_tree::Derived;
}

#[derive(Clone)]
pub struct AnyDerived;
impl Version for AnyDerived {
    type TreeVersion = lazy_merkle_tree::Derived;
}

#[derive(Clone)]
pub struct TreeVersion<V: Version>(Arc<Mutex<TreeVersionData<V::TreeVersion>>>);

impl<V: Version> TreeVersion<V> {
    fn get_data(&self) -> MutexGuard<TreeVersionData<V::TreeVersion>> {
        self.0.lock().expect("no lock tainting")
    }

    pub fn get_root(&self) -> Hash {
        self.get_data().get_root()
    }

    pub fn next_leaf(&self) -> usize {
        self.get_data().next_leaf
    }

    pub fn get_proof(&self, leaf: usize) -> (Hash, Proof) {
        let tree = self.get_data();
        (tree.tree.root(), tree.tree.proof(leaf))
    }
}

impl TreeVersion<Latest> {
    pub fn append_many_fresh(&self, updates: &[TreeUpdate]) {
        let mut data = self.get_data();
        let next_leaf = data.next_leaf;
        updates
            .iter()
            .filter(|update| update.leaf_index >= next_leaf)
            .for_each(|update| {
                data.update(update.leaf_index, update.element);
            });
    }
}

impl<V: HasNextVersion> TreeVersion<V> {
    pub fn peek_next_updates(&self, maximum_update_count: usize) -> Vec<TreeUpdate> {
        self.get_data().peek_next_updates(maximum_update_count)
    }

    pub fn apply_next_updates(&self, update_count: usize) -> Vec<Proof> {
        self.get_data().apply_next_updates(update_count)
    }
}

#[derive(Clone)]
pub struct TreeState {
    mined:    TreeVersion<Canonical>,
    batching: TreeVersion<Intermediate>,
    latest:   TreeVersion<Latest>,
}

impl TreeState {
    #[must_use]
    pub const fn new(
        mined: TreeVersion<Canonical>,
        batching: TreeVersion<Intermediate>,
        latest: TreeVersion<Latest>,
    ) -> Self {
        Self {
            mined,
            batching,
            latest,
        }
    }

    #[must_use]
    pub fn get_latest_tree(&self) -> TreeVersion<Latest> {
        self.latest.clone()
    }

    #[must_use]
    pub fn get_mined_tree(&self) -> TreeVersion<Canonical> {
        self.mined.clone()
    }

    #[must_use]
    pub fn get_batching_tree(&self) -> TreeVersion<Intermediate> {
        self.batching.clone()
    }

    pub fn get_proof_for(&self, item: &TreeItem) -> InclusionProof {
        let (root, proof) = match item.status {
            Status::Pending => self.latest.get_proof(item.leaf_index),
            Status::Mined => self.mined.get_proof(item.leaf_index),
        };
        InclusionProof {
            status: item.status,
            root,
            proof,
        }
    }
}

pub struct CanonicalTreeBuilder;

// impl CanonicalTreeBuilder {
//     #[must_use]
//     pub fn new(tree_depth: usize, initial_leaf: Field) -> Self {
//         Self(TreeVersionData::empty(tree_depth, initial_leaf))
//     }
//
//     pub fn append(&mut self, update: &TreeUpdate) {
//         self.0
//             .update_without_diff(update.leaf_index, update.element);
//     }
//
//     #[must_use]
//     pub fn seal(self) -> TreeVersion {
//         self.0.into()
//     }
// }
