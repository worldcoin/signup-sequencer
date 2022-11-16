use crate::{
    contracts::{self, Contracts},
    database::{self, Database},
    ethereum::{self, Ethereum, EventError},
    server::Error as ServerError,
    timed_rw_lock::TimedRwLock, utils::spawn_or_abort,
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
use tokio::{select, try_join, sync::mpsc};
use tracing::{error, info, instrument, warn};

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
pub struct InclusionProofResponse {
    pub root:  Field,
    pub proof: Proof,
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
    database:    Arc<Database>,
    #[allow(dead_code)]
    ethereum:    Ethereum,
    contracts:   Contracts,
    next_leaf:   AtomicUsize,
    last_block:  u64,
    merkle_tree: TimedRwLock<PoseidonTree>,
}

enum ItemType {
    Event((usize, Field, Field)),
    ServerReady,
}

impl App {
    /// # Errors
    ///
    /// Will return `Err` if the internal Ethereum handler errors or if the
    /// `options.storage_file` is not accessible.
    #[allow(clippy::missing_panics_doc)] // TODO
    #[instrument(name = "App::new", level = "debug")]
    pub async fn new(options: Options) -> EyreResult<Arc<Self>> {
        // Connect to Ethereum and Database
        let (database, (ethereum, contracts)) = {
            let db = Database::new(options.database);

            let eth = Ethereum::new(options.ethereum).and_then(|ethereum| async move {
                let contracts = Contracts::new(options.contracts, ethereum.clone()).await?;
                Ok((ethereum, contracts))
            });

            // Connect to both in parallel
            try_join!(db, eth)?
        };

        // Poseidon tree depth is one more than the contract's tree depth
        let merkle_tree = PoseidonTree::new(contracts.tree_depth() + 1, contracts.initial_leaf());

        let app = Arc::new(Self {
            database: Arc::new(database),
            ethereum,
            contracts,
            next_leaf: AtomicUsize::new(0),
            last_block: options.starting_block,
            merkle_tree: TimedRwLock::new(Duration::from_secs(options.lock_timeout), merkle_tree),
        });

        // Sync with chain on start up
        app.check_leaves().await;

        // TODO(gswirski)
        // 0. Load current max ETH block
        // 1. Load database events
        // 2. Set up ETH loop to continue reading events
        // 3. If root validator fails, restart with db wiped out (this should happen max once, right?)
        // 4. Once ETH loop catches up to max ETH block, mark app as ready to serve requests
        // 5. Continue reading ETH loop and appending to db cache

        let (server_ready_tx, mut sender_ready_rx) = mpsc::channel::<bool>(1);
        spawn_or_abort(app.clone().subscribe_events(server_ready_tx));
        sender_ready_rx.recv().await;

        app.check_health().await;

        Ok(app)
    }

    async fn subscribe_events(self: Arc<App>, ready_channel: mpsc::Sender<bool>) -> EyreResult<()> {
        let app = self;
        match app.clone().process_events(ready_channel.clone()).await {
            Err(Error::RootMismatch) => {
                error!("Error when rebuilding tree from cache. Retrying with db cache busted.");

                // Create a new empty MerkleTree and wipe out cache db
                let merkle_tree =
                    PoseidonTree::new(app.contracts.tree_depth() + 1, app.contracts.initial_leaf());
                {
                    let mut current_tree = app.merkle_tree.write().await?;
                    *current_tree = merkle_tree;
                }
                app.next_leaf.store(0, Ordering::Relaxed);
                app.database.wipe_cache().await?;

                // Retry
                app.clone().process_events(ready_channel).await?;
            }
            Err(e) => return Err(e.into()),
            Ok(_) => {}
        }

        info!("Subscribe events finishing");

        Ok(())
    }

    /// Inserts a new commitment into the Merkle tree. This will also update the
    /// contract's commitment tree.
    ///
    /// # Errors
    ///
    /// Will return `Err` if the Eth handler cannot insert the identity to the
    /// contract, or if writing to the storage file fails.
    #[instrument(level = "debug", skip_all)]
    pub async fn insert_identity(
        &self,
        group_id: usize,
        commitment: &Hash,
    ) -> Result<IndexResponse, ServerError> {
        if U256::from(group_id) != self.contracts.group_id() {
            return Err(ServerError::InvalidGroupId);
        }
        if commitment == &self.contracts.initial_leaf() {
            warn!(?commitment, next = %self.next_leaf.load(Ordering::Acquire), "Attempt to insert initial leaf.");
            return Err(ServerError::InvalidCommitment);
        }

        // Get a lock on the tree for the duration of this operation.
        // OPT: Sequence operations and allow concurrent inserts / transactions.
        let mut tree = self.merkle_tree.write().await.map_err(|e| {
            error!(?e, "Failed to obtain tree lock in insert_identity.");
            panic!("Sequencer potentially deadlocked, terminating.");
            #[allow(unreachable_code)]
            e
        })?;

        if let Some(existing) = tree.leaves().iter().position(|&x| x == *commitment) {
            warn!(?existing, ?commitment, next = %self.next_leaf.load(Ordering::Acquire), "Commitment already exists in tree.");
            return Err(ServerError::DuplicateCommitment);
        };

        // Fetch next leaf index
        let identity_index = self.next_leaf.fetch_add(1, Ordering::AcqRel);

        // Send Semaphore transaction
        self.contracts
            .insert_identity(commitment)
            .await
            .map_err(|e| {
                error!(?e, "Failed to insert identity to contract.");
                panic!("Failed to submit transaction, state synchronization lost.");
                #[allow(unreachable_code)]
                e
            })?;

        // Update  merkle tree
        tree.set(identity_index, *commitment);

        // Downgrade write lock to read lock
        let tree = tree.downgrade();

        // Check tree root
        if let Err(error) = self.contracts.assert_valid_root(tree.root()).await {
            error!(
                computed_root = ?tree.root(),
                ?error,
                "Root mismatch between tree and contract."
            );
            panic!("Root mismatch between tree and contract.");
        }

        // Immediately write the tree to storage, before anyone else can write.
        // TODO: Store tree in database

        Ok(IndexResponse { identity_index })
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

        let merkle_tree = self.merkle_tree.read().await.map_err(|e| {
            error!(?e, "Failed to obtain tree lock in inclusion_proof.");
            panic!("Sequencer potentially deadlocked, terminating.");
            #[allow(unreachable_code)]
            e
        })?;
        let identity_index = match merkle_tree.leaves().iter().position(|&x| x == *commitment) {
            Some(i) => i,
            None => return Err(ServerError::IdentityCommitmentNotFound),
        };

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

        Ok(InclusionProofResponse { root, proof })
    }

    #[instrument(level = "debug", skip_all)]
    async fn check_leaves(&self) {
        let merkle_tree = self.merkle_tree.read().await.unwrap_or_else(|e| {
            error!(?e, "Failed to obtain tree lock in check_leaves.");
            panic!("Sequencer potentially deadlocked, terminating.");
        });
        let next_leaf = self.next_leaf.load(Ordering::Acquire);
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
    async fn process_events(self: Arc<Self>, ready_channel: mpsc::Sender<bool>) -> Result<(), Error> {
        // TODO(gswirski): take into account confirmation del
        let ready_block = self.ethereum.fetch_last_block().await.map_err(Error::EventError)?;

        let initial_leaf = self.contracts.initial_leaf();
        let history = self
            .contracts
            .fetch_events(
                self.last_block,
                ready_block.as_u64(),
                self.next_leaf.load(Ordering::Acquire),
                self.database.clone(),
            )
            .map_ok(ItemType::Event);
        
        let marker = futures::stream::iter(vec![Ok(ItemType::ServerReady)]);

        let subscription = self
            .contracts
            .poll_events(
                ready_block.as_u64() + 1,
                self.database.clone(),
            )
            .map_ok(ItemType::Event);
        

        let mut chain_stream = history.chain(marker).chain(subscription).boxed();

        let shutdown = await_shutdown();
        pin_mut!(shutdown);

        loop {
            let (index, leaf, root) = select! {
                v = chain_stream.try_next() => match v.map_err(Error::EventError)? {
                    Some(ItemType::Event(a)) => a,
                    Some(ItemType::ServerReady) => {
                        ready_channel.send(true).await.map_err(|_| Error::InternalError)?;
                        continue;
                    }
                    None => break,
                },
                _ = &mut shutdown => return Err(Error::Interrupted),
            };

            let mut merkle_tree = self.merkle_tree.write().await.unwrap_or_else(|e| {
                error!(?e, "Failed to obtain tree lock in process_events.");
                panic!("Sequencer potentially deadlocked, terminating.");
            });
    
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
            if index != self.next_leaf.load(Ordering::Acquire) {
                error!(
                    ?index,
                    ?self.next_leaf,
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
            self.next_leaf.store(
                max(self.next_leaf.load(Ordering::Acquire), index + 1),
                Ordering::Release,
            );

            // Check root
            if root != merkle_tree.root() {
                error!(computed_root = ?merkle_tree.root(), event_root = ?root, "Root mismatch between event and computed tree.");
                return Err(Error::RootMismatch);
            }

            info!(?index, ?leaf, ?root, "Received event I");
        }
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn check_health(&self) {
        let merkle_tree = self.merkle_tree.read().await.unwrap_or_else(|e| {
            error!(?e, "Failed to obtain tree lock in check_health.");
            panic!("Sequencer potentially deadlocked, terminating.");
        });
        let initial_leaf = self.contracts.initial_leaf();
        // TODO: A re-org undoing events would cause this to fail.
        if self.next_leaf.load(Ordering::Acquire) > 0 {
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
    #[error("Internal error")]
    InternalError,
}
