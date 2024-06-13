use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use chrono::{Duration, Utc};
use ruint::Uint;
use semaphore::protocol::verify_proof;
use sqlx::{Postgres, Transaction};
use tracing::{info, instrument, warn};

use crate::config::Config;
use crate::contracts::IdentityManager;
use crate::database::query::DatabaseQuery as _;
use crate::database::Database;
use crate::ethereum::Ethereum;
use crate::identity::processor::{
    IdentityProcessor, OffChainIdentityProcessor, OnChainIdentityProcessor,
};
use crate::identity::validator::IdentityValidator;
use crate::identity_tree::initializer::TreeInitializer;
use crate::identity_tree::{
    Hash, InclusionProof, ProcessedStatus, RootItem, TreeState, TreeVersionReadOps,
};
use crate::prover::map::initialize_prover_maps;
use crate::prover::repository::ProverRepository;
use crate::prover::validator::ProofValidator;
use crate::prover::{ProverConfig, ProverType};
use crate::server::data::{
    InclusionProofResponse, ListBatchSizesResponse, VerifySemaphoreProofQuery,
    VerifySemaphoreProofRequest, VerifySemaphoreProofResponse,
};
use crate::server::error::Error as ServerError;
use crate::utils::retry_tx;

pub struct App {
    pub database:           Arc<Database>,
    pub identity_processor: Arc<dyn IdentityProcessor>,
    pub prover_repository:  Arc<ProverRepository>,
    tree_state:             OnceLock<TreeState>,
    pub config:             Config,

    pub identity_validator: IdentityValidator,
    pub proof_validator:    Arc<ProofValidator>,
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
        let db = Database::new(&config.database).await?;
        let database = Arc::new(db);
        let mut provers: HashSet<ProverConfig> = database.get_provers().await?;

        let non_inserted_provers =
            Self::merge_env_provers(&config.app.provers_urls.0, &mut provers);

        database.insert_provers(non_inserted_provers).await?;

        let (insertion_prover_map, deletion_prover_map) = initialize_prover_maps(provers)?;

        let prover_repository = Arc::new(ProverRepository::new(
            insertion_prover_map,
            deletion_prover_map,
        ));

        let proof_validator = Arc::new(ProofValidator::new(&config));

        let identity_processor: Arc<dyn IdentityProcessor> = if config.offchain_mode.enabled {
            Arc::new(OffChainIdentityProcessor::new(database.clone()))
        } else {
            let ethereum = Ethereum::new(&config).await?;

            let identity_manager = Arc::new(IdentityManager::new(&config, ethereum.clone()).await?);

            Arc::new(OnChainIdentityProcessor::new(
                ethereum.clone(),
                config.clone(),
                database.clone(),
                identity_manager.clone(),
                prover_repository.clone(),
                proof_validator.clone(),
            )?)
        };

        let identity_validator = IdentityValidator::new(&config);

        let app = Arc::new(Self {
            database,
            identity_processor,
            prover_repository,
            tree_state: OnceLock::new(),
            config,
            identity_validator,
            proof_validator,
        });

        Ok(app)
    }

    /// Initializes the tree state. This should only ever be called once.
    /// Attempts to call this method more than once will result in a panic.
    pub async fn init_tree(self: Arc<Self>) -> anyhow::Result<()> {
        let tree_state = TreeInitializer::new(
            self.database.clone(),
            self.identity_processor.clone(),
            self.config.tree.clone(),
        )
        .run()
        .await?;

        self.tree_state.set(tree_state).map_err(|_| {
            anyhow::anyhow!(
                "Failed to set tree state. 'App::init_tree' should only be called once."
            )
        })?;

        Ok::<(), anyhow::Error>(())
    }

    pub fn tree_state(&self) -> anyhow::Result<&TreeState> {
        Ok(self
            .tree_state
            .get()
            .ok_or(ServerError::TreeStateUninitialized)?)
    }

    /// Queues an insert into the merkle tree.
    ///
    /// # Errors
    ///
    /// Will return `Err` if identity is already queued, or in the tree, or the
    /// queue malfunctions.
    #[instrument(level = "debug", skip(self))]
    pub async fn insert_identity(&self, commitment: Hash) -> Result<(), ServerError> {
        if self.identity_validator.is_initial_leaf(&commitment) {
            warn!(?commitment, "Attempt to insert initial leaf.");
            return Err(ServerError::InvalidCommitment);
        }

        if !self.prover_repository.has_insertion_provers().await {
            warn!(
                ?commitment,
                "Identity Manager has no insertion provers. Add provers with /addBatchSize \
                 request."
            );
            return Err(ServerError::NoProversOnIdInsert);
        }

        if !self.identity_validator.is_reduced(commitment) {
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
        if !self.prover_repository.has_deletion_provers().await {
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
            if self.identity_validator.is_initial_leaf(new_commitment) {
                warn!(
                    ?new_commitment,
                    "Attempt to insert initial leaf in recovery."
                );
                return Err(ServerError::InvalidCommitment);
            }

            if !self.prover_repository.has_insertion_provers().await {
                warn!(
                    ?new_commitment,
                    "Identity Manager has no provers. Add provers with /addBatchSize request."
                );
                return Err(ServerError::NoProversOnIdInsert);
            }

            if !self.identity_validator.is_reduced(*new_commitment) {
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
        self.prover_repository
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
        self.prover_repository
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
        let batches = self.prover_repository.list_batch_sizes().await?;

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
        if self.identity_validator.is_initial_leaf(commitment) {
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
            self.config.tree.tree_depth,
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
