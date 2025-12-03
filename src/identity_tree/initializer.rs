use std::sync::Arc;
use std::time::Instant;

use semaphore_rs::poseidon_tree::LazyPoseidonTree;
use tokio::sync::Mutex;
use tracing::{info, instrument, warn};

use crate::config::TreeConfig;
use crate::database::methods::DbMethods;
use crate::database::Database;
use crate::identity::processor::IdentityProcessor;
use crate::identity_tree::builder::CanonicalTreeBuilder;
use crate::identity_tree::db_sync::sync_tree;
use crate::identity_tree::{Hash, ProcessedStatus, TreeState, TreeVersionReadOps};
use crate::retry_tx;
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
    pub async fn run(self) -> anyhow::Result<Mutex<TreeState>> {
        let initial_root_hash =
            LazyPoseidonTree::new(self.config.tree_depth, self.config.initial_leaf_value).root();

        self.identity_processor
            .tree_init_correction(&initial_root_hash)
            .await?;

        let timer = Instant::now();
        info!("Tree state initialization started");
        let tree_state = self.restore_or_initialize_tree(initial_root_hash).await?;
        info!("Tree state initialization took: {:?}", timer.elapsed());

        let timer = Instant::now();
        info!("Syncing tree on startup");
        retry_tx!(
            &self.database,
            tx,
            sync_tree(&mut tx, &tree_state.lock().await).await
        )
        .await?;
        info!("Sync tree on startup took: {:?}", timer.elapsed());

        Ok(tree_state)
    }

    #[instrument(skip(self))]
    async fn restore_or_initialize_tree(
        &self,
        initial_root_hash: Hash,
    ) -> anyhow::Result<Mutex<TreeState>> {
        if !self.config.force_cache_purge {
            info!("Trying to load tree from cache");
            let timer_cache = Instant::now();
            let tree_state = self.restore_cached_tree().await?;
            info!("Restoring cached tree took: {:?}", timer_cache.elapsed());

            if let Some(tree_state) = tree_state {
                let (tree_root, tree_last_sequence_id) = {
                    let processed_tree = tree_state.lock().await.get_processed_tree();
                    (
                        processed_tree.get_root(),
                        processed_tree.get_last_sequence_id(),
                    )
                };

                if tree_root == initial_root_hash {
                    warn!("Restored cached tree is empty.");
                } else if let Some(root) = self.identity_processor.latest_root().await? {
                    if let Some(tree_update) = self.database.get_tree_update_by_root(&root).await? {
                        if tree_last_sequence_id <= tree_update.sequence_id {
                            return Ok(tree_state);
                        } else {
                            warn!("Cached tree last sequence id is ahead of one set in identity processor.")
                        }
                    } else {
                        warn!("Couldn't find tree update with root returned by identity processor.")
                    }
                } else {
                    warn!("Identity processor returned no latest root.")
                }
            }
        }

        info!("Trying to create tree from database");
        let timer_db = Instant::now();
        let tree_state = self.initialize_tree().await?;
        info!("Creating tree from database took: {:?}", timer_db.elapsed());

        Ok(tree_state)
    }

    #[instrument(skip(self))]
    async fn restore_cached_tree(&self) -> anyhow::Result<Option<Mutex<TreeState>>> {
        info!("Restoring tree from cache");

        info!(
            "Restoring dense canonical mined tree from cache (file={})",
            &self.config.cache_file,
        );

        let Some(restored_mined_builder) = CanonicalTreeBuilder::restore_dense(
            self.config.tree_depth,
            self.config.dense_tree_prefix_depth,
            &self.config.initial_leaf_value,
            self.config.tree_gc_threshold,
            &self.config.cache_file,
        ) else {
            return Ok(None);
        };

        info!("Restored dense canonical mined tree");

        let mined_builder = match self
            .database
            .get_tree_update_by_root(&restored_mined_builder.root())
            .await?
        {
            Some(last_dense_leaf) => {
                let next_leaf_index = self.database.get_next_leaf_index_up_to_sequence_id(last_dense_leaf.sequence_id).await?;
                restored_mined_builder.with_leaf(next_leaf_index, last_dense_leaf.sequence_id)
            },
            None => {
                info!("Cannot find tree update matching restored tree root.");
                return Ok(None);
            }
        };

        info!("Restoring canonical mined tree");

        let (mined, processed_builder) = mined_builder.seal();

        info!("Restoring derived processed, batching and latest tree");

        let (processed, batching_builder) = processed_builder.seal_and_continue();
        let (batching, latest_builder) = batching_builder.seal_and_continue();
        let latest = latest_builder.seal();

        let tree_state = Mutex::new(TreeState::new(mined, processed, batching, latest));

        info!("Tree restored.");

        Ok(Some(tree_state))
    }

    #[instrument(skip_all)]
    async fn initialize_tree(&self) -> anyhow::Result<Mutex<TreeState>> {
        info!("Creating tree from the database");

        info!("Getting mined commitments from DB");
        let mut mined_items = self
            .database
            .get_tree_updates_by_status(ProcessedStatus::Mined)
            .await?;

        mined_items.sort_by_key(|item| item.leaf_index);

        let mined_items = dedup_tree_updates(mined_items);

        info!("Retrieved {} mined commitments from DB", mined_items.len());

        let initial_leaf_value = self.config.initial_leaf_value;

        let initial_leaves = if mined_items.is_empty() {
            vec![]
        } else {
            let max_leaf = mined_items.last().map(|item| item.leaf_index).unwrap();
            let mut leaves = vec![None; max_leaf + 1];

            for item in mined_items {
                let i = item.leaf_index;
                leaves[i] = Some(item);
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

        let (mined, processed_builder) = mined_builder.seal();

        info!("Creating derived processed, batching and latest tree");

        let (processed, batching_builder) = processed_builder.seal_and_continue();
        let (batching, latest_builder) = batching_builder.seal_and_continue();
        let latest = latest_builder.seal();

        let tree_state = Mutex::new(TreeState::new(mined, processed, batching, latest));

        info!("Initial tree state created. Syncing tree.");

        retry_tx!(
            &self.database,
            tx,
            sync_tree(&mut tx, &tree_state.lock().await).await
        )
        .await?;

        info!("Tree created.");

        Ok(tree_state)
    }
}
