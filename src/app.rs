use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result as AnyhowResult;
use chrono::{Duration, Utc};
use clap::Parser;
use hyper::StatusCode;
use ruint::Uint;
use semaphore::poseidon_tree::LazyPoseidonTree;
use semaphore::protocol::verify_proof;
use serde::Serialize;
use tracing::{info, instrument, warn};

use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::database::{self, Database};
use crate::ethereum::{self, Ethereum};
use crate::identity_tree::{
    CanonicalTreeBuilder, Hash, InclusionProof, RootItem, Status, TreeState, TreeVersionReadOps,
};
use crate::prover::map::initialize_prover_maps;
use crate::prover::{self, ProverConfiguration, ProverType, Provers};
use crate::server::error::Error as ServerError;
use crate::server::{ToResponseCode, VerifySemaphoreProofQuery, VerifySemaphoreProofRequest};
use crate::task_monitor::TaskMonitor;
use crate::utils::tree_updates::dedup_tree_updates;
use crate::{contracts, task_monitor};

#[derive(Serialize)]
#[serde(transparent)]
pub struct InclusionProofResponse(InclusionProof);

impl InclusionProofResponse {
    #[must_use]
    pub fn hide_processed_status(mut self) -> Self {
        self.0.status = if self.0.status == Status::Processed {
            Status::Pending
        } else {
            self.0.status
        };

        self
    }
}

impl From<InclusionProof> for InclusionProofResponse {
    fn from(value: InclusionProof) -> Self {
        Self(value)
    }
}

impl ToResponseCode for InclusionProofResponse {
    fn to_response_code(&self) -> StatusCode {
        match self.0.status {
            Status::Failed => StatusCode::BAD_REQUEST,
            Status::New | Status::Pending => StatusCode::ACCEPTED,
            Status::Mined | Status::Processed => StatusCode::OK,
        }
    }
}

#[derive(Serialize)]
#[serde(transparent)]
pub struct ListBatchSizesResponse(Vec<ProverConfiguration>);

impl From<Vec<ProverConfiguration>> for ListBatchSizesResponse {
    fn from(value: Vec<ProverConfiguration>) -> Self {
        Self(value)
    }
}

impl ToResponseCode for ListBatchSizesResponse {
    fn to_response_code(&self) -> StatusCode {
        StatusCode::OK
    }
}

#[derive(Serialize)]
#[serde(transparent)]
pub struct VerifySemaphoreProofResponse(RootItem);

impl VerifySemaphoreProofResponse {
    #[must_use]
    pub fn hide_processed_status(mut self) -> Self {
        self.0.status = if self.0.status == Status::Processed {
            Status::Pending
        } else {
            self.0.status
        };

        self
    }
}

impl ToResponseCode for VerifySemaphoreProofResponse {
    fn to_response_code(&self) -> StatusCode {
        StatusCode::OK
    }
}

#[derive(Clone, Debug, PartialEq, Parser)]
#[group(skip)]
pub struct Options {
    #[clap(flatten)]
    pub ethereum: ethereum::Options,

    #[clap(flatten)]
    pub contracts: contracts::Options,

    #[clap(flatten)]
    pub database: database::Options,

    #[clap(flatten)]
    pub batch_provers: prover::Options,

    #[clap(flatten)]
    pub committer: task_monitor::Options,

    /// Block number to start syncing from
    #[clap(long, env, default_value = "0")]
    pub starting_block: u64,

    /// Timeout for the tree lock (seconds).
    #[clap(long, env, default_value = "120")]
    pub lock_timeout: u64,

    /// The depth of the tree prefix that is vectorized.
    #[clap(long, env, default_value = "20")]
    pub dense_tree_prefix_depth: usize,

    /// The number of updates to trigger garbage collection.
    #[clap(long, env, default_value = "10000")]
    pub tree_gc_threshold: usize,
}

pub struct App {
    database:           Arc<Database>,
    identity_manager:   SharedIdentityManager,
    identity_committer: Arc<TaskMonitor>,
    tree_state:         TreeState,
    snark_scalar_field: Hash,
}

impl App {
    /// # Errors
    ///
    /// Will return `Err` if the internal Ethereum handler errors or if the
    /// `options.storage_file` is not accessible.
    #[instrument(name = "App::new", level = "debug")]
    pub async fn new(options: Options) -> AnyhowResult<Self> {
        let ethereum = Ethereum::new(options.ethereum);
        let db = Database::new(options.database);

        let (ethereum, db) = tokio::try_join!(ethereum, db)?;

        let database = Arc::new(db);
        let mut provers: HashSet<ProverConfiguration> = database.get_provers().await?;

        let non_inserted_provers = Self::merge_env_provers(options.batch_provers, &mut provers);

        database.insert_provers(non_inserted_provers).await?;

        let (insertion_prover_map, deletion_prover_map) = initialize_prover_maps(provers)?;

        let identity_manager = IdentityManager::new(
            options.contracts,
            ethereum.clone(),
            insertion_prover_map,
            deletion_prover_map,
        )
        .await?;

        let identity_manager = Arc::new(identity_manager);

        // Await for all pending transactions
        identity_manager.await_clean_slate().await?;

        // Prefetch latest root & mark it as mined
        let root_hash = identity_manager.latest_root().await?;
        let root_hash = root_hash.into();

        let initial_root_hash = LazyPoseidonTree::new(
            identity_manager.tree_depth(),
            identity_manager.initial_leaf_value(),
        )
        .root();

        // We don't store the initial root in the database, so we have to skip this step
        // if the contract root hash is equal to initial root hash
        if root_hash != initial_root_hash {
            // Note that we don't have a way of queuing a root here for finalization.
            // so it's going to stay as "processed" until the next root is mined.
            database.mark_root_as_processed(&root_hash).await?;
        }

        let timer = Instant::now();
        let tree_state = Self::initialize_tree(
            &database,
            // Poseidon tree depth is one more than the contract's tree depth
            identity_manager.tree_depth(),
            options.dense_tree_prefix_depth,
            options.tree_gc_threshold,
            identity_manager.initial_leaf_value(),
        )
        .await?;
        info!("Tree state initialization took: {:?}", timer.elapsed());

        let identity_committer = Arc::new(TaskMonitor::new(
            database.clone(),
            identity_manager.clone(),
            tree_state.clone(),
            &options.committer,
        ));

        // TODO Export the reduced-ness check that this is enabling from the
        //  `semaphore-rs` library when we bump the version.
        let snark_scalar_field = Hash::from_str_radix(
            "21888242871839275222246405745257275088548364400416034343698204186575808495617",
            10,
        )
        .expect("This should just parse.");

        // Process to push new identities to Ethereum
        identity_committer.start().await;

        // Sync with chain on start up
        let app = Self {
            database,
            identity_manager,
            identity_committer,
            tree_state,
            snark_scalar_field,
        };

        Ok(app)
    }

    async fn initialize_tree(
        database: &Database,
        tree_depth: usize,
        dense_prefix_depth: usize,
        gc_threshold: usize,
        initial_leaf_value: Hash,
    ) -> AnyhowResult<TreeState> {
        let mined_items = database.get_commitments_by_status(Status::Mined).await?;

        // Flatten the updates for initial leaves
        let mined_items = dedup_tree_updates(mined_items);

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

        let mined_builder = CanonicalTreeBuilder::new(
            tree_depth,
            dense_prefix_depth,
            gc_threshold,
            initial_leaf_value,
            &initial_leaves,
        );

        let (mined, mut processed_builder) = mined_builder.seal();

        let processed_items = database
            .get_commitments_by_status(Status::Processed)
            .await?;

        for processed_item in processed_items {
            processed_builder.update(&processed_item);
        }

        let (processed, batching_builder) = processed_builder.seal_and_continue();
        let (batching, mut latest_builder) = batching_builder.seal_and_continue();

        let pending_items = database.get_commitments_by_status(Status::Pending).await?;
        for update in pending_items {
            latest_builder.update(&update);
        }

        let latest = latest_builder.seal();

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

        let identity_exists = self.database.identity_exists(commitment).await?;
        if identity_exists {
            return Err(ServerError::DuplicateCommitment);
        }

        self.database
            .insert_new_identity(commitment, Utc::now())
            .await?;

        Ok(())
    }

    /// Queues a deletion from the merkle tree.
    ///
    /// # Errors
    ///
    /// Will return `Err` if identity is already queued, not in the tree, or the
    /// queue malfunctions.
    #[instrument(level = "debug", skip(self))]
    pub async fn delete_identity(&self, commitment: &Hash) -> Result<(), ServerError> {
        // Ensure that deletion provers exist
        if !self.identity_manager.has_deletion_provers().await {
            warn!(
                ?commitment,
                "Identity Manager has no deletion provers. Add provers with /addBatchSize request."
            );
            return Err(ServerError::NoProversOnIdDeletion);
        }

        if !self.database.identity_exists(*commitment).await? {
            return Err(ServerError::IdentityCommitmentNotFound);
        }

        // Get the leaf index for the id commitment
        let leaf_index = self
            .database
            .get_identity_leaf_index(commitment)
            .await?
            .ok_or(ServerError::IdentityCommitmentNotFound)?
            .leaf_index;

        // Check if the id has already been deleted
        if self.tree_state.get_latest_tree().get_leaf(leaf_index) == Uint::ZERO {
            return Err(ServerError::IdentityAlreadyDeleted);
        }

        // Check if the id is already queued for deletion
        if self
            .database
            .identity_is_queued_for_deletion(commitment)
            .await?
        {
            return Err(ServerError::IdentityQueuedForDeletion);
        }

        // Check if there are any deletions, if not, set the latest deletion timestamp
        // to now to ensure that the new deletion is processed by the next deletion
        // interval
        if self.database.get_deletions().await?.is_empty() {
            self.database.update_latest_deletion(Utc::now()).await?;
        }

        // If the id has not been deleted, insert into the deletions table
        self.database
            .insert_new_deletion(leaf_index, commitment)
            .await?;

        Ok(())
    }

    /// Queues a deletion from the merkle tree.
    ///
    /// # Errors
    ///
    /// Will return `Err` if identity is already queued, not in the tree, or the
    /// queue malfunctions.
    #[instrument(level = "debug", skip(self))]
    pub async fn recover_identity(
        &self,
        existing_commitment: &Hash,
        new_commitment: &Hash,
    ) -> Result<(), ServerError> {
        // Ensure that insertion provers exist
        if !self.identity_manager.has_insertion_provers().await {
            warn!(
                ?new_commitment,
                "Identity Manager has no provers. Add provers with /addBatchSize request."
            );
            return Err(ServerError::NoProversOnIdInsert);
        }

        // Delete the existing id and insert the commitments into the recovery table
        self.delete_identity(existing_commitment).await?;

        self.database
            .insert_new_recovery(existing_commitment, new_commitment)
            .await?;

        Ok(())
    }

    fn merge_env_provers(options: prover::Options, existing_provers: &mut Provers) -> Provers {
        let options_set: HashSet<ProverConfiguration> = options
            .prover_urls
            .0
            .into_iter()
            .map(|opt| ProverConfiguration {
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
                status,
                root: None,
                proof: None,
                message: Some(error_message),
            }));
        }

        let item = self
            .database
            .get_identity_leaf_index(commitment)
            .await?
            .ok_or(ServerError::IdentityCommitmentNotFound)?;

        let (leaf, proof) = self.tree_state.get_proof_for(&item);

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
        let latest_root = self.tree_state.get_latest_tree().get_root();
        let batching_root = self.tree_state.get_batching_tree().get_root();
        let processed_root = self.tree_state.get_processed_tree().get_root();
        let mined_root = self.tree_state.get_mined_tree().get_root();

        let root = root_state.root;

        match root_state.status {
            // Pending status implies the batching or latest tree
            Status::Pending if latest_root == root || batching_root == root => return Ok(()),
            // Processed status is hidden - this should never happen
            Status::Processed if processed_root == root => return Ok(()),
            // Processed status is hidden so it could be either processed or mined
            Status::Mined if processed_root == root || mined_root == root => return Ok(()),
            _ => (),
        }

        let now = chrono::Utc::now();

        let root_age = if matches!(root_state.status, Status::Pending | Status::Processed) {
            now - root_state.pending_valid_as_of
        } else {
            let mined_at = root_state
                .mined_valid_as_of
                .ok_or(ServerError::InvalidRoot)?;
            now - mined_at
        };

        if root_age > max_root_age {
            Err(ServerError::RootTooOld)
        } else {
            Ok(())
        }
    }

    /// # Errors
    ///
    /// Will return an Error if any of the components cannot be shut down
    /// gracefully.
    pub async fn shutdown(&self) -> AnyhowResult<()> {
        info!("Shutting down identity committer.");
        self.identity_committer.shutdown().await
    }
}
