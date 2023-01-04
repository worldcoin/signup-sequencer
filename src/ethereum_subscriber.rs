use crate::{
    contracts::{Contracts, MemberAddedEvent},
    database::{
        ConfirmedIdentityEvent, Database, Error as DatabaseError, IdentityConfirmationResult,
    },
    ethereum::{EventError, Log},
    identity_committer::IdentityCommitter,
    identity_tree::{SharedTreeState, TreeState},
};
use futures::{StreamExt, TryStreamExt};
use semaphore::Field;
use std::{cmp::min, collections::HashMap, sync::Arc, time::Duration};
use thiserror::Error;
use tokio::{sync::RwLock, task::JoinHandle, time::sleep};
use tracing::{error, info, instrument, warn};

struct RunningInstance {
    #[allow(dead_code)]
    handle: JoinHandle<eyre::Result<()>>,
}

impl RunningInstance {
    fn shutdown(self) {
        info!("Sending a shutdown signal to the subscriber.");
        self.handle.abort();
    }
}

pub struct EthereumSubscriber {
    instance:           RwLock<Option<RunningInstance>>,
    starting_block:     u64,
    database:           Arc<Database>,
    contracts:          Arc<Contracts>,
    tree_state:         SharedTreeState,
    identity_committer: Arc<IdentityCommitter>,
}

impl EthereumSubscriber {
    pub fn new(
        starting_block: u64,
        database: Arc<Database>,
        contracts: Arc<Contracts>,
        tree_state: SharedTreeState,
        identity_committer: Arc<IdentityCommitter>,
    ) -> Self {
        Self {
            instance: RwLock::new(None),
            starting_block,
            database,
            contracts,
            tree_state,
            identity_committer,
        }
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn start(&self, refresh_rate: Duration) {
        let mut instance = self.instance.write().await;
        if instance.is_some() {
            info!("Chain Subscriber already running");
            return;
        }

        let mut starting_block = self.starting_block;
        let database = self.database.clone();
        let tree_state = self.tree_state.clone();
        let contracts = self.contracts.clone();
        let identity_committer = self.identity_committer.clone();

        let handle = tokio::spawn(async move {
            loop {
                sleep(refresh_rate).await;

                let processed_block = Self::process_events_internal(
                    starting_block,
                    tree_state.clone(),
                    contracts.clone(),
                    database.clone(),
                    identity_committer.clone(),
                )
                .await;
                match processed_block {
                    Ok(block_number) => starting_block = block_number + 1,
                    Err(error) => {
                        error!(?error, "Couldn't process events update");
                    }
                }
            }
        });
        *instance = Some(RunningInstance { handle });
    }

    #[instrument(level = "info", skip_all)]
    pub async fn process_initial_events(&mut self) -> Result<(), Error> {
        let end_block = self
            .contracts
            .confirmed_block_number()
            .await
            .map_err(Error::Event)?;

        let last_db_block = Self::process_cached_events(
            self.starting_block,
            end_block,
            self.tree_state.clone(),
            self.database.clone(),
        )
        .await?;
        let processed_block = Self::process_blockchain_events(
            last_db_block + 1,
            end_block,
            self.tree_state.clone(),
            self.contracts.clone(),
            self.database.clone(),
            self.identity_committer.clone(),
        )
        .await?;
        self.starting_block = processed_block + 1;
        Ok(())
    }

    async fn process_events_internal(
        start_block: u64,
        tree_state: SharedTreeState,
        contracts: Arc<Contracts>,
        database: Arc<Database>,
        identity_committer: Arc<IdentityCommitter>,
    ) -> Result<u64, Error> {
        let end_block = contracts
            .confirmed_block_number()
            .await
            .map_err(Error::Event)?;

        Self::process_blockchain_events(
            start_block,
            end_block,
            tree_state,
            contracts,
            database,
            identity_committer,
        )
        .await
    }

    async fn process_cached_events(
        start_block: u64,
        end_block: u64,
        tree_state: SharedTreeState,
        database: Arc<Database>,
    ) -> Result<u64, Error> {
        if start_block > end_block {
            return Ok(end_block);
        }

        let last_cached_block = database.get_block_number().await.unwrap();

        info!(
            start_block,
            end_block, last_cached_block, "Processing cached events"
        );

        let events = database
            .load_logs(
                i64::try_from(start_block).unwrap(),
                Some(i64::try_from(end_block).unwrap()),
            )
            .await
            .map_err(Error::Database)?;
        let root = events.last().map(|event| event.1);
        let leaves = events.iter().map(|event| event.0);
        let count = leaves.len();

        let mut tree = tree_state.write().await.unwrap_or_else(|e| {
            error!(?e, "Failed to obtain tree lock in process_events.");
            panic!("Sequencer potentially deadlocked, terminating.");
        });

        // Insert
        let index = tree.next_leaf;
        tree.merkle_tree.set_range(index, leaves);
        tree.next_leaf += count;

        // Check root
        if let Some(root) = root {
            if root != tree.merkle_tree.root() {
                error!(computed_root = ?tree.merkle_tree.root(), event_root = ?root, "Root mismatch between event and computed tree.");
                return Err(Error::RootMismatch);
            }
        }

        Ok(min(end_block, last_cached_block))
    }

    async fn process_blockchain_events(
        start_block: u64,
        end_block: u64,
        tree_state: SharedTreeState,
        contracts: Arc<Contracts>,
        database: Arc<Database>,
        identity_committer: Arc<IdentityCommitter>,
    ) -> Result<u64, Error> {
        if start_block > end_block {
            return Ok(end_block);
        }

        info!(
            start_block,
            end_block, "processing blockchain events in ethereum subscriber"
        );

        let mut events = contracts.fetch_events(start_block, Some(end_block)).boxed();

        let mut tree = tree_state.write().await.unwrap_or_else(|e| {
            error!(?e, "Failed to obtain tree lock in process_events.");
            panic!("Sequencer potentially deadlocked, terminating.");
        });

        let mut wake_up_committer = false;

        loop {
            let event = match events.try_next().await.map_err(Error::Event)? {
                Some(a) => a,
                None => break,
            };

            let identity = ConfirmedIdentityEvent::try_from(event)?;

            Self::log_event_errors(
                &tree,
                &contracts.initial_leaf(),
                tree.next_leaf,
                &identity.leaf,
            )?;

            // Cache event
            database
                .save_log(&identity)
                .await
                .map_err(Error::Database)?;

            // Insert
            let index = tree.next_leaf;
            tree.merkle_tree.set(index, identity.leaf);
            tree.next_leaf += 1;

            // Check root
            if identity.root != tree.merkle_tree.root() {
                error!(computed_root = ?tree.merkle_tree.root(), event_root = ?identity.root, "Root mismatch between event and computed tree.");
                return Err(Error::RootMismatch);
            }

            // Remove from pending identities
            let queue_status = database
                .confirm_identity_and_retrigger_stale_recods(&identity.leaf)
                .await
                .map_err(Error::Database)?;
            if let IdentityConfirmationResult::RetriggerProcessing = queue_status {
                wake_up_committer = true;
            }
        }

        if wake_up_committer {
            error!(
                "event sequencing inconsistent between chain and identity committer. re-org \
                 happened?"
            );
            identity_committer.notify_queued().await;
        }

        Ok(end_block)
    }

    #[allow(clippy::cognitive_complexity)]
    fn log_event_errors(
        tree: &TreeState,
        initial_leaf: &Field,
        index: usize,
        leaf: &Field,
    ) -> Result<(), Error> {
        // Check leaf index is valid
        if index >= tree.merkle_tree.num_leaves() {
            error!(?index, ?leaf, num_leaves = ?tree.merkle_tree.num_leaves(), "Received event out of range");
            return Err(Error::EventOutOfRange);
        }

        // Check if leaf value is valid
        if leaf == initial_leaf {
            error!(?index, ?leaf, "Inserting empty leaf");
            return Ok(());
        }

        // Check duplicates
        if let Some(previous) = tree.merkle_tree.leaves()[..index]
            .iter()
            .position(|l| l == leaf)
        {
            error!(
                ?index,
                ?leaf,
                ?previous,
                "Received event for already inserted leaf."
            );
        }

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn check_leaves(&self) {
        let tree = self.tree_state.read().await.unwrap_or_else(|e| {
            error!(?e, "Failed to obtain tree lock in check_leaves.");
            panic!("Sequencer potentially deadlocked, terminating.");
        });
        let next_leaf = tree.next_leaf;
        let initial_leaf = self.contracts.initial_leaf();

        let mut visited_identities = HashMap::<Field, usize>::new();
        for (index, &leaf) in tree.merkle_tree.leaves().iter().enumerate() {
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
                if let Some(previous) = visited_identities.get(&leaf) {
                    error!(?index, ?leaf, ?previous, "Leaf not unique.");
                }
            }
            visited_identities.insert(leaf, index);
        }
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn check_health(&self) {
        let tree = self.tree_state.read().await.unwrap_or_else(|e| {
            error!(?e, "Failed to obtain tree lock in check_leaves.");
            panic!("Sequencer potentially deadlocked, terminating.");
        });
        let initial_leaf = self.contracts.initial_leaf();

        if tree.next_leaf > 0 {
            if let Err(error) = self
                .contracts
                .assert_valid_root(tree.merkle_tree.root())
                .await
            {
                error!(root = ?tree.merkle_tree.root(), %error, "Root not valid on-chain.");
            } else {
                info!(root = ?tree.merkle_tree.root(), "Root matches on-chain root.");
            }
        } else {
            // TODO: This should still be checkable.
            info!(root = ?tree.merkle_tree.root(), "Empty tree, not checking root.");
        }

        // Check tree health
        let next_leaf = tree
            .merkle_tree
            .leaves()
            .iter()
            .rposition(|&l| l != initial_leaf)
            .map_or(0, |i| i + 1);
        let used_leaves = &tree.merkle_tree.leaves()[..next_leaf];
        let skipped = used_leaves.iter().filter(|&&l| l == initial_leaf).count();
        let mut dedup = used_leaves
            .iter()
            .filter(|&&l| l != initial_leaf)
            .collect::<Vec<_>>();
        dedup.sort();
        dedup.dedup();
        let unique = dedup.len();
        let duplicates = used_leaves.len() - skipped - unique;
        let total = tree.merkle_tree.num_leaves();
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

    pub async fn shutdown(&self) {
        let mut instance = self.instance.write().await;
        instance.take().map_or_else(
            || {
                info!("Subscriber not running.");
            },
            |instance| {
                instance.shutdown();
            },
        );
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("Root mismatch between event and computed tree.")]
    RootMismatch,
    #[error("Received event out of range")]
    EventOutOfRange,
    #[error("Event error: {0}")]
    Event(#[source] EventError),
    #[error("Database error: {0}")]
    Database(#[source] DatabaseError),
    #[error("Integer conversion error: {0}")]
    Conversion(String),
}

struct IdentityCommitment {
    leaf: Field,
    root: Field,
}

impl From<MemberAddedEvent> for IdentityCommitment {
    fn from(value: MemberAddedEvent) -> Self {
        Self {
            leaf: value.identity_commitment.into(),
            root: value.root.into(),
        }
    }
}

impl TryFrom<Log<MemberAddedEvent>> for ConfirmedIdentityEvent {
    type Error = Error;

    fn try_from(value: Log<MemberAddedEvent>) -> Result<Self, Self::Error> {
        let commitment = IdentityCommitment::from(value.event);

        let block_index: i64 = value
            .block_index
            .try_into()
            .map_err(|e: &str| Error::Conversion(e.to_owned()))?;

        let transaction_index: i32 = value
            .transaction_index
            .try_into()
            .map_err(|e: &str| Error::Conversion(e.to_owned()))?;

        let log_index: i32 = value
            .log_index
            .try_into()
            .map_err(|e: &str| Error::Conversion(e.to_owned()))?;

        Ok(Self {
            block_index,
            transaction_index,
            log_index,
            raw_log: value.raw_log,
            leaf: commitment.leaf,
            root: commitment.root,
        })
    }
}
