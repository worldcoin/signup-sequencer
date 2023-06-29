use std::cmp::min;
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::Utc;
use semaphore::lazy_merkle_tree::{Derived, LazyMerkleTree};
use semaphore::merkle_tree::Hasher;
use semaphore::poseidon_tree::{PoseidonHash, Proof};
use semaphore::{lazy_merkle_tree, Field};
use serde::Serialize;
use thiserror::Error;
use tracing::{info, warn};

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

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    New,
    Failed,
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
            "new" => Ok(Self::New),
            "failed" => Ok(Self::Failed),
            "pending" => Ok(Self::Pending),
            "mined" => Ok(Self::Mined),
            _ => Err(UnknownStatus),
        }
    }
}

impl From<Status> for &str {
    fn from(scope: Status) -> Self {
        match scope {
            Status::New => "new",
            Status::Failed => "failed",
            Status::Pending => "pending",
            Status::Mined => "mined",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootItem {
    pub root:                Field,
    pub status:              Status,
    pub pending_valid_as_of: chrono::DateTime<Utc>,
    pub mined_valid_as_of:   Option<chrono::DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InclusionProof {
    pub status:  Status,
    pub root:    Option<Field>,
    pub proof:   Option<Proof>,
    pub message: Option<String>,
}

/// Additional data held by the canonical tree version. It includes data
/// necessary to control garbage collection.
pub struct CanonicalTreeMetadata {
    flatten_threshold:        usize,
    count_since_last_flatten: usize,
}

/// Additional data held by any derived tree version. Includes the list of
/// updates performed since previous version.
pub struct DerivedTreeMetadata {
    diff: Vec<AppliedTreeUpdate>,
}

#[derive(Clone)]
pub struct AppliedTreeUpdate {
    pub update: TreeUpdate,
    pub result: PoseidonTree<Derived>,
}

/// Trait used to associate a version marker with its metadata type.
pub trait AllowedTreeVersionMarker
where
    Self: lazy_merkle_tree::VersionMarker,
{
    type Metadata;
}

impl AllowedTreeVersionMarker for lazy_merkle_tree::Canonical {
    type Metadata = CanonicalTreeMetadata;
}

impl AllowedTreeVersionMarker for lazy_merkle_tree::Derived {
    type Metadata = DerivedTreeMetadata;
}

/// Underlying data structure for a tree version. It holds the tree itself, the
/// next leaf (only used in the latest tree), a pointer to the next version (if
/// exists) and the metadata specified by the version marker.
struct TreeVersionData<V: AllowedTreeVersionMarker> {
    tree:      PoseidonTree<V>,
    next_leaf: usize,
    next:      Option<TreeVersion<AnyDerived>>,
    metadata:  V::Metadata,
}

/// Basic operations that should be available for all tree versions.
trait BasicTreeOps {
    /// Updates the tree with the given element at the given leaf index.
    fn update(&mut self, leaf_index: usize, element: Hash);

    fn apply_diffs(&mut self, diffs: Vec<AppliedTreeUpdate>);

    /// Notifies the tree that it was changed and can perform garbage
    /// collection. This is version-specific and it is up to the implementer to
    /// decide how to handle this signal.
    fn garbage_collect(&mut self);
}

impl<V> TreeVersionData<V>
where
    V: lazy_merkle_tree::VersionMarker + AllowedTreeVersionMarker,
    Self: BasicTreeOps,
{
    /// Gets the current tree root.
    fn get_root(&self) -> Hash {
        self.tree.root()
    }

    /// Gets the proof of the given leaf index element
    fn get_proof(&self, leaf: usize) -> (Hash, Proof) {
        let proof = self.tree.proof(leaf);
        (self.tree.root(), proof)
    }

    /// Returns _up to_ `maximum_update_count` updates that are to be applied to
    /// the tree.
    fn peek_next_updates(&self, maximum_update_count: usize) -> Vec<AppliedTreeUpdate> {
        let Some(next) = self.next.as_ref() else { return Vec::new(); };

        let next = next.get_data();
        next.metadata
            .diff
            .iter()
            .take(maximum_update_count)
            .cloned()
            .collect()
    }

    fn apply_updates_up_to(&mut self, root: Hash) -> usize {
        let Some(next) = self.next.clone() else { return 0; };

        let num_updates;
        {
            // Acquire the exclusive write lock on the next version.
            let mut next = next.get_data();

            let index_of_root = next
                .metadata
                .diff
                .iter()
                .position(|update| update.result.root() == root);

            let Some(index_of_root) = index_of_root else {
                warn!(?root, "Root not found in the diff");
                return 0;
            };

            let applied_updates: Vec<_> = next.metadata.diff.drain(..=index_of_root).collect();

            num_updates = applied_updates.len();

            self.apply_diffs(applied_updates);
        }

        self.garbage_collect();

        num_updates
    }
}

impl BasicTreeOps for TreeVersionData<lazy_merkle_tree::Canonical> {
    fn update(&mut self, leaf_index: usize, element: Hash) {
        take_mut::take(&mut self.tree, |tree| {
            tree.update_with_mutation(leaf_index, &element)
        });
        self.next_leaf = leaf_index + 1;
        self.metadata.count_since_last_flatten += 1;
    }

    fn apply_diffs(&mut self, diffs: Vec<AppliedTreeUpdate>) {
        for applied_update in &diffs {
            let update = &applied_update.update;
            self.update(update.leaf_index, update.element);
        }
    }

    /// Garbage collection for the canonical tree version. It rewrites all
    /// future versions of the tree to use the more optimized storage of this
    /// tree. This is done periodically, to really make the additional
    /// time-costs of these rebuilds repay themselves in saved memory.
    ///
    /// Warning: this will attempt to acquire lock for all transitive successors
    /// of this tree, and therefore no version locks acquired through
    /// `TreeVersion#get_data()` may be held at the time of calling this.
    fn garbage_collect(&mut self) {
        if self.metadata.count_since_last_flatten >= self.metadata.flatten_threshold {
            info!("Flattening threshold reached, rebuilding tree versions");
            self.metadata.count_since_last_flatten = 0;
            let next = &self.next;
            if let Some(next) = next {
                next.get_data().rebuild_on(self.tree.derived());
            }
            info!("Tree versions rebuilt");
        }
    }
}

impl TreeVersionData<lazy_merkle_tree::Derived> {
    fn rebuild_on(&mut self, mut tree: PoseidonTree<lazy_merkle_tree::Derived>) {
        for update in &mut self.metadata.diff {
            tree = tree.update(update.update.leaf_index, &update.update.element);
            update.result = tree.clone();
        }
        self.tree = tree;
        let next = &self.next;
        if let Some(next) = next {
            next.get_data().rebuild_on(self.tree.clone());
        }
    }
}

impl BasicTreeOps for TreeVersionData<lazy_merkle_tree::Derived> {
    fn update(&mut self, leaf_index: usize, element: Hash) {
        let updated_tree = self.tree.update(leaf_index, &element);

        self.tree = updated_tree.clone();

        self.next_leaf = leaf_index + 1;
        self.metadata.diff.push(AppliedTreeUpdate {
            update: TreeUpdate {
                leaf_index,
                element,
            },
            result: updated_tree,
        });
    }

    fn apply_diffs(&mut self, mut diffs: Vec<AppliedTreeUpdate>) {
        let last = diffs.last().cloned();

        self.metadata.diff.append(&mut diffs);

        if let Some(last) = last {
            self.tree = last.result.clone();
            self.next_leaf = last.update.leaf_index + 1;
        }
    }

    fn garbage_collect(&mut self) {}
}

/// The marker trait for linear ordering of tree versions. It also defines the
/// marker for underlying tree storage.
pub trait Version {
    type TreeVersion: AllowedTreeVersionMarker;
}

/// Marks tree versions that have a successor. This modifies the behavior of the
/// tree, only allowing it to be modified by pulling changes from the successor,
/// rather than to be updated freely.
pub trait HasNextVersion
where
    Self: Version,
{
}

/// Marker for the canonical tree version – one that is not a successor of any
/// other version. This marker is mostly useful for optimizing storage – not
/// storing the `diff` and performing in-place updates.
#[derive(Clone)]
pub struct Canonical;
impl Version for Canonical {
    type TreeVersion = lazy_merkle_tree::Canonical;
}
impl HasNextVersion for Canonical {}

/// Marker for an intermediate version – one that has both a predecessor and a
/// successor.
#[derive(Clone)]
pub struct Intermediate;
impl Version for Intermediate {
    type TreeVersion = lazy_merkle_tree::Derived;
}
impl HasNextVersion for Intermediate {}

/// Marker for the latest tree version – one that has no successor. It enables a
/// different API, focusing on outside updates, rather than just pulling in
/// updates from the successor.
#[derive(Clone)]
pub struct Latest;
impl Version for Latest {
    type TreeVersion = lazy_merkle_tree::Derived;
}

/// Marker for any tree version that has a predecessor. It is useful internally
/// for storage inside `TreeVersionData`, but it is probably not going to be
/// useful for any clients.
#[derive(Clone)]
struct AnyDerived;
impl Version for AnyDerived {
    type TreeVersion = lazy_merkle_tree::Derived;
}

/// The most important public-facing type of this library. Exposes a type-safe
/// API for working with versioned trees. It uses interior mutability and
/// cloning it only gives a new handle on the underlying shared memory.
pub struct TreeVersion<V: Version>(Arc<Mutex<TreeVersionData<V::TreeVersion>>>);

impl<V: Version> Clone for TreeVersion<V> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<V: Version<TreeVersion = lazy_merkle_tree::Derived>> TreeVersion<V> {
    /// Only used internally to upcast a compatible tree version to
    /// `AnyDerived`.
    fn as_derived(&self) -> TreeVersion<AnyDerived> {
        TreeVersion(self.0.clone())
    }
}

/// The public-facing API for reading from a tree version. It is implemented for
/// all versions. This being a trait allows us to hide some of the
/// implementation details.
pub trait TreeVersionReadOps {
    /// Returns the current tree root.
    fn get_root(&self) -> Hash;
    /// Returns the next free leaf.
    fn next_leaf(&self) -> usize;
    /// Returns the merkle proof and element at the given leaf.
    fn get_proof(&self, leaf: usize) -> (Hash, Proof);
}

impl<V: Version> TreeVersionReadOps for TreeVersion<V>
where
    TreeVersionData<V::TreeVersion>: BasicTreeOps,
{
    fn get_root(&self) -> Hash {
        self.get_data().get_root()
    }

    fn next_leaf(&self) -> usize {
        self.get_data().next_leaf
    }

    fn get_proof(&self, leaf: usize) -> (Hash, Proof) {
        let tree = self.get_data();
        tree.get_proof(leaf)
    }
}

impl<V: Version> TreeVersion<V> {
    fn get_data(&self) -> MutexGuard<TreeVersionData<V::TreeVersion>> {
        self.0.lock().expect("no lock poisoning")
    }
}

impl TreeVersion<Latest> {
    /// Appends many identities to the tree, returns a list with the root, proof
    /// of inclusion and leaf index
    #[must_use]
    pub fn append_many(&self, identities: &[Hash]) -> Vec<(Hash, Proof, usize)> {
        let mut data = self.get_data();
        let next_leaf = data.next_leaf;

        let mut output = Vec::with_capacity(identities.len());

        for (idx, identity) in identities.iter().enumerate() {
            let leaf_index = next_leaf + idx;

            data.update(leaf_index, *identity);
            let (root, proof) = data.get_proof(leaf_index);

            output.push((root, proof, leaf_index));
        }

        output
    }
}

/// Public API for working with versions that have a successor. Such versions
/// only allow peeking and applying updates from the successor.
pub trait TreeWithNextVersion {
    fn peek_next_updates(&self, maximum_update_count: usize) -> Vec<AppliedTreeUpdate>;
    fn apply_updates_up_to(&self, root: Hash) -> usize;
}

impl<V> TreeWithNextVersion for TreeVersion<V>
where
    V: HasNextVersion,
    TreeVersionData<<V as Version>::TreeVersion>: BasicTreeOps,
{
    fn peek_next_updates(&self, maximum_update_count: usize) -> Vec<AppliedTreeUpdate> {
        self.get_data().peek_next_updates(maximum_update_count)
    }

    fn apply_updates_up_to(&self, root: Hash) -> usize {
        self.get_data().apply_updates_up_to(root)
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

    #[must_use]
    pub fn get_proof_for(&self, item: &TreeItem) -> InclusionProof {
        let (root, proof) = match item.status {
            Status::Pending | Status::New | Status::Failed => {
                self.latest.get_proof(item.leaf_index)
            }
            Status::Mined => self.mined.get_proof(item.leaf_index),
        };
        InclusionProof {
            status:  item.status,
            root:    Some(root),
            proof:   Some(proof),
            message: None,
        }
    }
}

/// A helper for building the first tree version. Exposes a type-safe API over
/// building a sequence of tree versions efficiently.
pub struct CanonicalTreeBuilder(TreeVersionData<lazy_merkle_tree::Canonical>);
impl CanonicalTreeBuilder {
    /// Creates a new builder with the given parameters.
    /// * `tree_depth`: The depth of the tree.
    /// * `dense_prefix_depth`: The depth of the dense prefix – i.e. the prefix
    ///   of the tree that will be stored in a vector rather than in a
    ///   pointer-based structure.
    /// * `flattening_threshold`: The number of updates that can be applied to
    ///   this tree before garbage collection is triggered. GC is quite
    ///   expensive time-wise, so this value should be chosen carefully to make
    ///   sure it pays off in memory savings. A rule of thumb is that GC will
    ///   free up roughly `Depth * Number of Versions * Flattening Threshold`
    ///   nodes in the tree.
    /// * `initial_leaf`: The initial value of the tree leaves.
    #[must_use]
    pub fn new(
        tree_depth: usize,
        dense_prefix_depth: usize,
        flattening_threshold: usize,
        initial_leaf: Field,
        initial_leaves: &[Field],
        mmap_file_path: &str,
    ) -> Self {
        let initial_leaves_in_dense_count = min(initial_leaves.len(), 1 << dense_prefix_depth);
        let (initial_leaves_in_dense, leftover_initial_leaves) =
            initial_leaves.split_at(initial_leaves_in_dense_count);

        let tree =
            PoseidonTree::<lazy_merkle_tree::Canonical>::new_mmapped_with_dense_prefix_with_init_values(
                tree_depth,
                dense_prefix_depth,
                &initial_leaf,
                initial_leaves_in_dense,
                mmap_file_path
            ).unwrap();

        let metadata = CanonicalTreeMetadata {
            flatten_threshold:        flattening_threshold,
            count_since_last_flatten: 0,
        };
        let mut builder = Self(TreeVersionData {
            tree,
            next_leaf: initial_leaves_in_dense_count,
            metadata,
            next: None,
        });
        for (index, leaf) in leftover_initial_leaves.iter().enumerate() {
            builder.update(&TreeUpdate {
                leaf_index: index + initial_leaves_in_dense_count,
                element:    *leaf,
            });
        }
        builder
    }

    pub fn restore(
        tree_depth: usize,
        dense_prefix_depth: usize,
        initial_leaf: &Field,
        last_index: usize,
        flattening_threshold: usize,
        mmap_file_path: &str,
    ) -> Option<Self> {
        let tree: LazyMerkleTree<PoseidonHash, lazy_merkle_tree::Canonical> =
            match PoseidonTree::<lazy_merkle_tree::Canonical>::attempt_dense_mmap_restore(
                tree_depth,
                dense_prefix_depth,
                initial_leaf,
                mmap_file_path,
            ) {
                Ok(tree) => tree,
                Err(error) => {
                    warn!("Tree wasn't restored. Reason: {}", error.to_string());
                    return None;
                }
            };

        let leaves_in_dense_count = min(last_index, 1 << dense_prefix_depth);

        let metadata = CanonicalTreeMetadata {
            flatten_threshold:        flattening_threshold,
            count_since_last_flatten: 0,
        };
        let builder = Self(TreeVersionData {
            tree,
            next_leaf: leaves_in_dense_count,
            metadata,
            next: None,
        });

        Some(builder)
    }

    /// Updates a leaf in the resulting tree.
    pub fn update(&mut self, update: &TreeUpdate) {
        self.0.update(update.leaf_index, update.element);
    }

    /// Seals this version and returns a builder for the next version.
    #[must_use]
    pub fn seal(self) -> (TreeVersion<Canonical>, DerivedTreeBuilder<Canonical>) {
        let next_tree = self.0.tree.derived();
        let next_leaf = self.0.next_leaf;
        let sealed = TreeVersion(Arc::new(Mutex::new(self.0)));
        let next = DerivedTreeBuilder::<Canonical>::new(next_tree, next_leaf, sealed.clone());
        (sealed, next)
    }
}

/// A helper for building successive tree versions. Exposes a type-safe API over
/// building a sequence of tree versions efficiently.
pub struct DerivedTreeBuilder<P: Version> {
    prev:    TreeVersion<P>,
    current: TreeVersionData<lazy_merkle_tree::Derived>,
}

impl<P: Version> DerivedTreeBuilder<P> {
    #[must_use]
    const fn new<Prev: Version>(
        tree: PoseidonTree<lazy_merkle_tree::Derived>,
        next_leaf: usize,
        prev: TreeVersion<Prev>,
    ) -> DerivedTreeBuilder<Prev> {
        let metadata = DerivedTreeMetadata { diff: vec![] };
        DerivedTreeBuilder {
            prev,
            current: TreeVersionData {
                tree,
                next_leaf,
                metadata,
                next: None,
            },
        }
    }

    /// Updates a leaf in the resulting tree.
    pub fn update(&mut self, update: &TreeUpdate) {
        self.current.update(update.leaf_index, update.element);
    }

    /// Seals this version and returns a builder for the next version.
    #[must_use]
    pub fn seal_and_continue(
        self,
    ) -> (TreeVersion<Intermediate>, DerivedTreeBuilder<Intermediate>) {
        let next_tree = self.current.tree.clone();
        let next_leaf = self.current.next_leaf;
        let sealed = TreeVersion(Arc::new(Mutex::new(self.current)));
        let next = Self::new(next_tree, next_leaf, sealed.clone());
        self.prev.get_data().next = Some(sealed.as_derived());
        (sealed, next)
    }

    /// Seals this version and finishes the building process.
    #[must_use]
    pub fn seal(self) -> TreeVersion<Latest> {
        let sealed = TreeVersion(Arc::new(Mutex::new(self.current)));
        self.prev.get_data().next = Some(sealed.as_derived());
        sealed
    }
}
