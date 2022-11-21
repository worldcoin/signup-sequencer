use crate::{
    contracts::{self, Contracts},
    database::{self, Database},
    ethereum::{self, Ethereum, EventError},
    identity_committer::IdentityCommitter,
    server::Error as ServerError,
    timed_read_progress_lock::TimedReadProgressLock,
};
use clap::Parser;
use cli_batteries::await_shutdown;
use core::cmp::max;
use ethers::types::U256;
use eyre::Result as EyreResult;
use futures::{pin_mut, StreamExt, TryFutureExt, TryStreamExt};
use semaphore::{
    merkle_tree::Hasher,
    poseidon_tree::{PoseidonHash, PoseidonTree, Proof},
    Field,
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use thiserror::Error;
use tokio::{select, try_join};
use tracing::{debug, error, info, instrument, warn};

pub type Hash = <PoseidonHash as Hasher>::Hash;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonCommitment {
    pub last_block:  u64,
    pub commitments: Vec<Hash>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexResponse {
    identity_index: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InclusionProofResponse {
    Proof { root: Field, proof: Proof },
    Pending,
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

pub struct TreeState {
    pub next_leaf:   AtomicUsize,
    pub merkle_tree: TimedReadProgressLock<PoseidonTree>,
}

impl TreeState {
    #[must_use]
    pub fn new(tree_depth: usize, initial_leaf: Field, lock_timeout: Duration) -> Self {
        Self {
            next_leaf:   AtomicUsize::new(0),
            merkle_tree: TimedReadProgressLock::new(
                lock_timeout,
                PoseidonTree::new(tree_depth, initial_leaf),
            ),
        }
    }
}

pub struct App {
    database:           Arc<Database>,
    #[allow(dead_code)]
    ethereum:           Ethereum,
    contracts:          Arc<Contracts>,
    identity_committer: IdentityCommitter,
    tree_state:         Arc<TreeState>,
    last_block:         u64,
}

impl App {
    /// # Errors
    ///
    /// Will return `Err` if the internal Ethereum handler errors or if the
    /// `options.storage_file` is not accessible.
    #[allow(clippy::missing_panics_doc)] // TODO
    #[instrument(name = "App::new", level = "debug")]
    pub async fn new(options: Options) -> EyreResult<Self> {
        // Connect to Ethereum and Database
        let (database, (ethereum, contracts)) = {
            let db = Database::new(options.database);

            let eth = Ethereum::new(options.ethereum).and_then(|ethereum| async move {
                let contracts = Contracts::new(options.contracts, ethereum.clone()).await?;
                Ok((ethereum, Arc::new(contracts)))
            });

            // Connect to both in parallel
            try_join!(db, eth)?
        };

        let database = Arc::new(database);

        // Poseidon tree depth is one more than the contract's tree depth
        let tree_state = Arc::new(TreeState::new(
            contracts.tree_depth() + 1,
            contracts.initial_leaf(),
            Duration::from_secs(options.lock_timeout),
        ));

        let identity_committer =
            IdentityCommitter::new(database.clone(), contracts.clone(), tree_state.clone());

        let mut app = Self {
            database,
            ethereum,
            contracts,
            identity_committer,
            tree_state,
            last_block: options.starting_block,
        };

        // Sync with chain on start up
        app.check_leaves().await;

        match app.process_events().await {
            Err(Error::RootMismatch) => {
                error!("Error when rebuilding tree from cache. Retrying with db cache busted.");

                // Create a new empty MerkleTree and wipe out cache db
                app.tree_state = Arc::new(TreeState::new(
                    app.contracts.tree_depth() + 1,
                    app.contracts.initial_leaf(),
                    Duration::from_secs(options.lock_timeout),
                ));
                app.database.wipe_cache().await?;

                // Retry
                app.process_events().await?;
            }
            Err(e) => return Err(e.into()),
            Ok(_) => {}
        }

        app.check_health().await;
        app.identity_committer.start().await;
        Ok(app)
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

        let tree = self.tree_state.merkle_tree.read().await?;

        if commitment == self.contracts.initial_leaf() {
            warn!(?commitment, next = %self.tree_state.next_leaf.load(Ordering::Acquire), "Attempt to insert initial leaf.");
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
            warn!(?commitment, next = %self.tree_state.next_leaf.load(Ordering::Acquire), "Pending identity already exists.");
            return Err(ServerError::DuplicateCommitment);
        }

        if let Some(existing) = tree.leaves().iter().position(|&x| x == commitment) {
            warn!(?existing, ?commitment, next = %self.tree_state.next_leaf.load(Ordering::Acquire), "Commitment already exists in tree.");
            return Err(ServerError::DuplicateCommitment);
        };

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

        let merkle_tree = self.tree_state.merkle_tree.read().await.map_err(|e| {
            error!(?e, "Failed to obtain tree lock in inclusion_proof.");
            panic!("Sequencer potentially deadlocked, terminating.");
            #[allow(unreachable_code)]
            e
        })?;

        if let Some(identity_index) = merkle_tree.leaves().iter().position(|&x| x == *commitment) {
            let proof = merkle_tree
                .proof(identity_index)
                .ok_or(ServerError::IndexOutOfBounds)?;
            let root = merkle_tree.root();

            // Locally check the proof
            // TODO: Check the leaf index / path
            if !merkle_tree.verify(*commitment, &proof) {
                error!(
                    ?commitment,
                    ?identity_index,
                    ?root,
                    "Proof does not verify locally."
                );
                panic!("Proof does not verify locally.");
            }

            // Verify the root on chain
            if let Err(error) = self.contracts.assert_valid_root(root).await {
                error!(
                    computed_root = ?root,
                    ?error,
                    "Root mismatch between tree and contract."
                );
                return Err(ServerError::RootMismatch);
            }
            Ok(InclusionProofResponse::Proof { root, proof })
        } else if self
            .database
            .pending_identity_exists(group_id, commitment)
            .await?
        {
            Ok(InclusionProofResponse::Pending)
        } else {
            Err(ServerError::IdentityCommitmentNotFound)
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn check_leaves(&self) {
        let merkle_tree = self
            .tree_state
            .merkle_tree
            .read()
            .await
            .unwrap_or_else(|e| {
                error!(?e, "Failed to obtain tree lock in check_leaves.");
                panic!("Sequencer potentially deadlocked, terminating.");
            });
        let next_leaf = self.tree_state.next_leaf.load(Ordering::Acquire);
        let initial_leaf = self.contracts.initial_leaf();
        for (index, &leaf) in merkle_tree.leaves().iter().enumerate() {
            if index < next_leaf && leaf == initial_leaf {
                error!(
                    ?index,
                    ?leaf,
                    ?next_leaf,
                    "Leaf in non-empty spot set to initial leaf value."
                );
            }
            if index >= next_leaf && leaf != initial_leaf {
                error!(
                    ?index,
                    ?leaf,
                    ?next_leaf,
                    "Leaf in empty spot not set to initial leaf value."
                );
            }
            if leaf != initial_leaf {
                if let Some(previous) = merkle_tree.leaves()[..index]
                    .iter()
                    .position(|&l| l == leaf)
                {
                    error!(?index, ?leaf, ?previous, "Leaf not unique.");
                }
            }
        }
    }

    #[instrument(level = "info", skip_all)]
    async fn process_events(&mut self) -> Result<(), Error> {
        let mut merkle_tree = self
            .tree_state
            .merkle_tree
            .write()
            .await
            .unwrap_or_else(|e| {
                error!(?e, "Failed to obtain tree lock in process_events.");
                panic!("Sequencer potentially deadlocked, terminating.");
            });

        let initial_leaf = self.contracts.initial_leaf();
        let mut events = self
            .contracts
            .fetch_events(
                self.last_block,
                self.tree_state.next_leaf.load(Ordering::Acquire),
                self.database.clone(),
            )
            .boxed();
        let shutdown = await_shutdown();
        pin_mut!(shutdown);
        loop {
            let (index, leaf, root) = select! {
                v = events.try_next() => match v.map_err(Error::EventError)? {
                    Some(a) => a,
                    None => break,
                },
                _ = &mut shutdown => return Err(Error::Interrupted),
            };
            debug!(?index, ?leaf, ?root, "Received event");

            // Check leaf index is valid
            if index >= merkle_tree.num_leaves() {
                error!(?index, ?leaf, num_leaves = ?merkle_tree.num_leaves(), "Received event out of range");
                return Err(Error::EventOutOfRange);
            }

            // Check if leaf value is valid
            if leaf == initial_leaf {
                error!(?index, ?leaf, "Inserting empty leaf");
                continue;
            }

            // Check leaf value with existing value
            let existing = merkle_tree.leaves()[index];
            if existing != initial_leaf {
                if existing == leaf {
                    error!(?index, ?leaf, "Received event for already existing leaf.");
                    continue;
                }
                error!(
                    ?index,
                    ?leaf,
                    ?existing,
                    "Received event for already set leaf."
                );
            }

            // Check insertion counter
            if index != self.tree_state.next_leaf.load(Ordering::Acquire) {
                error!(
                    ?index,
                    ?self.tree_state.next_leaf,
                    ?leaf,
                    "Event leaf index does not match expected leaf index."
                );
            }

            // Check duplicates
            if let Some(previous) = merkle_tree.leaves()[..index]
                .iter()
                .position(|&l| l == leaf)
            {
                error!(
                    ?index,
                    ?leaf,
                    ?previous,
                    "Received event for already inserted leaf."
                );
            }

            // Insert
            merkle_tree.set(index, leaf);
            self.tree_state.next_leaf.store(
                max(self.tree_state.next_leaf.load(Ordering::Acquire), index + 1),
                Ordering::Release,
            );

            // Check root
            if root != merkle_tree.root() {
                error!(computed_root = ?merkle_tree.root(), event_root = ?root, "Root mismatch between event and computed tree.");
                return Err(Error::RootMismatch);
            }
        }
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn check_health(&self) {
        let merkle_tree = self
            .tree_state
            .merkle_tree
            .read()
            .await
            .unwrap_or_else(|e| {
                error!(?e, "Failed to obtain tree lock in check_health.");
                panic!("Sequencer potentially deadlocked, terminating.");
            });
        let initial_leaf = self.contracts.initial_leaf();
        // TODO: A re-org undoing events would cause this to fail.
        if self.tree_state.next_leaf.load(Ordering::Acquire) > 0 {
            if let Err(error) = self.contracts.assert_valid_root(merkle_tree.root()).await {
                error!(root = ?merkle_tree.root(), %error, "Root not valid on-chain.");
            } else {
                info!(root = ?merkle_tree.root(), "Root matches on-chain root.");
            }
        } else {
            // TODO: This should still be checkable.
            info!(root = ?merkle_tree.root(), "Empty tree, not checking root.");
        }

        // Check tree health
        let next_leaf = merkle_tree
            .leaves()
            .iter()
            .rposition(|&l| l != initial_leaf)
            .map_or(0, |i| i + 1);
        let used_leaves = &merkle_tree.leaves()[..next_leaf];
        let skipped = used_leaves.iter().filter(|&&l| l == initial_leaf).count();
        let mut dedup = used_leaves
            .iter()
            .filter(|&&l| l != initial_leaf)
            .collect::<Vec<_>>();
        dedup.sort();
        dedup.dedup();
        let unique = dedup.len();
        let duplicates = used_leaves.len() - skipped - unique;
        let total = merkle_tree.num_leaves();
        let available = total - next_leaf;
        #[allow(clippy::cast_precision_loss)]
        let fill = (next_leaf as f64) / (total as f64);
        if skipped == 0 && duplicates == 0 {
            info!(
                healthy = %unique,
                %available,
                %total,
                %fill,
                "Merkle tree is healthy, no duplicates or skipped leaves."
            );
        } else {
            error!(
                healthy = %unique,
                %duplicates,
                %skipped,
                used = %next_leaf,
                %available,
                %total,
                %fill,
                "Merkle tree has duplicate or skipped leaves."
            );
        }
        if next_leaf > available * 3 {
            if next_leaf > available * 19 {
                error!(
                    used = %next_leaf,
                    available = %available,
                    total = %total,
                    "Merkle tree is over 95% full."
                );
            } else {
                warn!(
                    used = %next_leaf,
                    available = %available,
                    total = %total,
                    "Merkle tree is over 75% full."
                );
            }
        }
    }

    pub async fn shutdown(&self) -> eyre::Result<()>{
        info!("Shutting down identity committer.");
        self.identity_committer.shutdown().await
    }
}

#[derive(Debug, Error)]
enum Error {
    #[error("Interrupted")]
    Interrupted,
    #[error("Root mismatch between event and computed tree.")]
    RootMismatch,
    #[error("Received event out of range")]
    EventOutOfRange,
    #[error("Event error: {0}")]
    EventError(#[source] EventError),
}
