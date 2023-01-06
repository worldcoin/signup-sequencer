use crate::{
    contracts::{self, Contracts},
    database::{self, Database},
    ethereum::{self, Ethereum},
    ethereum_subscriber::{Error as SubscriberError, EthereumSubscriber},
    identity_committer::IdentityCommitter,
    identity_tree::{Hash, SharedTreeState, TreeState},
    server::{Error as ServerError, ToResponseCode},
    timed_rw_lock::TimedRwLock,
};
use anyhow::{anyhow, Result as AnyhowResult};
use clap::Parser;
use cli_batteries::await_shutdown;
use ethers::types::U256;
use futures::TryFutureExt;
use hyper::StatusCode;
use semaphore::{poseidon_tree::Proof, Field};
use serde::{ser::SerializeStruct, Serialize, Serializer};
use std::{sync::Arc, time::Duration};
use tokio::{select, try_join};
use tracing::{error, info, instrument, warn};

pub enum InclusionProofResponse {
    Proof { root: Field, proof: Proof },
    Pending,
}

impl ToResponseCode for InclusionProofResponse {
    fn to_response_code(&self) -> StatusCode {
        match self {
            Self::Proof { .. } => StatusCode::OK,
            Self::Pending => StatusCode::ACCEPTED,
        }
    }
}

impl Serialize for InclusionProofResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Proof { root, proof } => {
                let mut state = serializer.serialize_struct("InclusionProof", 2)?;
                state.serialize_field("root", root)?;
                state.serialize_field("proof", proof)?;
                state.end()
            }
            Self::Pending => serializer.serialize_str("pending"),
        }
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
    contracts:          Arc<Contracts>,
    identity_committer: Arc<IdentityCommitter>,
    #[allow(dead_code)]
    chain_subscriber:   EthereumSubscriber,
    tree_state:         SharedTreeState,
}

impl App {
    /// # Errors
    ///
    /// Will return `Err` if the internal Ethereum handler errors or if the
    /// `options.storage_file` is not accessible.
    #[allow(clippy::missing_panics_doc)] // TODO
    #[instrument(name = "App::new", level = "debug")]
    pub async fn new(options: Options) -> AnyhowResult<Self> {
        let refresh_rate = options.ethereum.refresh_rate;

        // Connect to Ethereum and Database
        let (database, ethereum) = {
            let db = Database::new(options.database);
            let eth = Ethereum::new(options.ethereum);

            // Connect to both in parallel
            try_join!(db, eth)?
        };

        let database = Arc::new(database);

        let contracts =
            Contracts::new(options.contracts, database.clone(), ethereum.clone()).await?;
        let contracts = Arc::new(contracts);

        // Poseidon tree depth is one more than the contract's tree depth
        let tree_state = Arc::new(TimedRwLock::new(
            Duration::from_secs(options.lock_timeout),
            TreeState::new(contracts.tree_depth() + 1, contracts.initial_leaf()),
        ));

        let identity_committer = Arc::new(IdentityCommitter::new(
            database.clone(),
            contracts.clone(),
            tree_state.clone(),
        ));
        let chain_subscriber = EthereumSubscriber::new(
            options.starting_block,
            database.clone(),
            contracts.clone(),
            tree_state.clone(),
            identity_committer.clone(),
        );

        // Sync with chain on start up
        let mut app = Self {
            database,
            ethereum,
            contracts,
            identity_committer,
            chain_subscriber,
            tree_state,
        };

        select! {
            _ = app.load_initial_events(options.lock_timeout, options.starting_block) => {},
            _ = await_shutdown() => return Err(anyhow!("Interrupted"))
        }

        // Basic sanity checks on the merkle tree
        app.chain_subscriber.check_leaves().await;
        app.chain_subscriber.check_health().await;

        // Listen to Ethereum events
        app.chain_subscriber.start(refresh_rate).await;

        // Process to push new identities to Ethereum
        app.identity_committer.start().await;

        Ok(app)
    }

    async fn load_initial_events(
        &mut self,
        lock_timeout: u64,
        starting_block: u64,
    ) -> AnyhowResult<()> {
        match self.chain_subscriber.process_initial_events().await {
            Err(SubscriberError::RootMismatch) => {
                error!("Error when rebuilding tree from cache. Retrying with db cache busted.");

                // Create a new empty MerkleTree and wipe out cache db
                self.tree_state = Arc::new(TimedRwLock::new(
                    Duration::from_secs(lock_timeout),
                    TreeState::new(
                        self.contracts.tree_depth() + 1,
                        self.contracts.initial_leaf(),
                    ),
                ));
                self.database.wipe_cache().await?;

                // Retry
                self.chain_subscriber = EthereumSubscriber::new(
                    starting_block,
                    self.database.clone(),
                    self.contracts.clone(),
                    self.tree_state.clone(),
                    self.identity_committer.clone(),
                );
                self.chain_subscriber.process_initial_events().await?;
            }
            Err(e) => return Err(e.into()),
            Ok(_) => {}
        }
        Ok(())
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
        group_id: usize,
        commitment: Hash,
    ) -> Result<(), ServerError> {
        if U256::from(group_id) != self.contracts.group_id() {
            return Err(ServerError::InvalidGroupId);
        }

        if commitment == self.contracts.initial_leaf() {
            warn!(?commitment, "Attempt to insert initial leaf.");
            return Err(ServerError::InvalidCommitment);
        }

        // Note the ordering of duplicate checks: since we never want to lose data,
        // pending identities are removed from the DB _after_ they are inserted into the
        // tree. Therefore this order of checks guarantees we will not insert a
        // duplicate.
        if self
            .database
            .pending_identity_exists(group_id, &commitment)
            .await?
        {
            warn!(?commitment, "Pending identity already exists.");
            return Err(ServerError::DuplicateCommitment);
        }

        {
            let tree = self.tree_state.read().await?;
            if let Some(existing) = tree
                .merkle_tree
                .leaves()
                .iter()
                .position(|&x| x == commitment)
            {
                warn!(?existing, ?commitment, next = %tree.next_leaf, "Commitment already exists in tree.");
                return Err(ServerError::DuplicateCommitment);
            }
        }

        self.database
            .insert_pending_identity(group_id, &commitment)
            .await?;

        self.identity_committer.notify_queued().await;

        Ok(())
    }

    /// # Errors
    ///
    /// Will return `Err` if the provided index is out of bounds.
    #[instrument(level = "debug", skip_all)]
    pub async fn inclusion_proof(
        &self,
        group_id: usize,
        commitment: &Hash,
    ) -> Result<InclusionProofResponse, ServerError> {
        if U256::from(group_id) != self.contracts.group_id() {
            return Err(ServerError::InvalidGroupId);
        }

        if commitment == &self.contracts.initial_leaf() {
            return Err(ServerError::InvalidCommitment);
        }

        {
            let tree = self.tree_state.read().await.map_err(|e| {
                error!(?e, "Failed to obtain tree lock in inclusion_proof.");
                panic!("Sequencer potentially deadlocked, terminating.");
                #[allow(unreachable_code)]
                e
            })?;

            if let Some(identity_index) = tree
                .merkle_tree
                .leaves()
                .iter()
                .position(|&x| x == *commitment)
            {
                let proof = tree
                    .merkle_tree
                    .proof(identity_index)
                    .ok_or(ServerError::IndexOutOfBounds)?;
                let root = tree.merkle_tree.root();

                // Locally check the proof
                // TODO: Check the leaf index / path
                if !tree.merkle_tree.verify(*commitment, &proof) {
                    error!(
                        ?commitment,
                        ?identity_index,
                        ?root,
                        "Proof does not verify locally."
                    );
                    panic!("Proof does not verify locally.");
                }

                drop(tree);

                // Verify the root on chain
                if let Err(error) = self.contracts.assert_valid_root(root).await {
                    error!(
                        computed_root = ?root,
                        ?error,
                        "Root mismatch between tree and contract."
                    );
                    return Err(ServerError::RootMismatch);
                }
                return Ok(InclusionProofResponse::Proof { root, proof });
            }
        }

        if self
            .database
            .pending_identity_exists(group_id, commitment)
            .await?
        {
            Ok(InclusionProofResponse::Pending)
        } else {
            Err(ServerError::IdentityCommitmentNotFound)
        }
    }

    /// # Errors
    ///
    /// Will return an Error if any of the components cannot be shut down
    /// gracefully.
    pub async fn shutdown(&self) -> AnyhowResult<()> {
        info!("Shutting down identity committer and chain subscriber.");
        self.chain_subscriber.shutdown().await;
        self.identity_committer.shutdown().await
    }
}
