use anyhow::Result as AnyhowResult;
use clap::Parser;
use futures::TryFutureExt;
use hyper::StatusCode;
use semaphore::protocol::verify_proof;
use serde::Serialize;
use std::sync::Arc;
use tokio::try_join;
use tracing::{info, instrument, warn};

use crate::{
    contracts,
    contracts::{IdentityManager, SharedIdentityManager},
    database::{self, Database},
    ethereum::{self, Ethereum},
    identity_committer,
    identity_committer::IdentityCommitter,
    identity_tree::{
        CanonicalTreeBuilder, Hash, InclusionProof, RootItem, Status, TreeItem, TreeState,
    },
    prover,
    prover::batch_insertion::Prover as BatchInsertionProver,
    server::{Error as ServerError, ToResponseCode, VerifySemaphoreProofRequest},
};

#[derive(Serialize)]
#[serde(transparent)]
pub struct InclusionProofResponse(InclusionProof);

impl From<InclusionProof> for InclusionProofResponse {
    fn from(value: InclusionProof) -> Self {
        Self(value)
    }
}

impl ToResponseCode for InclusionProofResponse {
    fn to_response_code(&self) -> StatusCode {
        StatusCode::OK
    }
}

#[derive(Serialize)]
#[serde(transparent)]
pub struct VerifySemaphoreProofResponse(RootItem);

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
    pub prover: prover::Options,

    #[clap(flatten)]
    pub committer: identity_committer::Options,

    /// Block number to start syncing from
    #[clap(long, env, default_value = "0")]
    pub starting_block: u64,

    /// Timeout for the tree lock (seconds).
    #[clap(long, env, default_value = "120")]
    pub lock_timeout: u64,
}

pub struct App {
    database:           Arc<Database>,
    #[allow(dead_code)]
    ethereum:           Ethereum,
    identity_manager:   SharedIdentityManager,
    identity_committer: Arc<IdentityCommitter>,
    tree_state:         TreeState,
    snark_scalar_field: Hash,
}

impl App {
    /// # Errors
    ///
    /// Will return `Err` if the internal Ethereum handler errors or if the
    /// `options.storage_file` is not accessible.
    #[allow(clippy::missing_panics_doc)] // TODO
    #[instrument(name = "App::new", level = "debug")]
    pub async fn new(options: Options) -> AnyhowResult<Self> {
        // Connect to Ethereum and Database
        let (database, (ethereum, identity_manager)) = {
            let db = Database::new(options.database);
            let prover = BatchInsertionProver::new(&options.prover.batch_insertion)?;

            let eth = Ethereum::new(options.ethereum).and_then(|ethereum| async move {
                let identity_manager =
                    IdentityManager::new(options.contracts, ethereum.clone(), prover).await?;
                Ok((ethereum, Arc::new(identity_manager)))
            });

            // Connect to both in parallel
            try_join!(db, eth)?
        };
        let database = Arc::new(database);

        let tree_state = Self::initialize_tree(
            &database,
            // Poseidon tree depth is one more than the contract's tree depth
            identity_manager.tree_depth() + 1,
            identity_manager.initial_leaf_value(),
        )
        .await?;

        let identity_committer = Arc::new(IdentityCommitter::new(
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

        // Sync with chain on start up
        let app = Self {
            database,
            ethereum,
            identity_manager,
            identity_committer,
            tree_state,
            snark_scalar_field,
        };

        // Process to push new identities to Ethereum
        app.identity_committer.start().await;

        Ok(app)
    }

    async fn initialize_tree(
        database: &Database,
        tree_depth: usize,
        initial_leaf_value: Hash,
    ) -> AnyhowResult<TreeState> {
        let mut mined_builder = CanonicalTreeBuilder::new(tree_depth, initial_leaf_value);
        let mined_items = database.get_commitments_by_status(Status::Mined).await?;
        for update in mined_items {
            mined_builder.append(&update);
        }
        let mined = mined_builder.seal();
        let batching = mined.next_version().await;
        let latest = batching.next_version().await;
        let pending_items = database.get_commitments_by_status(Status::Pending).await?;
        latest.append_many_fresh(&pending_items).await;
        Ok(TreeState::new(mined, batching, latest))
    }

    /// Queues an insert into the merkle tree.
    ///
    /// # Errors
    ///
    /// Will return `Err` if identity is already queued, or in the tree, or the
    /// queue malfunctions.
    #[instrument(level = "debug", skip_all)]
    pub async fn insert_identity(
        &self,
        commitment: Hash,
    ) -> Result<InclusionProofResponse, ServerError> {
        if commitment == self.identity_manager.initial_leaf_value() {
            warn!(?commitment, "Attempt to insert initial leaf.");
            return Err(ServerError::InvalidCommitment);
        }

        if !self.identity_is_reduced(commitment) {
            warn!(
                ?commitment,
                "The provided commitment is not an element of the field."
            );
            return Err(ServerError::UnreducedCommitment);
        }

        let insertion_result = self
            .database
            .insert_identity_if_does_not_exist(&commitment)
            .await?;

        let Some(leaf_idx) = insertion_result else {
            warn!(?commitment, "Pending identity already exists.");

            return Err(ServerError::DuplicateCommitment);
        };

        self.sync_tree_to(leaf_idx).await?;

        self.identity_committer.notify_queued().await;

        Ok(InclusionProofResponse::from(
            self.tree_state
                .get_proof_for(&TreeItem {
                    leaf_index: leaf_idx,
                    status:     Status::Pending,
                })
                .await,
        ))
    }

    fn identity_is_reduced(&self, commitment: Hash) -> bool {
        commitment.lt(&self.snark_scalar_field)
    }

    async fn sync_tree_to(&self, leaf_idx: usize) -> Result<(), ServerError> {
        let tree = self.tree_state.get_latest_tree();
        let next_index = tree.next_leaf().await;
        if leaf_idx < next_index {
            return Ok(()); // Someone sync'd first, we're up to date
        }
        let identities = self
            .database
            .get_updates_in_range(next_index, leaf_idx)
            .await?;
        tree.append_many_fresh(&identities).await;

        let last_identity = tree.get_leaf(leaf_idx).await;
        self.database
            .insert_pending_root(&tree.get_root().await, &last_identity, leaf_idx)
            .await?;
        Ok(())
    }

    /// # Errors
    ///
    /// Will return `Err` if the provided index is out of bounds.
    #[instrument(level = "debug", skip_all)]
    pub async fn inclusion_proof(
        &self,
        commitment: &Hash,
    ) -> Result<InclusionProofResponse, ServerError> {
        if commitment == &self.identity_manager.initial_leaf_value() {
            return Err(ServerError::InvalidCommitment);
        }

        let item = self
            .database
            .get_identity_leaf_index(commitment)
            .await?
            .ok_or(ServerError::InvalidCommitment)?;

        let proof = self.tree_state.get_proof_for(&item).await;

        Ok(InclusionProofResponse(proof))
    }

    /// # Errors
    ///
    /// Will return `Err` if the provided proof is invalid.
    #[instrument(level = "debug", skip_all)]
    pub async fn verify_semaphore_proof(
        &self,
        request: &VerifySemaphoreProofRequest,
    ) -> Result<VerifySemaphoreProofResponse, ServerError> {
        let Some(root_state) = self.database.get_root_state(&request.root).await? else {
            return Err(ServerError::InvalidRoot)
        };

        let checked = verify_proof(
            request.root,
            request.nullifier_hash,
            request.signal_hash,
            request.external_nullifier_hash,
            &request.proof,
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

    /// # Errors
    ///
    /// Will return an Error if any of the components cannot be shut down
    /// gracefully.
    pub async fn shutdown(&self) -> AnyhowResult<()> {
        info!("Shutting down identity committer.");
        self.identity_committer.shutdown().await
    }
}
