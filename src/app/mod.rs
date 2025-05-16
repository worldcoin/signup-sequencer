use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use crate::app::error::VerifySemaphoreProofV2Error::RootAgeCheckingError;
use crate::app::error::{
    DeleteIdentityV2Error, InclusionProofV2Error, InsertIdentityV2Error,
    VerifySemaphoreProofV2Error,
};
use crate::config::Config;
use crate::contracts::IdentityManager;
use crate::database::methods::DbMethods as _;
use crate::database::{Database, IsolationLevel};
use crate::ethereum::Ethereum;
use crate::identity::processor::{
    IdentityProcessor, OffChainIdentityProcessor, OnChainIdentityProcessor,
};
use crate::identity::validator::IdentityValidator;
use crate::identity_tree::initializer::TreeInitializer;
use crate::identity_tree::{
    Hash, ProcessedStatus, RootItem, TreeState, TreeVersionReadOps, UnprocessedStatus,
};
use crate::prover::map::initialize_prover_maps;
use crate::prover::repository::ProverRepository;
use crate::prover::{ProverConfig, ProverType};
use crate::server::api_v1::data::{
    InclusionProofResponse, ListBatchSizesResponse, VerifySemaphoreProofQuery,
    VerifySemaphoreProofRequest, VerifySemaphoreProofResponse,
};
use crate::server::api_v1::error::Error as ServerError;
use chrono::{Duration, Utc};
use ruint::Uint;
use semaphore_rs::protocol::compression::CompressedProof;
use semaphore_rs::protocol::{verify_proof, Proof};
use semaphore_rs::Field;
use tracing::{error, info, instrument, warn};

pub mod error;

pub struct App {
    pub database: Arc<Database>,
    pub identity_processor: Arc<dyn IdentityProcessor>,
    pub prover_repository: Arc<ProverRepository>,
    tree_state: OnceLock<TreeState>,
    pub config: Config,

    pub identity_validator: IdentityValidator,
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

        let identity_processor: Arc<dyn IdentityProcessor> = if config.offchain_mode.enabled {
            Arc::new(OffChainIdentityProcessor::new(database.clone()).await?)
        } else {
            let ethereum = Ethereum::new(&config).await?;

            let identity_manager = Arc::new(IdentityManager::new(&config, ethereum.clone()).await?);

            Arc::new(
                OnChainIdentityProcessor::new(
                    ethereum.clone(),
                    config.clone(),
                    database.clone(),
                    identity_manager.clone(),
                    prover_repository.clone(),
                )
                .await?,
            )
        };

        let identity_validator = IdentityValidator::new(&config);

        let app = Arc::new(Self {
            database,
            identity_processor,
            prover_repository,
            tree_state: OnceLock::new(),
            config,
            identity_validator,
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

        let mut tx = self
            .database
            .begin_tx(IsolationLevel::ReadCommitted)
            .await?;

        if tx.identity_exists(commitment).await? {
            return Err(ServerError::DuplicateCommitment);
        }

        tx.insert_unprocessed_identity(commitment).await?;

        tx.commit().await?;

        Ok(())
    }

    /// Queues an insert into the merkle tree.
    ///
    /// # Errors
    ///
    /// Will return `Err` if identity is already queued, or in the tree, or the
    /// queue malfunctions.
    #[instrument(level = "debug", skip(self))]
    pub async fn insert_identity_v2(&self, commitment: Hash) -> Result<(), InsertIdentityV2Error> {
        if self.identity_validator.is_initial_leaf(&commitment) {
            warn!(?commitment, "Attempt to insert initial leaf.");
            return Err(InsertIdentityV2Error::InvalidCommitment);
        }

        if !self.identity_validator.is_reduced(commitment) {
            warn!(
                ?commitment,
                "The provided commitment is not an element of the field."
            );
            return Err(InsertIdentityV2Error::UnreducedCommitment);
        }

        let mut tx = self
            .database
            .begin_tx(IsolationLevel::RepeatableRead)
            .await?;

        let unprocessed = tx.get_unprocessed_commitment(&commitment).await?;
        if unprocessed.is_some() {
            return Err(InsertIdentityV2Error::DuplicateCommitment);
        }

        let processed = tx.get_tree_item(&commitment).await?;
        if let Some(processed) = processed {
            let latest_at_index = tx.get_tree_item_by_leaf_index(processed.leaf_index).await?;

            if let Some(latest_at_index) = latest_at_index {
                if latest_at_index.element == Hash::ZERO {
                    return Err(InsertIdentityV2Error::DeletedCommitment);
                }
            } else {
                return Err(InsertIdentityV2Error::DuplicateCommitment);
            }
        }

        tx.insert_unprocessed_identity(commitment).await?;

        tx.commit().await?;

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
        let mut tx = self
            .database
            .begin_tx(IsolationLevel::RepeatableRead)
            .await?;

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
            .get_tree_item(commitment)
            .await?
            .ok_or(ServerError::IdentityCommitmentNotFound)?
            .leaf_index;

        // Check if the id has already been deleted
        if self.tree_state()?.get_latest_tree().get_leaf(leaf_index) == Uint::ZERO {
            return Err(ServerError::IdentityAlreadyDeleted);
        }

        // Check if there are any deletions, if not, set the latest deletion timestamp
        // to now to ensure that the new deletion is processed by the next deletion
        // interval
        if tx.get_deletions().await?.is_empty() {
            tx.update_latest_deletion(Utc::now()).await?;
        }

        tx.insert_new_deletion(leaf_index, commitment).await?;

        tx.commit().await?;

        Ok(())
    }

    /// Queues a deletion from the merkle tree.
    ///
    /// # Errors
    ///
    /// Will return `Err` if identity is already queued, not in the tree, or the
    /// queue malfunctions.
    #[instrument(level = "debug", skip(self))]
    pub async fn delete_identity_v2(&self, commitment: Hash) -> Result<(), DeleteIdentityV2Error> {
        if self.identity_validator.is_initial_leaf(&commitment) {
            return Err(DeleteIdentityV2Error::InvalidCommitment);
        }

        if !self.identity_validator.is_reduced(commitment) {
            warn!(
                ?commitment,
                "The provided commitment is not an element of the field."
            );
            return Err(DeleteIdentityV2Error::UnreducedCommitment);
        }

        let mut tx = self
            .database
            .begin_tx(IsolationLevel::RepeatableRead)
            .await?;

        let processed = match tx.get_tree_item(&commitment).await? {
            None => {
                let unprocessed = tx.get_unprocessed_commitment(&commitment).await?;
                if unprocessed.is_some() {
                    return Err(DeleteIdentityV2Error::UnprocessedCommitment);
                } else {
                    return Err(DeleteIdentityV2Error::CommitmentNotFound);
                }
            }
            Some(tree_item) => tree_item,
        };

        let latest_at_index = match tx.get_tree_item_by_leaf_index(processed.leaf_index).await? {
            None => {
                // This should never happen as last query was returning row with that value
                return Err(DeleteIdentityV2Error::CommitmentNotFound);
            }
            Some(tree_item) => tree_item,
        };

        if latest_at_index.element == Hash::ZERO {
            return Err(DeleteIdentityV2Error::DeletedCommitment);
        }

        let deletion = tx.get_deletion(&commitment).await?;
        if deletion.is_some() {
            return Err(DeleteIdentityV2Error::DuplicateCommitmentDeletion);
        }

        // Check if there are any deletions, if not, set the latest deletion timestamp
        // to now to ensure that the new deletion is processed by the next deletion
        // interval
        if tx.count_deletions().await? == 0 {
            tx.update_latest_deletion(Utc::now()).await?;
        }

        tx.insert_new_deletion(processed.leaf_index, &commitment)
            .await?;

        tx.commit().await?;

        Ok(())
    }

    fn merge_env_provers(
        prover_urls: &[ProverConfig],
        existing_provers: &mut HashSet<ProverConfig>,
    ) -> HashSet<ProverConfig> {
        let options_set: HashSet<ProverConfig> = prover_urls
            .iter()
            .cloned()
            .map(|opt| ProverConfig {
                url: opt.url,
                batch_size: opt.batch_size,
                timeout_s: opt.timeout_s,
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

        if self
            .database
            .get_unprocessed_commitment(commitment)
            .await?
            .is_some()
        {
            return Ok(InclusionProofResponse {
                status: UnprocessedStatus::New.into(),
                root: None,
                proof: None,
                message: None,
            });
        }

        let item = self
            .database
            .get_tree_item(commitment)
            .await?
            .ok_or(ServerError::IdentityCommitmentNotFound)?;

        let tree_state = self.tree_state()?;
        if tree_state.latest_tree().get_last_sequence_id() < item.sequence_id {
            // If tree change was not yet applied to latest tree we treat it as it would be
            // unprocessed.
            return Ok(InclusionProofResponse {
                status: UnprocessedStatus::New.into(),
                root: None,
                proof: None,
                message: None,
            });
        }

        // If tree change was applied to latest tree we then look in the trees we have for proof
        let (leaf, proof) = self.tree_state()?.get_proof_for(&item);

        if leaf != *commitment {
            return Err(ServerError::InvalidCommitment);
        }

        Ok(proof.into())
    }

    /// # Errors
    ///
    /// Will return `Err` if the provided index is out of bounds.
    #[instrument(level = "debug", skip(self))]
    pub async fn inclusion_proof_v2(
        &self,
        commitment: Hash,
    ) -> Result<(Field, Field, semaphore_rs::poseidon_tree::Proof), InclusionProofV2Error> {
        if self.identity_validator.is_initial_leaf(&commitment) {
            return Err(InclusionProofV2Error::InvalidCommitment);
        }

        if !self.identity_validator.is_reduced(commitment) {
            warn!(
                ?commitment,
                "The provided commitment is not an element of the field."
            );
            return Err(InclusionProofV2Error::UnreducedCommitment);
        }

        let mut tx = self
            .database
            .begin_tx(IsolationLevel::RepeatableRead)
            .await?;

        let item = match tx.get_tree_item(&commitment).await? {
            None => {
                let unprocessed = tx.get_unprocessed_commitment(&commitment).await?;
                if unprocessed.is_some() {
                    return Err(InclusionProofV2Error::UnprocessedCommitment);
                } else {
                    return Err(InclusionProofV2Error::CommitmentNotFound);
                }
            }
            Some(tree_item) => tree_item,
        };

        let latest_at_index = match tx.get_tree_item_by_leaf_index(item.leaf_index).await? {
            None => {
                // This should never happen as last query was returning row with that value
                return Err(InclusionProofV2Error::CommitmentNotFound);
            }
            Some(tree_item) => tree_item,
        };

        if latest_at_index.element == Hash::ZERO {
            return Err(InclusionProofV2Error::DeletedCommitment);
        }

        let tree_state = self.tree_state()?;
        if tree_state.latest_tree().get_last_sequence_id() < item.sequence_id {
            // If tree change was not yet applied to latest tree we treat it as it would be
            // unprocessed.
            return Err(InclusionProofV2Error::UnprocessedCommitment);
        }

        // If tree change was applied to latest tree we look for inclusion proof
        // todo(piotrh): is it ok to use latest tree here?
        let (leaf, root, proof) = self
            .tree_state()?
            .get_latest_tree()
            .get_leaf_and_proof(item.leaf_index);

        if leaf != commitment {
            error!("Mismatch between database and in-memory tree. This should never happen.");
            return Err(InclusionProofV2Error::InvalidInternalState);
        }

        tx.commit().await?;

        Ok((leaf, root, proof))
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

        let proof = if request.is_proof_padded() {
            let proof_flat = request.proof.flatten();
            let compressed_flat = [proof_flat[0], proof_flat[1], proof_flat[2], proof_flat[3]];
            let compressed = CompressedProof::from_flat(compressed_flat);
            semaphore_rs::protocol::compression::decompress_proof(compressed)
                .ok_or_else(|| ServerError::InvalidProof)?
        } else {
            request.proof
        };

        let checked = verify_proof(
            request.root,
            request.nullifier_hash,
            request.signal_hash,
            request.external_nullifier_hash,
            &proof,
            self.config.tree.tree_depth,
        );

        match checked {
            Ok(true) => Ok(root_state.into()),
            Ok(false) => Err(ServerError::InvalidProof),
            Err(err) => {
                info!(?err, "verify_proof failed with error");
                Err(ServerError::ProverError)
            }
        }
    }

    fn is_proof_padded(proof: &Proof) -> bool {
        let Proof(_g1a, g2, g1b) = proof;

        g2.1[0].is_zero() && g2.1[1].is_zero() && g1b.0.is_zero() && g1b.1.is_zero()
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

        info!("Validating age max_root_age: {max_root_age:?}");

        let root = root_state.root;
        match root_state.status {
            // Pending status implies the batching or latest tree
            ProcessedStatus::Pending if latest_root == root || batching_root == root => {
                warn!("Root is pending - skipping");
                return Ok(());
            }
            // Processed status is hidden - this should never happen
            ProcessedStatus::Processed if processed_root == root => {
                warn!("Root is processed - skipping");
                return Ok(());
            }
            // Processed status is hidden, so it could be either processed or mined
            ProcessedStatus::Mined if processed_root == root || mined_root == root => {
                warn!("Root is mined - skipping");
                return Ok(());
            }
            _ => (),
        }

        let now = Utc::now();
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

        warn!("Root age: {root_age:?}");

        if root_age > max_root_age {
            Err(ServerError::RootTooOld)
        } else {
            Ok(())
        }
    }

    /// # Errors
    ///
    /// Will return `Err` if the provided proof is invalid.
    #[instrument(level = "debug", skip(self))]
    pub async fn verify_semaphore_proof_v2(
        &self,
        root: Field,
        signal_hash: Field,
        nullifier_hash: Field,
        external_nullifier_hash: Field,
        proof: Proof,
        max_root_age_seconds: Option<i64>,
    ) -> Result<bool, VerifySemaphoreProofV2Error> {
        let Some(root_state) = self.database.get_root_state(&root).await? else {
            return Err(VerifySemaphoreProofV2Error::InvalidRoot);
        };

        if let Some(max_root_age_seconds) = max_root_age_seconds {
            let max_root_age = Duration::seconds(max_root_age_seconds);
            let is_root_age_valid = self
                .validate_root_age_v2(max_root_age, &root_state)
                .map_err(RootAgeCheckingError)?;
            if !is_root_age_valid {
                return Err(VerifySemaphoreProofV2Error::RootTooOld);
            }
        }

        let proof = if Self::is_proof_padded(&proof) {
            let proof_flat = proof.flatten();
            let compressed_flat = [proof_flat[0], proof_flat[1], proof_flat[2], proof_flat[3]];
            let compressed = CompressedProof::from_flat(compressed_flat);
            semaphore_rs::protocol::compression::decompress_proof(compressed)
                .ok_or_else(|| VerifySemaphoreProofV2Error::DecompressingProofError)?
        } else {
            proof
        };

        verify_proof(
            root,
            nullifier_hash,
            signal_hash,
            external_nullifier_hash,
            &proof,
            self.config.tree.tree_depth,
        )
        .map_err(|_err| VerifySemaphoreProofV2Error::ProverError)
    }

    /// We consider root age valid in two cases:
    /// * root is current root of latest, processed or mined tree
    /// * root age (depending on state) is not greater than max_root_age
    fn validate_root_age_v2(
        &self,
        max_root_age: Duration,
        root_state: &RootItem,
    ) -> anyhow::Result<bool> {
        let tree_state = self.tree_state()?;
        let latest_root = tree_state.get_latest_tree().get_root();
        let processed_root = tree_state.get_processed_tree().get_root();
        let mined_root = tree_state.get_mined_tree().get_root();

        let root = root_state.root;
        match root_state.status {
            // Pending status implies the batching or latest tree, but batching tree is only
            // for internal processing purposes
            ProcessedStatus::Pending if latest_root == root => {
                return Ok(true);
            }
            // Processed status implies the processed tree
            ProcessedStatus::Processed if processed_root == root => {
                return Ok(true);
            }
            // Processed status implies the mined tree
            ProcessedStatus::Mined if mined_root == root => {
                return Ok(true);
            }
            _ => (),
        }

        let now = Utc::now();
        let root_age = match root_state.status {
            ProcessedStatus::Pending | ProcessedStatus::Processed => {
                now - root_state.pending_valid_as_of
            }
            ProcessedStatus::Mined => {
                if let Some(mined_at) = root_state.mined_valid_as_of {
                    now - mined_at
                } else {
                    error!("Root state does not have mined_at set while have mined status. This should never happen. Considering root age too old.");
                    return Err(anyhow::Error::msg(
                        "Unexpected error occurred while checking root age.",
                    ));
                }
            }
        };

        Ok(root_age <= max_root_age)
    }
}
