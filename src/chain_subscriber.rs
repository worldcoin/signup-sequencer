use crate::{
    contracts::Contracts,
    database::{Database, Error as DatabaseError, IsExpectedResponse},
    ethereum::EventError,
    identity_committer::IdentityCommitter,
    tree::SharedTreeState,
};
use cli_batteries::await_shutdown;
use futures::{pin_mut, StreamExt, TryStreamExt};
use std::{cmp::max, sync::Arc, time::Duration};
use thiserror::Error;
use tokio::{select, sync::RwLock, task::JoinHandle, time::sleep};
use tracing::{debug, error, info, instrument, warn};

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

pub struct ChainSubscriber {
    instance:           RwLock<Option<RunningInstance>>,
    starting_block:     u64,
    database:           Arc<Database>,
    contracts:          Arc<Contracts>,
    tree_state:         SharedTreeState,
    identity_committer: Arc<IdentityCommitter>,
}

impl ChainSubscriber {
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
    pub async fn start(&self) {
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
                sleep(Duration::from_secs(2)).await;
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
    pub async fn process_events(&mut self) -> Result<(), Error> {
        let processed_block = Self::process_events_internal(
            self.starting_block,
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
        starting_block: u64,
        tree_state: SharedTreeState,
        contracts: Arc<Contracts>,
        database: Arc<Database>,
        identity_committer: Arc<IdentityCommitter>,
    ) -> Result<u64, Error> {
        let mut tree = tree_state.write().await.unwrap_or_else(|e| {
            error!(?e, "Failed to obtain tree lock in process_events.");
            panic!("Sequencer potentially deadlocked, terminating.");
        });

        let initial_leaf = contracts.initial_leaf();

        let end_block = contracts
            .confirmed_block_number()
            .await
            .map_err(Error::EventError)?;

        if starting_block > end_block {
            return Ok(end_block);
        }
        info!(
            starting_block,
            end_block, "processing events in chain subscriber"
        );

        let mut events = contracts
            .fetch_events(
                starting_block,
                Some(end_block),
                tree.next_leaf,
                database.clone(),
            )
            .boxed();
        let shutdown = await_shutdown();
        pin_mut!(shutdown);

        let mut retrigger_processing = Option::<i64>::None;

        loop {
            let (index, leaf, root) = select! {
                v = events.try_next() => match v.map_err(Error::EventError)? {
                    Some(a) => a,
                    None => break,
                },
                _ = &mut shutdown => return Err(Error::Interrupted),
            };
            debug!(?index, ?leaf, ?root, "Received event");

            // Confirm if this is a node expected by the identity committer
            // If not, we need to re-add identities to the work queue
            // We only care about the first failure as everything else needs to be
            // recomputed anyway
            let is_expected = database
                .is_expected_identity_confirmation(&leaf)
                .await
                .map_err(Error::DatabaseError)?;
            if retrigger_processing.is_none() {
                if let IsExpectedResponse::NotExpected {
                    starting: row_index,
                } = is_expected
                {
                    retrigger_processing = Some(row_index);
                }
            }

            // Check leaf index is valid
            if index >= tree.merkle_tree.num_leaves() {
                error!(?index, ?leaf, num_leaves = ?tree.merkle_tree.num_leaves(), "Received event out of range");
                return Err(Error::EventOutOfRange);
            }

            // Check if leaf value is valid
            if leaf == initial_leaf {
                error!(?index, ?leaf, "Inserting empty leaf");
                continue;
            }

            // Check leaf value with existing value
            let existing = tree.merkle_tree.leaves()[index];
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
            if index != tree.next_leaf {
                error!(
                    ?index,
                    ?tree.next_leaf,
                    ?leaf,
                    "Event leaf index does not match expected leaf index."
                );
            }

            // Check duplicates
            if let Some(previous) = tree.merkle_tree.leaves()[..index]
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
            tree.merkle_tree.set(index, leaf);
            tree.next_leaf = max(tree.next_leaf, index + 1);

            // let index = tree_state.next_leaf.fetch_add(1, Ordering::Relaxed);
            // tree.merkle_tree.set(index, leaf);
            // tree.next_leaf = max(tree.next_leaf, index + 1);

            // Check root
            if root != tree.merkle_tree.root() {
                error!(computed_root = ?tree.merkle_tree.root(), event_root = ?root, "Root mismatch between event and computed tree.");
                return Err(Error::RootMismatch);
            }

            // Remove from pending identities
            if let IsExpectedResponse::NotExpected {
                starting: row_index,
            } = is_expected
            {
                database
                    .pending_identity_confirmed(row_index, &leaf)
                    .await
                    .map_err(Error::DatabaseError)?;
            }
        }

        if let Some(first_invalid_row) = retrigger_processing {
            error!(
                first_invalid_row,
                "event sequencing inconsistent between chain and identity committer. re-org \
                 happened?"
            );
            database
                .retrigger_pending_identities(first_invalid_row)
                .await
                .map_err(Error::DatabaseError)?;
            identity_committer.notify_queued().await;
        }

        Ok(end_block)
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn check_leaves(&self) {
        let tree = self.tree_state.read().await.unwrap_or_else(|e| {
            error!(?e, "Failed to obtain tree lock in check_leaves.");
            panic!("Sequencer potentially deadlocked, terminating.");
        });
        let next_leaf = tree.next_leaf;
        let initial_leaf = self.contracts.initial_leaf();
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
                if let Some(previous) = tree.merkle_tree.leaves()[..index]
                    .iter()
                    .position(|&l| l == leaf)
                {
                    error!(?index, ?leaf, ?previous, "Leaf not unique.");
                }
            }
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
        if let Some(instance) = instance.take() {
            instance.shutdown();
        } else {
            info!("Subscriber not running.");
        }
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("Interrupted")]
    Interrupted,
    #[error("Root mismatch between event and computed tree.")]
    RootMismatch,
    #[error("Received event out of range")]
    EventOutOfRange,
    #[error("Event error: {0}")]
    EventError(#[source] EventError),
    #[error("Database error: {0}")]
    DatabaseError(#[source] DatabaseError),
}
