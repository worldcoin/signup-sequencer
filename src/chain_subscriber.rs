use crate::{app::{TreeState, SharedTreeState}, contracts::Contracts, database::Database, ethereum::EventError};
use cli_batteries::await_shutdown;
use futures::{pin_mut, StreamExt, TryStreamExt};
use std::{
    cmp::max,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use thiserror::Error;
use tokio::{select, sync::RwLock, task::JoinHandle, time::sleep};
use tracing::{debug, error, info, instrument};

struct RunningInstance {
    #[allow(dead_code)]
    handle: JoinHandle<eyre::Result<()>>,
}

pub struct ChainSubscriber {
    instance:   RwLock<Option<RunningInstance>>,
    database:   Arc<Database>,
    contracts:  Arc<Contracts>,
    tree_state: SharedTreeState,
}

impl ChainSubscriber {
    pub fn new(
        database: Arc<Database>,
        contracts: Arc<Contracts>,
        tree_state: SharedTreeState,
    ) -> Self {
        Self {
            instance: RwLock::new(None),
            database,
            contracts,
            tree_state,
        }
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn start(&self) {
        let mut instance = self.instance.write().await;
        if instance.is_some() {
            info!("Chain Subscriber already running");
            return;
        }

        let database = self.database.clone();
        let tree_state = self.tree_state.clone();
        let contracts = self.contracts.clone();
        let handle = tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(60)).await;
            }
        });
        *instance = Some(RunningInstance { handle });
    }

    #[instrument(level = "info", skip_all)]
    pub async fn process_events(&self) -> Result<(), Error> {
        let mut tree = self.tree_state.write().await.unwrap_or_else(|e| {
            error!(?e, "Failed to obtain tree lock in check_health.");
            panic!("Sequencer potentially deadlocked, terminating.");
        });

        let initial_leaf = self.contracts.initial_leaf();
        let mut events = self
            .contracts
            .fetch_events(
                0,
                tree.next_leaf,
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

            // Check root
            if root != tree.merkle_tree.root() {
                error!(computed_root = ?tree.merkle_tree.root(), event_root = ?root, "Root mismatch between event and computed tree.");
                return Err(Error::RootMismatch);
            }
        }
        Ok(())
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
}
