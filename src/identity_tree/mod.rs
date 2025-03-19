use std::fmt::Debug;
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use semaphore_rs::poseidon_tree::Proof;
use semaphore_rs::Field;
use semaphore_rs_hasher::Hasher;
use semaphore_rs_poseidon::Poseidon as PoseidonHash;
use semaphore_rs_trees::lazy;
use semaphore_rs_trees::lazy::LazyMerkleTree;
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;
use tracing::{info, warn};

pub mod builder;
pub mod db_sync;
pub mod initializer;
pub mod state;
mod status;

pub use self::state::TreeState;
pub use self::status::{ProcessedStatus, Status, UnknownStatus, UnprocessedStatus};

pub type PoseidonTree<Version> = LazyMerkleTree<PoseidonHash, Version>;
pub type Hash = <PoseidonHash as Hasher>::Hash;

#[derive(Clone, Eq, PartialEq, Hash, Debug, FromRow)]
pub struct TreeUpdate {
    #[sqlx(try_from = "i64")]
    pub sequence_id: usize,
    #[sqlx(try_from = "i64")]
    pub leaf_index: usize,
    pub element: Hash,
    pub post_root: Hash,
    pub received_at: Option<DateTime<Utc>>,
}

impl TreeUpdate {
    #[must_use]
    pub const fn new(
        sequence_id: usize,
        leaf_index: usize,
        element: Hash,
        post_root: Hash,
        received_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            sequence_id,
            leaf_index,
            element,
            post_root,
            received_at,
        }
    }
}

#[derive(Debug)]
pub struct TreeItem {
    pub sequence_id: usize,
    pub status: ProcessedStatus,
    pub leaf_index: usize,
    pub element: Hash,
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RootItem {
    pub root: Field,
    #[sqlx(try_from = "&'a str")]
    pub status: ProcessedStatus,
    pub pending_valid_as_of: chrono::DateTime<Utc>,
    pub mined_valid_as_of: Option<chrono::DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InclusionProof {
    pub status: Status,
    pub root: Option<Field>,
    pub proof: Option<Proof>,
    pub message: Option<String>,
}

/// Additional data held by the canonical tree version. It includes data
/// necessary to control garbage collection.
pub struct CanonicalTreeMetadata {
    flatten_threshold: usize,
    count_since_last_flatten: usize,
}

/// Additional data held by any derived tree version. Includes the list of
/// updates performed since previous version.
pub struct DerivedTreeMetadata {
    diff: Vec<AppliedTreeUpdate>,
    ref_state: TreeVersionState<lazy::Derived>,
}

#[derive(Clone)]
pub struct AppliedTreeUpdate {
    pub update: TreeUpdate,
    pub post_state: TreeVersionState<lazy::Derived>,
}

/// Trait used to associate a version marker with its metadata type.
pub trait AllowedTreeVersionMarker
where
    Self: lazy::VersionMarker,
{
    type Metadata;
}

impl AllowedTreeVersionMarker for lazy::Canonical {
    type Metadata = CanonicalTreeMetadata;
}

impl AllowedTreeVersionMarker for lazy::Derived {
    type Metadata = DerivedTreeMetadata;
}

pub struct TreeVersionState<V: AllowedTreeVersionMarker> {
    pub tree: PoseidonTree<V>,
    pub next_leaf: usize,
    pub last_sequence_id: usize,
}

impl TreeVersionState<lazy::Canonical> {
    fn derived(&self) -> TreeVersionState<lazy::Derived> {
        TreeVersionState {
            tree: self.tree.derived(),
            next_leaf: self.next_leaf,
            last_sequence_id: self.last_sequence_id,
        }
    }
}

impl Clone for TreeVersionState<lazy::Derived> {
    fn clone(&self) -> Self {
        TreeVersionState {
            tree: self.tree.clone(),
            next_leaf: self.next_leaf,
            last_sequence_id: self.last_sequence_id,
        }
    }
}

/// Underlying data structure for a tree version. It holds the tree itself, the
/// next leaf (only used in the latest tree), a last sequence id from database
/// indicating order of operations, a pointer to the next version (if exists)
/// and the metadata specified by the version marker.
struct TreeVersionData<V: AllowedTreeVersionMarker> {
    state: TreeVersionState<V>,
    next: Option<TreeVersion<AnyDerived>>,
    metadata: V::Metadata,
}

/// Basic operations that should be available for all tree versions.
pub trait BasicTreeOps {
    /// Updates the tree with the given element at the given leaf index.
    fn update(
        &mut self,
        sequence_id: usize,
        leaf_index: usize,
        element: Hash,
        received_at: Option<DateTime<Utc>>,
    );

    fn next_leaf(&self) -> usize;
    fn proof(&self, leaf_index: usize) -> (Hash, Proof);
    fn root(&self) -> Hash;

    fn apply_diffs(&mut self, diffs: Vec<AppliedTreeUpdate>);

    /// Notifies the tree that it was changed and can perform garbage
    /// collection. This is version-specific and it is up to the implementer to
    /// decide how to handle this signal.
    fn garbage_collect(&mut self);
}

impl<V> TreeVersionData<V>
where
    V: lazy::VersionMarker + AllowedTreeVersionMarker,
    Self: BasicTreeOps,
{
    /// Gets the current tree root.
    fn get_root(&self) -> Hash {
        self.state.tree.root()
    }

    /// Gets the leaf value at a given index.
    fn get_leaf(&self, leaf: usize) -> Hash {
        self.state.tree.get_leaf(leaf)
    }

    /// Gets the proof of the given leaf index element
    fn get_proof(&self, leaf: usize) -> (Hash, Proof) {
        let proof = self.state.tree.proof(leaf);
        (self.state.tree.root(), proof)
    }

    /// Returns _up to_ `maximum_update_count` contiguous deletion or insertion
    /// updates that are to be applied to the tree.
    fn peek_next_updates(&self, maximum_update_count: usize) -> Vec<AppliedTreeUpdate> {
        let Some(next) = self.next.as_ref() else {
            return Vec::new();
        };

        let next = next.get_data();

        let first_is_zero = match next.metadata.diff.first() {
            Some(first) => first.update.element == Hash::ZERO,
            None => return vec![],
        };

        // Gets the next contiguous of insertion or deletion updates from the diff
        let should_take = |elem: &&AppliedTreeUpdate| {
            if first_is_zero {
                // If first is zero, we should take only consecutive zeros
                elem.update.element == Hash::ZERO
            } else {
                // If first is not zero, we should take only non-zeros
                elem.update.element != Hash::ZERO
            }
        };

        next.metadata
            .diff
            .iter()
            .take_while(should_take)
            .take(maximum_update_count)
            .cloned()
            .collect()
    }

    /// Applies updates _up to_ `root`. Returns zero when root was not found.
    fn apply_updates_up_to(&mut self, root: Hash) -> usize {
        let Some(next) = self.next.clone() else {
            return 0;
        };

        let num_updates;
        {
            let applied_updates = {
                // Acquire the exclusive write lock on the next version.
                let mut next = next.get_data();

                let index_of_root = next
                    .metadata
                    .diff
                    .iter()
                    .position(|update| update.post_state.tree.root() == root);

                let Some(index_of_root) = index_of_root else {
                    warn!(?root, "Root not found in the diff");
                    return 0;
                };

                next.metadata
                    .diff
                    .drain(..=index_of_root)
                    .collect::<Vec<_>>()
            };

            num_updates = applied_updates.len();

            self.apply_diffs(applied_updates);
        }

        self.garbage_collect();

        num_updates
    }
}

impl TreeVersionData<lazy::Derived>
where
    Self: BasicTreeOps,
{
    /// Rewinds updates _up to_ `root`. Returns zero when root was not found.
    pub fn rewind_updates_up_to(&mut self, root: Hash) -> usize {
        let mut rest = if root == self.metadata.ref_state.tree.root() {
            self.state = self.metadata.ref_state.clone();

            self.metadata.diff.drain(..).collect::<Vec<_>>()
        } else {
            let index_of_root = self
                .metadata
                .diff
                .iter()
                .position(|update| update.post_state.tree.root() == root);

            let Some(index_of_root) = index_of_root else {
                warn!(?root, "Root not found in the diff");
                return 0;
            };

            let Some(root_update) = self.metadata.diff.get(index_of_root) else {
                warn!(?root, "Root position not found in the diff");
                return 0;
            };

            self.state = root_update.post_state.clone();

            self.metadata
                .diff
                .drain((index_of_root + 1)..)
                .collect::<Vec<_>>()
        };

        let num_updates = rest.len();

        if let Some(next) = self.next.clone() {
            let mut next = next.get_data();
            rest.append(&mut next.metadata.diff);
            next.metadata.diff = rest;
            next.metadata.ref_state = self.state.clone();

            next.garbage_collect();
        };

        self.garbage_collect();

        num_updates
    }
}

impl BasicTreeOps for TreeVersionData<lazy::Canonical> {
    fn update(
        &mut self,
        sequence_id: usize,
        leaf_index: usize,
        element: Hash,
        _: Option<DateTime<Utc>>,
    ) {
        take_mut::take(&mut self.state.tree, |tree| {
            tree.update_with_mutation(leaf_index, &element)
        });
        if element != Hash::ZERO {
            self.state.next_leaf = leaf_index + 1;
        }
        self.metadata.count_since_last_flatten += 1;
        self.state.last_sequence_id = sequence_id;
    }

    fn next_leaf(&self) -> usize {
        self.state.next_leaf
    }

    fn proof(&self, leaf_index: usize) -> (Hash, Proof) {
        let proof = self.state.tree.proof(leaf_index);
        (self.state.tree.root(), proof)
    }

    fn root(&self) -> Hash {
        self.state.tree.root()
    }

    fn apply_diffs(&mut self, diffs: Vec<AppliedTreeUpdate>) {
        for applied_update in &diffs {
            let update = &applied_update.update;
            self.update(
                update.sequence_id,
                update.leaf_index,
                update.element,
                update.received_at,
            );
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
                next.get_data().rebuild_on(self.state.tree.derived());
            }
            info!("Tree versions rebuilt");
        }
    }
}

impl TreeVersionData<lazy::Derived> {
    /// This method recalculate tree to use different tree version as a base.
    /// The tree itself is same in terms of root hash but differs how
    /// internally is stored in memory and on disk.
    fn rebuild_on(&mut self, mut tree: PoseidonTree<lazy::Derived>) {
        self.metadata.ref_state.tree = tree.clone();
        for update in &mut self.metadata.diff {
            tree = tree.update(update.update.leaf_index, &update.update.element);
            update.post_state.tree = tree.clone();
        }
        self.state.tree = tree.clone();
        let next = &self.next;
        if let Some(next) = next {
            next.get_data().rebuild_on(self.state.tree.clone());
        }
    }
}

impl BasicTreeOps for TreeVersionData<lazy::Derived> {
    fn update(
        &mut self,
        sequence_id: usize,
        leaf_index: usize,
        element: Hash,
        received_at: Option<DateTime<Utc>>,
    ) {
        let updated_tree = self.state.tree.update(leaf_index, &element);
        let updated_next_leaf = if element != Hash::ZERO {
            leaf_index + 1
        } else {
            self.state.next_leaf
        };

        self.state = TreeVersionState {
            tree: updated_tree.clone(),
            next_leaf: updated_next_leaf,
            last_sequence_id: sequence_id,
        };
        self.metadata.diff.push(AppliedTreeUpdate {
            update: TreeUpdate {
                sequence_id,
                leaf_index,
                element,
                post_root: updated_tree.root(),
                received_at,
            },
            post_state: self.state.clone(),
        });

        if let Some(next) = &self.next {
            let mut next = next.get_data();
            next.metadata.ref_state = self.state.clone();
        }
    }

    fn next_leaf(&self) -> usize {
        self.state.next_leaf
    }

    fn proof(&self, leaf_index: usize) -> (Hash, Proof) {
        let proof = self.state.tree.proof(leaf_index);
        (self.state.tree.root(), proof)
    }

    fn root(&self) -> Hash {
        self.state.tree.root()
    }

    fn apply_diffs(&mut self, mut diffs: Vec<AppliedTreeUpdate>) {
        let last = diffs.last().cloned();

        self.metadata.diff.append(&mut diffs);

        if let Some(last) = last {
            self.state = last.post_state.clone();
        }

        if let Some(next) = &self.next {
            let mut next = next.get_data();
            next.metadata.ref_state = self.state.clone();
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
    type TreeVersion = lazy::Canonical;
}
impl HasNextVersion for Canonical {}

/// Marker for an intermediate version – one that has both a predecessor and a
/// successor.
#[derive(Clone)]
pub struct Intermediate;
impl Version for Intermediate {
    type TreeVersion = lazy::Derived;
}
impl HasNextVersion for Intermediate {}

/// Marker for the latest tree version – one that has no successor. It enables a
/// different API, focusing on outside updates, rather than just pulling in
/// updates from the successor.
#[derive(Clone)]
pub struct Latest;
impl Version for Latest {
    type TreeVersion = lazy::Derived;
}

/// Marker for any tree version that has a predecessor. It is useful internally
/// for storage inside `TreeVersionData`, but it is probably not going to be
/// useful for any clients.
#[derive(Clone)]
struct AnyDerived;
impl Version for AnyDerived {
    type TreeVersion = lazy::Derived;
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

impl<V: Version<TreeVersion = lazy::Derived>> TreeVersion<V> {
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
    /// Returns the given leaf value, the root of the tree and the proof
    fn get_leaf_and_proof(&self, leaf: usize) -> (Hash, Hash, Proof);
    /// Returns the merkle proof and element at the given leaf.
    fn get_proof(&self, leaf: usize) -> (Hash, Proof);
    /// Gets the leaf value at a given index.
    fn get_leaf(&self, leaf: usize) -> Hash;
    /// Gets commitments at given leaf values
    fn commitments_by_leaves(&self, leaves: impl IntoIterator<Item = usize>) -> Vec<Hash>;
    /// Gets last sequence id
    fn get_last_sequence_id(&self) -> usize;
}

impl<V: Version> TreeVersionReadOps for TreeVersion<V>
where
    TreeVersionData<V::TreeVersion>: BasicTreeOps,
{
    fn get_root(&self) -> Hash {
        self.get_data().get_root()
    }

    fn next_leaf(&self) -> usize {
        self.get_data().state.next_leaf
    }

    fn get_leaf_and_proof(&self, leaf: usize) -> (Hash, Hash, Proof) {
        let tree = self.get_data();

        let (root, proof) = tree.get_proof(leaf);
        let leaf = tree.get_leaf(leaf);

        (leaf, root, proof)
    }

    fn get_proof(&self, leaf: usize) -> (Hash, Proof) {
        let tree = self.get_data();
        tree.get_proof(leaf)
    }

    fn get_leaf(&self, leaf: usize) -> Hash {
        let tree = self.get_data();
        tree.get_leaf(leaf)
    }

    fn commitments_by_leaves(&self, leaves: impl IntoIterator<Item = usize>) -> Vec<Hash> {
        let tree = self.get_data();

        let mut commitments = vec![];

        for leaf in leaves {
            commitments.push(tree.state.tree.get_leaf(leaf));
        }

        commitments
    }

    fn get_last_sequence_id(&self) -> usize {
        self.get_data().state.last_sequence_id
    }
}

impl<V: Version> TreeVersion<V> {
    fn get_data(&self) -> MutexGuard<TreeVersionData<V::TreeVersion>> {
        self.0.lock().expect("no lock poisoning")
    }
}

impl TreeVersion<Latest> {
    /// Simulate appending many identities to the tree by copying it underneath,
    /// returns a list with the root, proof of inclusion and leaf index. No
    /// changes are made to the tree.
    #[must_use]
    pub fn simulate_append_many(&self, identities: &[Hash]) -> Vec<(Hash, Proof, usize)> {
        let data = self.get_data();
        let mut tree = data.state.tree.clone();
        let next_leaf = data.state.next_leaf;

        let mut output = Vec::with_capacity(identities.len());

        for (idx, identity) in identities.iter().enumerate() {
            let leaf_index = next_leaf + idx;

            tree = tree.update(leaf_index, identity);
            let root = tree.root();
            let proof = tree.proof(leaf_index);

            output.push((root, proof, leaf_index));
        }

        output
    }

    /// Simulates deleting many identities from the tree by copying it
    /// underneath, returns a list with the root and proof of inclusion. No
    /// changes are made to the tree.
    #[must_use]
    pub fn simulate_delete_many(&self, leaf_indices: &[usize]) -> Vec<(Hash, Proof)> {
        let mut tree = self.get_data().state.tree.clone();

        let mut output = Vec::with_capacity(leaf_indices.len());

        for leaf_index in leaf_indices {
            tree = tree.update(*leaf_index, &Hash::ZERO);
            let root = tree.root();
            let proof = tree.proof(*leaf_index);

            output.push((root, proof));
        }

        output
    }

    /// Latest tree is the only way to apply new updates. Other versions may
    /// only move on the chain of changes by passing desired root.
    pub fn apply_updates(&self, tree_updates: &[TreeUpdate]) -> Vec<Hash> {
        let mut data = self.get_data();

        let mut output = Vec::with_capacity(tree_updates.len());

        for tree_update in tree_updates {
            data.update(
                tree_update.sequence_id,
                tree_update.leaf_index,
                tree_update.element,
                tree_update.received_at,
            );
            output.push(data.get_root());
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

/// Public API for working with versions that can rollback updates. Rollback is
/// only possible up to previous tree root.
pub trait ReversibleVersion {
    fn rewind_updates_up_to(&self, root: Hash) -> usize;
}

impl<V: Version<TreeVersion = lazy::Derived>> ReversibleVersion for TreeVersion<V> {
    fn rewind_updates_up_to(&self, root: Hash) -> usize {
        self.get_data().rewind_updates_up_to(root)
    }
}

#[cfg(test)]
mod tests {
    use super::builder::CanonicalTreeBuilder;
    use super::{Hash, ReversibleVersion, TreeUpdate, TreeVersionReadOps, TreeWithNextVersion};
    use chrono::Utc;

    #[test]
    fn test_peek_next_updates() {
        let temp_dir = tempfile::tempdir().unwrap();

        let (canonical_tree, processed_builder) = CanonicalTreeBuilder::new(
            10,
            10,
            0,
            Hash::ZERO,
            &[],
            temp_dir.path().join("testfile").to_str().unwrap(),
        )
        .seal();
        let processed_tree = processed_builder.seal();

        let insertions = [
            Hash::from(1),
            Hash::from(2),
            Hash::from(3),
            Hash::from(4),
            Hash::from(5),
            Hash::from(6),
            Hash::from(7),
        ];
        let updates = processed_tree.simulate_append_many(&insertions);
        let insertion_updates = (0..7)
            .zip(updates)
            .map(|(i, (root, _, leaf_index))| {
                TreeUpdate::new(
                    i,
                    leaf_index,
                    *insertions.get(i).unwrap(),
                    root,
                    Some(Utc::now()),
                )
            })
            .collect::<Vec<_>>();
        _ = processed_tree.apply_updates(&insertion_updates);

        let deletions = [0, 1, 2];
        let updates = processed_tree.simulate_delete_many(&deletions);
        let deletion_updates = (7..10)
            .zip(updates)
            .map(|(i, (root, _))| {
                TreeUpdate::new(
                    i,
                    *deletions.get(i - 7).unwrap(),
                    Hash::ZERO,
                    root,
                    Some(Utc::now()),
                )
            })
            .collect::<Vec<_>>();
        _ = processed_tree.apply_updates(&deletion_updates);

        let next_updates = canonical_tree.peek_next_updates(10);
        assert_eq!(next_updates.len(), 7);

        canonical_tree.apply_updates_up_to(
            next_updates
                .last()
                .expect("Could not get insertion updates")
                .update
                .post_root,
        );

        let insertions = [Hash::from(8), Hash::from(9), Hash::from(10), Hash::from(11)];
        let updates = processed_tree.simulate_append_many(&insertions);
        let insertion_updates = (10..14)
            .zip(updates)
            .map(|(i, (root, _, leaf_index))| {
                TreeUpdate::new(
                    i,
                    leaf_index,
                    *insertions.get(i - 10).unwrap(),
                    root,
                    Some(Utc::now()),
                )
            })
            .collect::<Vec<_>>();
        let _ = processed_tree.apply_updates(&insertion_updates);

        let next_updates = canonical_tree.peek_next_updates(10);

        assert_eq!(next_updates.len(), 3);
    }

    #[test]
    fn test_rewind_up_to_root() {
        let temp_dir = tempfile::tempdir().unwrap();

        let (processed_tree, batching_tree_builder) = CanonicalTreeBuilder::new(
            10,
            10,
            0,
            Hash::ZERO,
            &[],
            temp_dir.path().join("testfile").to_str().unwrap(),
        )
        .seal();
        let (batching_tree, latest_tree_builder) = batching_tree_builder.seal_and_continue();
        let latest_tree = latest_tree_builder.seal();

        let insertions = (1..=30).map(Hash::from).collect::<Vec<_>>();
        let updates = latest_tree.simulate_append_many(&insertions);
        let insertion_updates = (0..30)
            .zip(updates)
            .map(|(i, (root, _, leaf_index))| {
                TreeUpdate::new(
                    i,
                    leaf_index,
                    *insertions.get(i).unwrap(),
                    root,
                    Some(Utc::now()),
                )
            })
            .collect::<Vec<_>>();
        _ = latest_tree.apply_updates(&insertion_updates);

        batching_tree.apply_updates_up_to(insertion_updates.get(19).unwrap().post_root);
        processed_tree.apply_updates_up_to(insertion_updates.get(9).unwrap().post_root);

        assert_eq!(processed_tree.next_leaf(), 10);
        assert_eq!(
            processed_tree.get_root(),
            insertion_updates.get(9).unwrap().post_root
        );
        assert_eq!(batching_tree.next_leaf(), 20);
        assert_eq!(
            batching_tree.get_root(),
            insertion_updates.get(19).unwrap().post_root
        );
        assert_eq!(latest_tree.next_leaf(), 30);
        assert_eq!(
            latest_tree.get_root(),
            insertion_updates.get(29).unwrap().post_root
        );

        batching_tree.rewind_updates_up_to(insertion_updates.get(15).unwrap().post_root);
        latest_tree.rewind_updates_up_to(insertion_updates.get(25).unwrap().post_root);

        assert_eq!(processed_tree.next_leaf(), 10);
        assert_eq!(
            processed_tree.get_root(),
            insertion_updates.get(9).unwrap().post_root
        );
        assert_eq!(batching_tree.next_leaf(), 16);
        assert_eq!(
            batching_tree.get_root(),
            insertion_updates.get(15).unwrap().post_root
        );
        assert_eq!(latest_tree.next_leaf(), 26);
        assert_eq!(
            latest_tree.get_root(),
            insertion_updates.get(25).unwrap().post_root
        );

        latest_tree.rewind_updates_up_to(insertion_updates.get(15).unwrap().post_root);

        assert_eq!(processed_tree.next_leaf(), 10);
        assert_eq!(
            processed_tree.get_root(),
            insertion_updates.get(9).unwrap().post_root
        );
        assert_eq!(batching_tree.next_leaf(), 16);
        assert_eq!(
            batching_tree.get_root(),
            insertion_updates.get(15).unwrap().post_root
        );
        assert_eq!(latest_tree.next_leaf(), 16);
        assert_eq!(
            latest_tree.get_root(),
            insertion_updates.get(15).unwrap().post_root
        );
    }
}
