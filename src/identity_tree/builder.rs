use std::cmp::min;
use std::sync::{Arc, Mutex};

use semaphore_rs::Field;
use semaphore_rs_poseidon::Poseidon as PoseidonHash;
use semaphore_rs_trees::lazy::{self, LazyMerkleTree};
use tracing::{info, warn};

use crate::identity_tree::{
    BasicTreeOps, Canonical, CanonicalTreeMetadata, DerivedTreeMetadata, Hash, Intermediate,
    Latest, PoseidonTree, TreeUpdate, TreeVersion, TreeVersionData, TreeVersionState, Version,
};

/// A helper for building the first tree version. Exposes a type-safe API over
/// building a sequence of tree versions efficiently.
pub struct CanonicalTreeBuilder(TreeVersionData<lazy::Canonical>);

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
    /// * `initial_leaf`: The default value of the tree leaves.
    /// * `initial_leaves`: The initial values of the tree leaves. Index in
    ///   array is a leaf index in the tree.
    /// * `mmap_file_path`: Path to file where data are stored on disk.
    #[must_use]
    pub fn new(
        tree_depth: usize,
        dense_prefix_depth: usize,
        flattening_threshold: usize,
        default_leaf: Field,
        initial_leaves: &[Option<TreeUpdate>],
        mmap_file_path: &str,
    ) -> Self {
        info!("Creating new tree");
        let initial_leaves_in_dense_count = min(initial_leaves.len(), 1 << dense_prefix_depth);
        let (initial_leaves_in_dense, leftover_initial_leaves) =
            initial_leaves.split_at(initial_leaves_in_dense_count);

        info!("Creating mmap dense tree");
        let tree = PoseidonTree::<lazy::Canonical>::new_mmapped_with_dense_prefix_with_init_values(
            tree_depth,
            dense_prefix_depth,
            &default_leaf,
            &initial_leaves_in_dense
                .iter()
                .map(|tree_update| {
                    tree_update
                        .as_ref()
                        .map(|v| v.element)
                        .unwrap_or(default_leaf)
                })
                .collect::<Vec<Field>>(),
            mmap_file_path,
        )
        .unwrap();

        info!("Applying leaves not in dense tree");
        let metadata = CanonicalTreeMetadata {
            flatten_threshold: flattening_threshold,
            count_since_last_flatten: 0,
        };

        let last_dense_leaf = initial_leaves_in_dense
            .iter()
            .last()
            .unwrap_or(&None)
            .as_ref();
        let mut builder = Self(TreeVersionData {
            state: TreeVersionState {
                tree,
                next_leaf: last_dense_leaf.map(|v| v.leaf_index + 1).unwrap_or(0),
                last_sequence_id: last_dense_leaf.map(|v| v.sequence_id).unwrap_or(0),
            },
            next: None,
            metadata,
        });
        let last_index = leftover_initial_leaves.len();
        for (i, tree_update) in leftover_initial_leaves.iter().flatten().enumerate() {
            if i % 10000 == 0 {
                info!("Current leaf index {i}/{last_index}");
            }
            builder.update(tree_update);
        }

        info!("Tree created");
        builder
    }

    pub fn restore_dense(
        tree_depth: usize,
        dense_prefix_depth: usize,
        default_leaf: &Field,
        flattening_threshold: usize,
        mmap_file_path: &str,
    ) -> Option<RestoredCanonicalTreeBuilder> {
        info!("Restoring dense tree from file");
        let tree: LazyMerkleTree<PoseidonHash, lazy::Canonical> =
            match PoseidonTree::<lazy::Canonical>::attempt_dense_mmap_restore(
                tree_depth,
                dense_prefix_depth,
                default_leaf,
                mmap_file_path,
            ) {
                Ok(tree) => tree,
                Err(error) => {
                    warn!("Dense tree wasn't restored. Reason: {}", error.to_string());
                    return None;
                }
            };

        let metadata = CanonicalTreeMetadata {
            flatten_threshold: flattening_threshold,
            count_since_last_flatten: 0,
        };

        let builder = RestoredCanonicalTreeBuilder { tree, metadata };

        info!("Dense tree restored");
        Some(builder)
    }

    /// Updates a leaf in the resulting tree.
    pub fn update(&mut self, update: &TreeUpdate) {
        self.0.update(
            update.sequence_id,
            update.leaf_index,
            update.element,
            update.received_at,
        );
    }

    /// Seals this version and returns a builder for the next version.
    #[must_use]
    pub fn seal(self) -> (TreeVersion<Canonical>, DerivedTreeBuilder<Canonical>) {
        let state = self.0.state.derived();
        let sealed = TreeVersion(Arc::new(Mutex::new(self.0)));
        let next = DerivedTreeBuilder::<Canonical>::new(state, sealed.clone());
        (sealed, next)
    }
}

pub struct RestoredCanonicalTreeBuilder {
    pub tree: PoseidonTree<lazy::Canonical>,
    pub metadata: CanonicalTreeMetadata,
}

impl RestoredCanonicalTreeBuilder {
    pub fn with_leaf(self, next_leaf: usize, last_sequence_id: usize) -> CanonicalTreeBuilder {
        CanonicalTreeBuilder(TreeVersionData {
            state: TreeVersionState {
                tree: self.tree,
                next_leaf,
                last_sequence_id,
            },
            next: None,
            metadata: self.metadata,
        })
    }

    pub fn root(&self) -> Hash {
        self.tree.root()
    }
}

/// A helper for building successive tree versions. Exposes a type-safe API over
/// building a sequence of tree versions efficiently.
pub struct DerivedTreeBuilder<P: Version> {
    prev: TreeVersion<P>,
    current: TreeVersionData<lazy::Derived>,
}

impl<P: Version> DerivedTreeBuilder<P> {
    #[must_use]
    fn new<Prev: Version>(
        state: TreeVersionState<lazy::Derived>,
        prev: TreeVersion<Prev>,
    ) -> DerivedTreeBuilder<Prev> {
        let metadata = DerivedTreeMetadata {
            diff: vec![],
            ref_state: state.clone(),
        };
        DerivedTreeBuilder {
            prev,
            current: TreeVersionData {
                state,
                next: None,
                metadata,
            },
        }
    }

    /// Updates a leaf in the resulting tree.
    pub fn update(&mut self, update: &TreeUpdate) {
        self.current.update(
            update.sequence_id,
            update.leaf_index,
            update.element,
            update.received_at,
        );
    }

    /// Seals this version and returns a builder for the next version.
    #[must_use]
    pub fn seal_and_continue(
        self,
    ) -> (TreeVersion<Intermediate>, DerivedTreeBuilder<Intermediate>) {
        let state = self.current.state.clone();
        let sealed = TreeVersion(Arc::new(Mutex::new(self.current)));
        let next = Self::new(state, sealed.clone());
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
