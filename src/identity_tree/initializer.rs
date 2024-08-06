use std::sync::Arc;
use std::time::Instant;

use semaphore::poseidon_tree::LazyPoseidonTree;
use tracing::{info, instrument, warn};

use crate::config::TreeConfig;
use crate::database::query::DatabaseQuery;
use crate::database::Database;
use crate::identity::processor::IdentityProcessor;
use crate::identity_tree::builder::CanonicalTreeBuilder;
use crate::identity_tree::{
    Hash, ProcessedStatus, TreeState, TreeUpdate, TreeVersionReadOps, TreeWithNextVersion,
};
use crate::utils::tree_updates::dedup_tree_updates;

pub struct TreeInitializer {
    pub database:           Arc<Database>,
    pub identity_processor: Arc<dyn IdentityProcessor>,
    pub config:             TreeConfig,
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
        let mut tree_state = self.restore_or_initialize_tree(initial_root_hash).await?;
        info!("Tree state initialization took: {:?}", timer.elapsed());

        let tree_root = tree_state.get_processed_tree().get_root();

        if tree_root != initial_root_hash {
            warn!(
                "Cached tree root is different from the contract root. Purging cache and \
                 reinitializing."
            );

            tree_state = self.restore_or_initialize_tree(initial_root_hash).await?;
        }

        Ok(tree_state)
    }

    #[instrument(skip(self))]
    async fn restore_or_initialize_tree(
        &self,
        initial_root_hash: Hash,
    ) -> anyhow::Result<TreeState> {
        let mut mined_or_processed_items = self
            .database
            .get_commitments_by_statuses(vec![ProcessedStatus::Mined, ProcessedStatus::Processed])
            .await?;

        mined_or_processed_items.sort_by_key(|item| item.leaf_index);

        let mined_or_processed_items = dedup_tree_updates(mined_or_processed_items);

        if !self.config.force_cache_purge {
            info!("Attempting to restore tree from cache");
            if let Some(tree_state) = self
                .get_cached_tree_state(&mined_or_processed_items, initial_root_hash)
                .await?
            {
                info!("tree restored from cache");
                return Ok(tree_state);
            }
        }

        info!("Initializing tree from the database");
        let tree_state = self.initialize_tree(mined_or_processed_items).await?;

        info!("tree initialization successful");

        Ok(tree_state)
    }

    pub fn get_leftover_leaves_and_update_index(
        last_dense_leaf: &mut Option<TreeUpdate>,
        dense_prefix_depth: usize,
        mined_or_processed_items: &[TreeUpdate],
    ) -> Vec<TreeUpdate> {
        let leftover_items = if mined_or_processed_items.is_empty() {
            vec![]
        } else {
            let max_leaf = mined_or_processed_items
                .last()
                .map(|item| item.leaf_index)
                .unwrap();
            // if the last index is greater than dense_prefix_depth, 1 << dense_prefix_depth
            // should be the last index in restored tree
            let max_dense_leaf_index = std::cmp::min(max_leaf, (1 << dense_prefix_depth) - 1);
            let last_dense_leaf_index = mined_or_processed_items
                .iter()
                .rposition(|v| v.leaf_index <= max_dense_leaf_index);

            *last_dense_leaf = last_dense_leaf_index
                .and_then(|v| mined_or_processed_items.get(v))
                .cloned();

            println!("{:?}", mined_or_processed_items);
            println!("{:?}", last_dense_leaf_index);

            mined_or_processed_items[last_dense_leaf_index.map(|v| v + 1).unwrap_or(0)..].to_vec()
        };

        leftover_items
    }

    async fn get_cached_tree_state(
        &self,
        mined_or_processed_items: &[TreeUpdate],
        initial_root_hash: Hash,
    ) -> anyhow::Result<Option<TreeState>> {
        let mut last_mined_or_processed_leaf_in_dense: Option<TreeUpdate> = None;
        let leftover_items = Self::get_leftover_leaves_and_update_index(
            &mut last_mined_or_processed_leaf_in_dense,
            self.config.dense_tree_prefix_depth,
            mined_or_processed_items,
        );

        let Some(processed_builder) = CanonicalTreeBuilder::restore(
            self.config.tree_depth,
            self.config.dense_tree_prefix_depth,
            &self.config.initial_leaf_value,
            last_mined_or_processed_leaf_in_dense,
            &leftover_items,
            self.config.tree_gc_threshold,
            &self.config.cache_file,
        ) else {
            return Ok(None);
        };

        let (processed, batching_builder) = processed_builder.seal();

        match self
            .database
            .get_latest_root_by_status(ProcessedStatus::Processed)
            .await?
        {
            Some(root) => {
                if !processed.get_root().eq(&root) {
                    return Ok(None);
                }
            }
            None => {
                if !processed.get_root().eq(&initial_root_hash) {
                    return Ok(None);
                }
            }
        }

        let (batching, mut latest_builder) = batching_builder.seal_and_continue();

        let pending_items = self
            .database
            .get_commitments_by_status(ProcessedStatus::Pending)
            .await?;
        for update in pending_items {
            latest_builder.update(&update);
        }
        let latest = latest_builder.seal();

        let batch = self.database.get_latest_batch().await?;
        if let Some(batch) = batch {
            if batching.get_root() != batch.next_root {
                batching.apply_updates_up_to(batch.next_root);
            }
            assert_eq!(batching.get_root(), batch.next_root);
        }

        Ok(Some(TreeState::new(processed, batching, latest)))
    }

    #[instrument(skip_all)]
    async fn initialize_tree(
        &self,
        mined_or_processed_items: Vec<TreeUpdate>,
    ) -> anyhow::Result<TreeState> {
        let initial_leaf_value = self.config.initial_leaf_value;

        let initial_leaves = if mined_or_processed_items.is_empty() {
            vec![]
        } else {
            let max_leaf = mined_or_processed_items
                .last()
                .map(|item| item.leaf_index)
                .unwrap();
            let mut leaves = vec![None; max_leaf + 1];

            for item in mined_or_processed_items {
                let i = item.leaf_index;
                leaves[i] = Some(item);
            }

            leaves
        };

        info!("Creating mined tree");
        let tree_depth = self.config.tree_depth;
        let dense_tree_prefix_depth = self.config.dense_tree_prefix_depth;
        let tree_gc_threshold = self.config.tree_gc_threshold;
        let cache_file = self.config.cache_file.clone();

        let processed_builder = tokio::task::spawn_blocking(move || {
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

        let (processed, batching_builder) = processed_builder.seal();
        let (batching, mut latest_builder) = batching_builder.seal_and_continue();

        let pending_items = self
            .database
            .get_commitments_by_status(ProcessedStatus::Pending)
            .await?;

        info!("Updating latest tree");
        let latest_builder = tokio::task::spawn_blocking(move || {
            for update in pending_items {
                latest_builder.update(&update);
            }

            latest_builder
        })
        .await?;

        let latest = latest_builder.seal();

        let batch = self.database.get_latest_batch().await?;
        if let Some(batch) = batch {
            if batching.get_root() != batch.next_root {
                batching.apply_updates_up_to(batch.next_root);
            }
            assert_eq!(batching.get_root(), batch.next_root);
        }

        Ok(TreeState::new(processed, batching, latest))
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
        let mut identities = vec![];

        for i in 0..identity_count {
            let bytes: [u8; 32] = U256::from(rand::random::<u64>()).into();
            let identity = Uint::<256, 4>::from_le_bytes(bytes);

            identities.push(TreeUpdate {
                sequence_id: i + 1,
                leaf_index:  i,
                element:     identity,
                post_root:   identity,
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
        // indecies at all)
        let identities: Vec<TreeUpdate> = vec![];

        let mut last_mined_index_in_dense: Option<TreeUpdate> = None;
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
        assert_eq!(
            last_mined_index_in_dense.unwrap().leaf_index,
            identities.len() - 1
        );
        // since there are fewer identities than dense prefix, the leaves should be
        // empty vector
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
            last_mined_index_in_dense.unwrap().leaf_index,
            (1 << dense_prefix_depth) - 1
        );

        // since there are more identities than dense prefix, the leaves should be 2
        assert_eq!(leaves.len(), 2);

        // additional check for correctness
        assert_eq!(leaves[0].element, identities[8].element);
        assert_eq!(leaves[1].element, identities[9].element);

        Ok(())
    }
}
