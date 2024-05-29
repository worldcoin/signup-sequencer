use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use chrono::{Duration, Utc};
use ruint::Uint;
use semaphore::poseidon_tree::LazyPoseidonTree;
use semaphore::protocol::verify_proof;
use sqlx::{Postgres, Transaction};
use tracing::{info, instrument, warn};

use crate::config::Config;
use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::database::query::DatabaseQuery as _;
use crate::database::Database;
use crate::ethereum::Ethereum;
use crate::identity_tree::{
    CanonicalTreeBuilder, Hash, InclusionProof, ProcessedStatus, RootItem, TreeState, TreeUpdate,
    TreeVersionReadOps, TreeWithNextVersion,
};
use crate::prover::map::initialize_prover_maps;
use crate::prover::{ProverConfig, ProverType};
use crate::server::data::{
    InclusionProofResponse, ListBatchSizesResponse, VerifySemaphoreProofQuery,
    VerifySemaphoreProofRequest, VerifySemaphoreProofResponse,
};
use crate::server::error::Error as ServerError;
use crate::utils::retry_tx;
use crate::utils::tree_updates::dedup_tree_updates;

pub struct App {
    pub database:           Arc<Database>,
    pub identity_manager:   SharedIdentityManager,
    tree_state:             OnceLock<TreeState>,
    pub snark_scalar_field: Hash,
    pub config:             Config,
}

impl App {
    /// # Errors
    ///
    /// Will return `Err` if the internal Ethereum handler errors or if the
    /// `options.storage_file` is not accessible.
    ///
    /// Upon calling `new`, the tree state will be uninitialized, and calling
    /// `app.tree_state()` will return an `Err`, and any methods which rely
    /// on the tree state will also error.
    #[instrument(name = "App::new", level = "debug", skip_all)]
    pub async fn new(config: Config) -> anyhow::Result<Arc<Self>> {
        let ethereum = Ethereum::new(&config);
        let db = Database::new(&config.database);

        let (ethereum, db) = tokio::try_join!(ethereum, db)?;

        let database = Arc::new(db);
        let mut provers: HashSet<ProverConfig> = database.get_provers().await?;

        let non_inserted_provers =
            Self::merge_env_provers(&config.app.provers_urls.0, &mut provers);

        database.insert_provers(non_inserted_provers).await?;

        let (insertion_prover_map, deletion_prover_map) = initialize_prover_maps(provers)?;

        let identity_manager = Arc::new(
            IdentityManager::new(
                &config,
                ethereum.clone(),
                insertion_prover_map,
                deletion_prover_map,
            )
            .await?,
        );

        // TODO Export the reduced-ness check that this is enabling from the
        //  `semaphore-rs` library when we bump the version.
        let snark_scalar_field = Hash::from_str_radix(
            "21888242871839275222246405745257275088548364400416034343698204186575808495617",
            10,
        )
        .expect("This should just parse.");

        let app = Arc::new(Self {
            database,
            identity_manager,
            tree_state: OnceLock::new(),
            snark_scalar_field,
            config,
        });

        Ok(app)
    }

    /// Initializes the tree state. This should only ever be called once.
    /// Attempts to call this method more than once will result in a panic.
    pub async fn init_tree(self: Arc<Self>) -> anyhow::Result<()> {
        // Await for all pending transactions
        self.identity_manager.await_clean_slate().await?;

        // Prefetch latest root & mark it as mined
        let root_hash = self.identity_manager.latest_root().await?;
        let root_hash = root_hash.into();

        let initial_root_hash = LazyPoseidonTree::new(
            self.identity_manager.tree_depth(),
            self.identity_manager.initial_leaf_value(),
        )
        .root();

        // We don't store the initial root in the database, so we have to skip this step
        // if the contract root hash is equal to initial root hash
        if root_hash != initial_root_hash {
            // Note that we don't have a way of queuing a root here for finalization.
            // so it's going to stay as "processed" until the next root is mined.
            self.database.mark_root_as_processed_tx(&root_hash).await?;
            self.database.delete_batches_after_root(&root_hash).await?;
        } else {
            // Db is either empty or we're restarting with a new contract/chain
            // so we should mark everything as pending
            self.database.mark_all_as_pending().await?;
            self.database.delete_all_batches().await?;
        }

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

        self.tree_state.set(tree_state).map_err(|_| {
            anyhow::anyhow!(
                "Failed to set tree state. 'App::init_tree' should only be called once."
            )
        })?;

        Ok::<(), anyhow::Error>(())
    }

    #[instrument(skip(self))]
    async fn restore_or_initialize_tree(
        &self,
        initial_root_hash: Hash,
    ) -> anyhow::Result<TreeState> {
        let mut mined_items = self
            .database
            .get_commitments_by_status(ProcessedStatus::Mined)
            .await?;

        mined_items.sort_by_key(|item| item.leaf_index);

        if !self.config.tree.force_cache_purge {
            info!("Attempting to restore tree from cache");
            if let Some(tree_state) = self
                .get_cached_tree_state(mined_items.clone(), initial_root_hash)
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
            // if the last index is greater then dense_prefix_depth, 1 << dense_prefix_depth
            // should be the last index in restored tree
            let last_index = std::cmp::min(max_leaf, (1 << dense_prefix_depth) - 1);
            *index = Some(last_index);

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

        leftover_items
    }

    async fn get_cached_tree_state(
        &self,
        mined_items: Vec<TreeUpdate>,
        initial_root_hash: Hash,
    ) -> anyhow::Result<Option<TreeState>> {
        let mined_items = dedup_tree_updates(mined_items);

        let mut last_mined_index_in_dense: Option<usize> = None;
        let leftover_items = Self::get_leftover_leaves_and_update_index(
            &mut last_mined_index_in_dense,
            self.config.tree.dense_tree_prefix_depth,
            &mined_items,
        );

        let Some(mined_builder) = CanonicalTreeBuilder::restore(
            self.identity_manager.tree_depth(),
            self.config.tree.dense_tree_prefix_depth,
            &self.identity_manager.initial_leaf_value(),
            last_mined_index_in_dense,
            &leftover_items,
            self.config.tree.tree_gc_threshold,
            &self.config.tree.cache_file,
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
                    return Ok(None);
                }
            }
            None => {
                if !mined.get_root().eq(&initial_root_hash) {
                    return Ok(None);
                }
            }
        }

        let processed_items = self
            .database
            .get_commitments_by_status(ProcessedStatus::Processed)
            .await?;

        for processed_item in processed_items {
            processed_builder.update(&processed_item);
        }

        let (processed, batching_builder) = processed_builder.seal_and_continue();
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

        Ok(Some(TreeState::new(mined, processed, batching, latest)))
    }

    pub fn tree_state(&self) -> anyhow::Result<&TreeState> {
        Ok(self
            .tree_state
            .get()
            .ok_or(ServerError::TreeStateUninitialized)?)
    }

    #[instrument(skip_all)]
    async fn initialize_tree(&self, mined_items: Vec<TreeUpdate>) -> anyhow::Result<TreeState> {
        // Flatten the updates for initial leaves
        tracing::info!("Deduplicating mined items");

        let mined_items = dedup_tree_updates(mined_items);
        let initial_leaf_value = self.identity_manager.initial_leaf_value();

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

        tracing::info!("Creating mined tree");
        let tree_depth = self.identity_manager.tree_depth();
        let dense_tree_prefix_depth = self.config.tree.dense_tree_prefix_depth;
        let tree_gc_threshold = self.config.tree.tree_gc_threshold;
        let cache_file = self.config.tree.cache_file.clone();

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

        let processed_items = self
            .database
            .get_commitments_by_status(ProcessedStatus::Processed)
            .await?;

        tracing::info!("Updating processed tree");
        let processed_builder = tokio::task::spawn_blocking(move || {
            for processed_item in processed_items {
                processed_builder.update(&processed_item);
            }

            processed_builder
        })
        .await?;

        let (processed, batching_builder) = processed_builder.seal_and_continue();
        let (batching, mut latest_builder) = batching_builder.seal_and_continue();

        let pending_items = self
            .database
            .get_commitments_by_status(ProcessedStatus::Pending)
            .await?;

        tracing::info!("Updating latest tree");
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

        Ok(TreeState::new(mined, processed, batching, latest))
    }

    /// Queues an insert into the merkle tree.
    ///
    /// # Errors
    ///
    /// Will return `Err` if identity is already queued, or in the tree, or the
    /// queue malfunctions.
    #[instrument(level = "debug", skip(self))]
    pub async fn insert_identity(&self, commitment: Hash) -> Result<(), ServerError> {
        if commitment == self.identity_manager.initial_leaf_value() {
            warn!(?commitment, "Attempt to insert initial leaf.");
            return Err(ServerError::InvalidCommitment);
        }

        if !self.identity_manager.has_insertion_provers().await {
            warn!(
                ?commitment,
                "Identity Manager has no insertion provers. Add provers with /addBatchSize \
                 request."
            );
            return Err(ServerError::NoProversOnIdInsert);
        }

        if !self.identity_is_reduced(commitment) {
            warn!(
                ?commitment,
                "The provided commitment is not an element of the field."
            );
            return Err(ServerError::UnreducedCommitment);
        }

        // TODO: ensure that the id is not in the tree or in unprocessed identities

        if self.database.identity_exists(commitment).await? {
            return Err(ServerError::DuplicateCommitment);
        }

        self.database
            .insert_new_identity(commitment, Utc::now())
            .await?;

        Ok(())
    }

    pub async fn delete_identity_tx(&self, commitment: &Hash) -> Result<(), ServerError> {
        retry_tx!(self.database.pool, tx, {
            self.delete_identity(&mut tx, commitment).await
        })
        .await?;
        Ok(())
    }

    /// Queues a deletion from the merkle tree.
    ///
    /// # Errors
    ///
    /// Will return `Err` if identity is already queued, not in the tree, or the
    /// queue malfunctions.
    #[instrument(level = "debug", skip(self, tx))]
    pub async fn delete_identity(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        commitment: &Hash,
    ) -> Result<(), ServerError> {
        // Ensure that deletion provers exist
        if !self.identity_manager.has_deletion_provers().await {
            warn!(
                ?commitment,
                "Identity Manager has no deletion provers. Add provers with /addBatchSize request."
            );
            return Err(ServerError::NoProversOnIdDeletion);
        }

        if !tx.identity_exists(*commitment).await? {
            return Err(ServerError::IdentityCommitmentNotFound);
        }

        // Get the leaf index for the id commitment
        let leaf_index = tx
            .get_identity_leaf_index(commitment)
            .await?
            .ok_or(ServerError::IdentityCommitmentNotFound)?
            .leaf_index;

        // Check if the id has already been deleted
        if self.tree_state()?.get_latest_tree().get_leaf(leaf_index) == Uint::ZERO {
            return Err(ServerError::IdentityAlreadyDeleted);
        }

        // Check if the id is already queued for deletion
        if tx.identity_is_queued_for_deletion(commitment).await? {
            return Err(ServerError::IdentityQueuedForDeletion);
        }

        // Check if there are any deletions, if not, set the latest deletion timestamp
        // to now to ensure that the new deletion is processed by the next deletion
        // interval
        if tx.get_deletions().await?.is_empty() {
            tx.update_latest_deletion(Utc::now()).await?;
        }

        // If the id has not been deleted, insert into the deletions table
        tx.insert_new_deletion(leaf_index, commitment).await?;

        Ok(())
    }

    /// Queues a recovery of an identity.
    ///
    /// i.e. deletion and reinsertion after a set period of time.
    ///
    /// # Errors
    ///
    /// Will return `Err` if identity is already queued for deletion, not in the
    /// tree, or the queue malfunctions.
    #[instrument(level = "debug", skip(self))]
    pub async fn recover_identity(
        &self,
        existing_commitment: &Hash,
        new_commitment: &Hash,
    ) -> Result<(), ServerError> {
        retry_tx!(self.database.pool, tx, {
            if *new_commitment == self.identity_manager.initial_leaf_value() {
                warn!(
                    ?new_commitment,
                    "Attempt to insert initial leaf in recovery."
                );
                return Err(ServerError::InvalidCommitment);
            }

            if !self.identity_manager.has_insertion_provers().await {
                warn!(
                    ?new_commitment,
                    "Identity Manager has no provers. Add provers with /addBatchSize request."
                );
                return Err(ServerError::NoProversOnIdInsert);
            }

            if !self.identity_is_reduced(*new_commitment) {
                warn!(
                    ?new_commitment,
                    "The new identity commitment is not reduced."
                );
                return Err(ServerError::UnreducedCommitment);
            }

            if tx.identity_exists(*new_commitment).await? {
                return Err(ServerError::DuplicateCommitment);
            }

            // Delete the existing id and insert the commitments into the recovery table
            self.delete_identity(&mut tx, existing_commitment).await?;

            tx.insert_new_recovery(existing_commitment, new_commitment)
                .await?;

            Ok(())
        })
        .await
    }

    fn merge_env_provers(
        prover_urls: &[ProverConfig],
        existing_provers: &mut HashSet<ProverConfig>,
    ) -> HashSet<ProverConfig> {
        let options_set: HashSet<ProverConfig> = prover_urls
            .iter()
            .cloned()
            .map(|opt| ProverConfig {
                url:         opt.url,
                batch_size:  opt.batch_size,
                timeout_s:   opt.timeout_s,
                prover_type: opt.prover_type,
            })
            .collect();

        let env_provers: HashSet<_> = options_set.difference(existing_provers).cloned().collect();

        for unique in &env_provers {
            existing_provers.insert(unique.clone());
        }

        env_provers
    }

    fn identity_is_reduced(&self, commitment: Hash) -> bool {
        commitment.lt(&self.snark_scalar_field)
    }

    /// # Errors
    ///
    /// Will return `Err` if the provided batch size already exists.
    /// Will return `Err` if the batch size fails to write to database.
    #[instrument(level = "debug", skip(self))]
    pub async fn add_batch_size(
        &self,
        url: String,
        batch_size: usize,
        timeout_seconds: u64,
        prover_type: ProverType,
    ) -> Result<(), ServerError> {
        self.identity_manager
            .add_batch_size(&url, batch_size, timeout_seconds, prover_type)
            .await?;

        self.database
            .insert_prover_configuration(batch_size, url, timeout_seconds, prover_type)
            .await?;

        Ok(())
    }

    /// # Errors
    ///
    /// Will return `Err` if the requested batch size does not exist.
    /// Will return `Err` if batch size fails to be removed from database.
    #[instrument(level = "debug", skip(self))]
    pub async fn remove_batch_size(
        &self,
        batch_size: usize,
        prover_type: ProverType,
    ) -> Result<(), ServerError> {
        self.identity_manager
            .remove_batch_size(batch_size, prover_type)
            .await?;

        self.database.remove_prover(batch_size, prover_type).await?;

        Ok(())
    }

    /// # Errors
    ///
    /// Will return `Err` if something unknown went wrong.
    #[instrument(level = "debug", skip(self))]
    pub async fn list_batch_sizes(&self) -> Result<ListBatchSizesResponse, ServerError> {
        let batches = self.identity_manager.list_batch_sizes().await?;

        Ok(ListBatchSizesResponse::from(batches))
    }

    /// # Errors
    ///
    /// Will return `Err` if the provided index is out of bounds.
    #[instrument(level = "debug", skip(self))]
    pub async fn inclusion_proof(
        &self,
        commitment: &Hash,
    ) -> Result<InclusionProofResponse, ServerError> {
        if commitment == &self.identity_manager.initial_leaf_value() {
            return Err(ServerError::InvalidCommitment);
        }

        if let Some((status, error_message)) = self
            .database
            .get_unprocessed_commit_status(commitment)
            .await?
        {
            return Ok(InclusionProofResponse(InclusionProof {
                status:  status.into(),
                root:    None,
                proof:   None,
                message: Some(error_message),
            }));
        }

        let item = self
            .database
            .get_identity_leaf_index(commitment)
            .await?
            .ok_or(ServerError::IdentityCommitmentNotFound)?;

        let (leaf, proof) = self.tree_state()?.get_proof_for(&item);

        if leaf != *commitment {
            return Err(ServerError::InvalidCommitment);
        }

        Ok(InclusionProofResponse(proof))
    }

    /// # Errors
    ///
    /// Will return `Err` if the provided proof is invalid.
    #[instrument(level = "debug", skip(self))]
    pub async fn verify_semaphore_proof(
        &self,
        request: &VerifySemaphoreProofRequest,
        query: &VerifySemaphoreProofQuery,
    ) -> Result<VerifySemaphoreProofResponse, ServerError> {
        let Some(root_state) = self.database.get_root_state(&request.root).await? else {
            return Err(ServerError::InvalidRoot);
        };

        if let Some(max_root_age_seconds) = query.max_root_age_seconds {
            let max_root_age = Duration::seconds(max_root_age_seconds);
            self.validate_root_age(max_root_age, &root_state)?;
        }

        let checked = verify_proof(
            request.root,
            request.nullifier_hash,
            request.signal_hash,
            request.external_nullifier_hash,
            &request.proof,
            self.identity_manager.tree_depth(),
        );

        match checked {
            Ok(true) => Ok(VerifySemaphoreProofResponse(root_state)),
            Ok(false) => Err(ServerError::InvalidProof),
            Err(err) => {
                info!(?err, "verify_proof failed with error");
                Err(ServerError::ProverError)
            }
        }
    }

    fn validate_root_age(
        &self,
        max_root_age: Duration,
        root_state: &RootItem,
    ) -> Result<(), ServerError> {
        let tree_state = self.tree_state()?;
        let latest_root = tree_state.get_latest_tree().get_root();
        let batching_root = tree_state.get_batching_tree().get_root();
        let processed_root = tree_state.get_processed_tree().get_root();
        let mined_root = tree_state.get_mined_tree().get_root();

        tracing::info!("Validating age max_root_age: {max_root_age:?}");

        let root = root_state.root;

        match root_state.status {
            // Pending status implies the batching or latest tree
            ProcessedStatus::Pending if latest_root == root || batching_root == root => {
                tracing::warn!("Root is pending - skipping");
                return Ok(());
            }
            // Processed status is hidden - this should never happen
            ProcessedStatus::Processed if processed_root == root => {
                tracing::warn!("Root is processed - skipping");
                return Ok(());
            }
            // Processed status is hidden so it could be either processed or mined
            ProcessedStatus::Mined if processed_root == root || mined_root == root => {
                tracing::warn!("Root is mined - skipping");
                return Ok(());
            }
            _ => (),
        }

        let now = chrono::Utc::now();

        let root_age = if matches!(
            root_state.status,
            ProcessedStatus::Pending | ProcessedStatus::Processed
        ) {
            now - root_state.pending_valid_as_of
        } else {
            let mined_at = root_state
                .mined_valid_as_of
                .ok_or(ServerError::InvalidRoot)?;
            now - mined_at
        };

        tracing::warn!("Root age: {root_age:?}");

        if root_age > max_root_age {
            Err(ServerError::RootTooOld)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod test {
    use ethers::prelude::rand;
    use ethers::types::U256;
    use ruint::Uint;

    use super::App;
    use crate::identity_tree::TreeUpdate;

    pub fn generate_test_identities_with_index(identity_count: usize) -> Vec<TreeUpdate> {
        let mut identities = vec![];

        for i in 1..=identity_count {
            let bytes: [u8; 32] = U256::from(rand::random::<u64>()).into();
            let identity = Uint::<256, 4>::from_le_bytes(bytes);

            identities.push(TreeUpdate {
                leaf_index: i,
                element:    identity,
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

        let mut last_mined_index_in_dense: Option<usize> = None;
        let leaves = App::get_leftover_leaves_and_update_index(
            &mut last_mined_index_in_dense,
            dense_prefix_depth,
            &identities,
        );

        // check if the index is correct
        assert_eq!(last_mined_index_in_dense, None);

        // since there are no identities at all the leaves should be 0
        assert_eq!(leaves.len(), 0);

        // first test with less then dense prefix
        let identities = generate_test_identities_with_index(less_identities_count);

        last_mined_index_in_dense = None;

        let leaves = App::get_leftover_leaves_and_update_index(
            &mut last_mined_index_in_dense,
            dense_prefix_depth,
            &identities,
        );

        // check if the index is correct
        assert_eq!(last_mined_index_in_dense, Some(identities.len()));
        // since there are less identities then dense prefix, the leavs should be empty
        // vector
        assert!(leaves.is_empty());

        // lets try now with more identities then dense prefix supports

        // this should generate 2^dense_prefix + 2
        let identities = generate_test_identities_with_index(more_identities_count);

        last_mined_index_in_dense = None;
        let leaves = App::get_leftover_leaves_and_update_index(
            &mut last_mined_index_in_dense,
            dense_prefix_depth,
            &identities,
        );

        // check if the index is correct
        assert_eq!(
            last_mined_index_in_dense,
            Some((1 << dense_prefix_depth) - 1)
        );

        // since there are more identities then dense prefix, the leavs should be 2
        assert_eq!(leaves.len(), 2);

        // additional check for correctness
        assert_eq!(leaves[0], identities[8].element);
        assert_eq!(leaves[1], identities[9].element);

        Ok(())
    }
}
