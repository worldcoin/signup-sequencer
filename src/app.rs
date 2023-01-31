use std::{sync::Arc, time::Duration};

use anyhow::Result as AnyhowResult;
use clap::Parser;
use futures::TryFutureExt;
use hyper::StatusCode;
use serde::Serialize;
use tokio::try_join;
use tracing::{error, info, instrument, warn};

use crate::{
    contracts,
    contracts::{legacy::Contract as LegacyContract, IdentityManager, SharedIdentityManager},
    database::{self, Database},
    ethereum::{self, Ethereum},
    ethereum_subscriber::{Error as SubscriberError, EthereumSubscriber},
    identity_committer::IdentityCommitter,
    identity_tree::{
        Hash, InclusionProof, OldTreeState, SharedTreeState, TreeItem, TreeState, ValidityScope,
    },
    server::{Error as ServerError, ToResponseCode},
    timed_rw_lock::TimedRwLock,
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
    identity_manager:   SharedIdentityManager,
    identity_committer: Arc<IdentityCommitter>,
    #[allow(dead_code)]
    chain_subscriber:   EthereumSubscriber,
    old_tree_state:     SharedTreeState,
    merkle_tree:        TreeState,
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
        let cache_recovery_step_size = options.ethereum.cache_recovery_step_size;

        // Connect to Ethereum and Database
        let (database, (ethereum, identity_manager)) = {
            let db = Database::new(options.database);

            let eth = Ethereum::new(options.ethereum).and_then(|ethereum| async move {
                let identity_manager = if cfg!(feature = "batching-contract") {
                    panic!("The batching contract does not yet exist but was requested.");
                } else {
                    LegacyContract::new(options.contracts, ethereum.clone()).await?
                };
                Ok((ethereum, Arc::new(identity_manager)))
            });

            // Connect to both in parallel
            try_join!(db, eth)?
        };

        let database = Arc::new(database);

        // Poseidon tree depth is one more than the contract's tree depth
        let tree_state = Arc::new(TimedRwLock::new(
            Duration::from_secs(options.lock_timeout),
            OldTreeState::new(
                identity_manager.tree_depth() + 1,
                identity_manager.initial_leaf_value(),
            ),
        ));

        let merkle_tree =
            TreeState::new(contracts.tree_depth() + 1, contracts.initial_leaf()).await;

        let identity_committer = Arc::new(IdentityCommitter::new(
            database.clone(),
            identity_manager.clone(),
            merkle_tree.clone(),
        ));
        let chain_subscriber = EthereumSubscriber::new(
            options.starting_block,
            database.clone(),
            identity_manager.clone(),
            tree_state.clone(),
        );

        // Sync with chain on start up
        let mut app = Self {
            database,
            ethereum,
            identity_manager,
            identity_committer,
            chain_subscriber,
            old_tree_state: tree_state,
            merkle_tree,
        };

        // TODO Rethink these with new arch
        // select! {
        //     _ = app.load_initial_events(options.lock_timeout, options.starting_block,
        // cache_recovery_step_size) => {},     _ = await_shutdown() => return
        // Err(anyhow!("Innterrupted")) }
        //
        // // Basic sanity checks on the merkle tree
        // app.chain_subscriber.check_health().await;
        //
        // // Listen to Ethereum events
        // app.chain_subscriber.start(refresh_rate).await;

        // Process to push new identities to Ethereum
        app.identity_committer.start().await;

        Ok(app)
    }

    async fn load_initial_events(
        &mut self,
        lock_timeout: u64,
        starting_block: u64,
        cache_recovery_step_size: usize,
    ) -> AnyhowResult<()> {
        let mut root_mismatch_count = 0;
        loop {
            if root_mismatch_count == 1 {
                error!(cache_recovery_step_size, "Removing most recent cache.");
                self.database
                    .delete_most_recent_cached_events(cache_recovery_step_size as i64)
                    .await?;
            } else if root_mismatch_count == 2 {
                error!("Wiping out the entire cache.");
                self.database.wipe_cache().await?;
            } else if root_mismatch_count >= 3 {
                return Err(SubscriberError::RootMismatch.into());
            }

            match self.chain_subscriber.process_initial_events().await {
                Err(SubscriberError::RootMismatch) => {
                    error!("Error when rebuilding tree from cache.");
                    root_mismatch_count += 1;

                    // Create a new empty MerkleTree
                    self.old_tree_state = Arc::new(TimedRwLock::new(
                        Duration::from_secs(lock_timeout),
                        OldTreeState::new(
                            self.identity_manager.tree_depth() + 1,
                            self.identity_manager.initial_leaf_value(),
                        ),
                    ));

                    // Retry
                    self.chain_subscriber = EthereumSubscriber::new(
                        starting_block,
                        self.database.clone(),
                        self.identity_manager.clone(),
                        self.old_tree_state.clone(),
                    );
                }
                Err(e) => return Err(e.into()),
                Ok(_) => return Ok(()),
            }
        }
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

        let insertion_result = self
            .database
            .insert_identity_if_not_duplicate(&commitment)
            .await?;

        let Some(leaf_idx) = insertion_result else {
            warn!(?commitment, "Pending identity already exists.");
            return Err(ServerError::DuplicateCommitment);
        };

        self.sync_tree_to(leaf_idx).await?;

        self.identity_committer.notify_queued().await;

        Ok(InclusionProofResponse::from(
            self.merkle_tree
                .get_proof(&TreeItem {
                    leaf_index: leaf_idx,
                    scope:      ValidityScope::SequencerOnly,
                })
                .await,
        ))
    }

    async fn sync_tree_to(&self, leaf_idx: usize) -> Result<(), ServerError> {
        let tree = self.merkle_tree.get_latest_tree();
        let last_index = tree.last_leaf().await;
        if leaf_idx <= last_index {
            return Ok(()); // Someone sync'd first, we're up to date
        }
        let identities = self
            .database
            .get_updates_range(last_index + 1, leaf_idx)
            .await?;
        tree.append_many_fresh(&identities).await;
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
            .get_identity_index(commitment)
            .await?
            .ok_or(ServerError::InvalidCommitment)?;

        let proof = self.merkle_tree.get_proof(&item).await;

        Ok(InclusionProofResponse(proof))
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
