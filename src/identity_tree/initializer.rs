use std::sync::Arc;
use std::time::Instant;

use semaphore::poseidon_tree::LazyPoseidonTree;
use tracing::{info, instrument, warn};

use crate::config::TreeConfig;
use crate::database::methods::DbMethods;
use crate::database::Database;
use crate::identity::processor::IdentityProcessor;
use crate::identity_tree::{
    CanonicalTreeBuilder, Hash, ProcessedStatus, TreeState, TreeUpdate, TreeVersionReadOps,
    TreeWithNextVersion,
};
use crate::utils::tree_updates::dedup_tree_updates;

pub struct TreeInitializer {
    pub database: Arc<Database>,
    pub identity_processor: Arc<dyn IdentityProcessor>,
    pub config: TreeConfig,
}

impl TreeInitializer {
    pub fn new(
        database: Arc<Database>,
        identity_processor: Arc<dyn IdentityProcessor>,
        config: TreeConfig,
    ) -> Self {
        Self {
            database,
            identity_processor,
            config,
        }
    }

    /// Initializes the tree state. This should only ever be called once.
    /// Attempts to call this method more than once will result in a panic.
    pub async fn run(self) -> anyhow::Result<TreeState> {
        // Await for all pending transactions
        self.identity_processor.await_clean_slate().await?;

        let initial_root_hash =
            LazyPoseidonTree::new(self.config.tree_depth, self.config.initial_leaf_value).root();

        self.identity_processor
            .tree_init_correction(&initial_root_hash)
            .await?;

        let timer = Instant::now();
        info!("Tree state initialization started");
        let tree_state = self
            .restore_or_initialize_tree(initial_root_hash, self.config.force_cache_purge)
            .await?;
        info!("Tree state initialization took: {:?}", timer.elapsed());

        let tree_root = tree_state.get_processed_tree().get_root();
        match self.identity_processor.latest_root().await? {
            Some(root) if root == tree_root => Ok(tree_state),
            None if initial_root_hash == tree_root => Ok(tree_state),
            _ => {
                warn!(
                    "Cached tree root is different from the contract root. Purging cache and \
                 reinitializing."
                );

                let timer = Instant::now();
                info!("Tree state initialization started");
                let tree_state = self
                    .restore_or_initialize_tree(initial_root_hash, true)
                    .await?;
                info!("Tree state initialization took: {:?}", timer.elapsed());

                Ok(tree_state)
            }
        }
    }

    #[instrument(skip(self))]
    async fn restore_or_initialize_tree(
        &self,
        initial_root_hash: Hash,
        force_cache_purge: bool,
    ) -> anyhow::Result<TreeState> {
        info!("Getting mined commitments from DB");
        let mut mined_items = self
            .database
            .get_commitments_by_status(ProcessedStatus::Mined)
            .await?;

        mined_items.sort_by_key(|item| item.leaf_index);

        let mined_items = dedup_tree_updates(mined_items);

        info!("Retrieved {} mined commitments from DB", mined_items.len());

        if !force_cache_purge {
            info!(
                "Attempting to restore tree from cache (file={})",
                &self.config.cache_file
            );
            if let Some(tree_state) = self
                .get_cached_tree_state(&mined_items, initial_root_hash)
                .await?
            {
                info!("tree restored from cache");
                return Ok(tree_state);
            }
        }

        info!("Initializing tree from the database");
        let tree_state = self.initialize_tree(mined_items).await?;

        info!("tree initialization successful");

        Ok(tree_state)
    }

    pub fn get_leftover_leaves_and_update_index(
        index: &mut Option<usize>,
        dense_prefix_depth: usize,
        mined_items: &[TreeUpdate],
    ) -> Vec<ruint::Uint<256, 4>> {
        let leftover_items = if mined_items.is_empty() {
            vec![]
        } else {
            let max_leaf = mined_items.last().map(|item| item.leaf_index).unwrap();
            // if the last index is greater than dense_prefix_depth, 1 << dense_prefix_depth
            // should be the last index in restored tree
            let last_index = std::cmp::min(max_leaf, (1 << dense_prefix_depth) - 1);
            *index = Some(last_index);

            info!("calculated last dense tree index: {}", last_index);

            if max_leaf - last_index == 0 {
                return vec![];
            }

            let mut leaves = Vec::with_capacity(max_leaf - last_index);

            let leftover = &mined_items[(last_index + 1)..];

            for item in leftover {
                leaves.push(item.element);
            }

            leaves
        };

        info!(
            "number of leaves not included in the dense tree: {}",
            leftover_items.len()
        );

        leftover_items
    }

    async fn get_cached_tree_state(
        &self,
        mined_items: &[TreeUpdate],
        initial_root_hash: Hash,
    ) -> anyhow::Result<Option<TreeState>> {
        info!("Restoring tree from cache");

        let mut last_mined_index_in_dense: Option<usize> = None;
        let leftover_items = Self::get_leftover_leaves_and_update_index(
            &mut last_mined_index_in_dense,
            self.config.dense_tree_prefix_depth,
            mined_items,
        );

        info!("Restoring canonical mined tree");

        let Some(mined_builder) = CanonicalTreeBuilder::restore(
            self.config.tree_depth,
            self.config.dense_tree_prefix_depth,
            &self.config.initial_leaf_value,
            last_mined_index_in_dense,
            &leftover_items,
            self.config.tree_gc_threshold,
            &self.config.cache_file,
        ) else {
            return Ok(None);
        };

        let (mined, mut processed_builder) = mined_builder.seal();

        match self
            .database
            .get_latest_root_by_status(ProcessedStatus::Mined)
            .await?
        {
            Some(root) => {
                if !mined.get_root().eq(&root) {
                    info!("Cached tree root differs from root in database");
                    return Ok(None);
                }
            }
            None => {
                if !mined.get_root().eq(&initial_root_hash) {
                    info!("Cached tree root differs from init root");
                    return Ok(None);
                }
            }
        }

        info!("Restoring derived processed and batching tree");

        let processed_items = self
            .database
            .get_commitments_by_status(ProcessedStatus::Processed)
            .await?;

        for processed_item in processed_items {
            processed_builder.update(&processed_item);
        }

        let (processed, batching_builder) = processed_builder.seal_and_continue();
        let (batching, mut latest_builder) = batching_builder.seal_and_continue();

        info!("Restoring derived latest tree");

        let pending_items = self
            .database
            .get_commitments_by_status(ProcessedStatus::Pending)
            .await?;
        for update in pending_items {
            latest_builder.update(&update);
        }
        let latest = latest_builder.seal();

        info!("Batching tree correction");

        let batch = self.database.get_latest_batch().await?;
        if let Some(batch) = batch {
            if batching.get_root() != batch.next_root {
                batching.apply_updates_up_to(batch.next_root);
            }
            assert_eq!(batching.get_root(), batch.next_root);
        }

        info!("Tree restored.");

        Ok(Some(TreeState::new(mined, processed, batching, latest)))
    }

    #[instrument(skip_all)]
    async fn initialize_tree(&self, mined_items: Vec<TreeUpdate>) -> anyhow::Result<TreeState> {
        info!("Creating tree from the database");

        let initial_leaf_value = self.config.initial_leaf_value;

        let initial_leaves = if mined_items.is_empty() {
            vec![]
        } else {
            let max_leaf = mined_items.last().map(|item| item.leaf_index).unwrap();
            let mut leaves = vec![initial_leaf_value; max_leaf + 1];

            for item in mined_items {
                leaves[item.leaf_index] = item.element;
            }

            leaves
        };

        let tree_depth = self.config.tree_depth;
        let dense_tree_prefix_depth = self.config.dense_tree_prefix_depth;
        let tree_gc_threshold = self.config.tree_gc_threshold;
        let cache_file = self.config.cache_file.clone();

        info!("Creating canonical mined tree");

        let mined_builder = tokio::task::spawn_blocking(move || {
            CanonicalTreeBuilder::new(
                tree_depth,
                dense_tree_prefix_depth,
                tree_gc_threshold,
                initial_leaf_value,
                &initial_leaves,
                &cache_file,
            )
        })
        .await?;

        let (mined, mut processed_builder) = mined_builder.seal();

        info!("Creating derived processed and batching tree");

        let processed_items = self
            .database
            .get_commitments_by_status(ProcessedStatus::Processed)
            .await?;

        let processed_builder = tokio::task::spawn_blocking(move || {
            for processed_item in processed_items {
                processed_builder.update(&processed_item);
            }

            processed_builder
        })
        .await?;

        let (processed, batching_builder) = processed_builder.seal_and_continue();
        let (batching, mut latest_builder) = batching_builder.seal_and_continue();

        info!("Creating derived latest tree");

        let pending_items = self
            .database
            .get_commitments_by_status(ProcessedStatus::Pending)
            .await?;

        let latest_builder = tokio::task::spawn_blocking(move || {
            for update in pending_items {
                latest_builder.update(&update);
            }

            latest_builder
        })
        .await?;

        let latest = latest_builder.seal();

        info!("Batching tree correction");
        let batch = self.database.get_latest_batch().await?;
        if let Some(batch) = batch {
            if batching.get_root() != batch.next_root {
                batching.apply_updates_up_to(batch.next_root);
            }
            assert_eq!(batching.get_root(), batch.next_root);
        }

        info!("Tree created.");

        Ok(TreeState::new(mined, processed, batching, latest))
    }
}

#[cfg(test)]
mod test {
    use ethers::prelude::rand;
    use ethers::types::U256;
    use ruint::Uint;

    use crate::identity_tree::initializer::TreeInitializer;
    use crate::identity_tree::TreeUpdate;

    pub fn generate_test_identities_with_index(identity_count: usize) -> Vec<TreeUpdate> {
        generate_test_identities_with_range(0, identity_count)
    }

    pub fn generate_test_identities_with_range(start: usize, end: usize) -> Vec<TreeUpdate> {
        let mut identities = vec![];

        for i in start..end {
            let bytes: [u8; 32] = U256::from(rand::random::<u64>()).into();
            let identity = Uint::<256, 4>::from_le_bytes(bytes);

            identities.push(TreeUpdate {
                leaf_index: i,
                element: identity,
            });
        }

        identities
    }

    #[tokio::test]
    async fn test_index_logic_for_cached_tree() -> anyhow::Result<()> {
        // supports 8 identities (2^3)
        let dense_prefix_depth: usize = 3;

        let less_identities_count = 2usize.pow(dense_prefix_depth.try_into().unwrap()) - 2;
        let more_identities_count = 2usize.pow(dense_prefix_depth.try_into().unwrap()) + 2;

        // test if empty case is handled correctly (it means no last mined index as no
        // indices at all)
        let identities: Vec<TreeUpdate> = vec![];

        let mut last_mined_index_in_dense: Option<usize> = None;
        let leaves = TreeInitializer::get_leftover_leaves_and_update_index(
            &mut last_mined_index_in_dense,
            dense_prefix_depth,
            &identities,
        );

        // check if the index is correct
        assert_eq!(last_mined_index_in_dense, None);

        // since there are no identities at all the leaves should be 0
        assert_eq!(leaves.len(), 0);

        // first test with less than dense prefix
        let identities = generate_test_identities_with_index(less_identities_count);

        last_mined_index_in_dense = None;

        let leaves = TreeInitializer::get_leftover_leaves_and_update_index(
            &mut last_mined_index_in_dense,
            dense_prefix_depth,
            &identities,
        );

        // check if the index is correct
        assert_eq!(last_mined_index_in_dense, Some(identities.len() - 1));
        // since there are fewer identities than dense prefix, the leaves should be empty
        // vector
        assert!(leaves.is_empty());

        // let's try now with more identities than dense prefix supports

        // this should generate 2^dense_prefix + 2
        let identities = generate_test_identities_with_index(more_identities_count);

        last_mined_index_in_dense = None;
        let leaves = TreeInitializer::get_leftover_leaves_and_update_index(
            &mut last_mined_index_in_dense,
            dense_prefix_depth,
            &identities,
        );

        // check if the index is correct
        assert_eq!(
            last_mined_index_in_dense,
            Some((1 << dense_prefix_depth) - 1)
        );

        // since there are more identities than dense prefix, the leaves should be 2
        assert_eq!(leaves.len(), 2);

        // additional check for correctness
        assert_eq!(leaves[0], identities[8].element);
        assert_eq!(leaves[1], identities[9].element);

        Ok(())
    }

    #[tokio::test]
    async fn test_index_logic_with_leaf_gap_1() -> anyhow::Result<()> {
        // test if empty case is handled correctly (it means no last mined index as no
        // indices at all)

        // supports 8 identities (2^3)
        let dense_prefix_depth: usize = 3;

        let identities: Vec<TreeUpdate> = vec![];

        let mut last_mined_index_in_dense: Option<usize> = None;
        let leaves = TreeInitializer::get_leftover_leaves_and_update_index(
            &mut last_mined_index_in_dense,
            dense_prefix_depth,
            &identities,
        );

        // check if the index is correct
        assert_eq!(last_mined_index_in_dense, None);

        // since there are no identities at all the leaves should be 0
        assert_eq!(leaves.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_index_logic_with_leaf_gap_2() -> anyhow::Result<()> {
        // first test with less than dense prefix

        // supports 8 identities (2^3)
        let dense_prefix_depth: usize = 3;

        let mut identities = generate_test_identities_with_range(0, 2);
        identities.append(&mut generate_test_identities_with_range(4, 6));

        let mut last_mined_index_in_dense: Option<usize> = None;
        let leaves = TreeInitializer::get_leftover_leaves_and_update_index(
            &mut last_mined_index_in_dense,
            dense_prefix_depth,
            &identities,
        );

        // check if the index is correct
        assert_eq!(last_mined_index_in_dense, Some(5));
        // since there are fewer identities than dense prefix, the leaves should be empty
        // vector
        assert!(leaves.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_index_logic_with_leaf_gap_3() -> anyhow::Result<()> {
        // let's try now with more identities than dense prefix supports

        // supports 8 identities (2^3)
        let dense_prefix_depth: usize = 3;

        // this should generate 2^dense_prefix + 2
        let mut identities = generate_test_identities_with_range(0, 2);
        identities.append(&mut generate_test_identities_with_range(4, 12));

        let mut last_mined_index_in_dense: Option<usize> = None;
        let leaves = TreeInitializer::get_leftover_leaves_and_update_index(
            &mut last_mined_index_in_dense,
            dense_prefix_depth,
            &identities,
        );

        // check if the index is correct
        assert_eq!(
            last_mined_index_in_dense,
            Some((1 << dense_prefix_depth) - 1)
        );

        // since there are more identities than dense prefix, the leaves should be 2
        assert_eq!(leaves.len(), 4);

        // additional check for correctness
        assert_eq!(leaves[0], identities[6].element);
        assert_eq!(leaves[1], identities[7].element);
        assert_eq!(leaves[2], identities[8].element);
        assert_eq!(leaves[3], identities[9].element);

        Ok(())
    }

    #[tokio::test]
    async fn test_index_logic_with_leaf_gap_4() -> anyhow::Result<()> {
        // let's try now with more identities that should fit in dense prefix but won't due to leaf index

        // supports 8 identities (2^3)
        let dense_prefix_depth: usize = 3;

        // this should generate 2^dense_prefix + 2
        let mut identities = generate_test_identities_with_range(0, 2);
        identities.append(&mut generate_test_identities_with_range(8, 12));

        let mut last_mined_index_in_dense: Option<usize> = None;
        let leaves = TreeInitializer::get_leftover_leaves_and_update_index(
            &mut last_mined_index_in_dense,
            dense_prefix_depth,
            &identities,
        );

        // check if the index is correct
        assert_eq!(
            last_mined_index_in_dense,
            Some((1 << dense_prefix_depth) - 1)
        );

        // since there are more identities than dense prefix, the leaves should be 2
        assert_eq!(leaves.len(), 4);

        // additional check for correctness
        assert_eq!(leaves[0], identities[2].element);
        assert_eq!(leaves[1], identities[3].element);
        assert_eq!(leaves[2], identities[4].element);
        assert_eq!(leaves[3], identities[5].element);

        Ok(())
    }
}
