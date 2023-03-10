use std::sync::Arc;

use anyhow::{anyhow, Result as AnyhowResult};
use clap::Parser;
use ethers::types::U256;
use once_cell::sync::Lazy;
use prometheus::{register_gauge, Gauge};
use tokio::{
    select,
    sync::{broadcast, mpsc, mpsc::error::TrySendError, oneshot, RwLock},
    task::JoinHandle,
};
use tracing::{debug, info, instrument, warn};

use crate::{
    contracts::SharedIdentityManager, database::Database, ethereum::write::TransactionId,
    identity_tree::TreeState, utils::spawn_or_abort,
};

mod tasks;

struct RunningInstance {
    process_identities_handle:       JoinHandle<()>,
    mine_identities_handle:          JoinHandle<()>,
    fetch_pending_identities_handle: JoinHandle<()>,
    wake_up_sender:                  mpsc::Sender<()>,
    shutdown_sender:                 broadcast::Sender<()>,
}

#[derive(Debug, Clone)]
pub struct PendingIdentities {
    identity_keys:  Vec<usize>,
    transaction_id: TransactionId,
    pre_root:       U256,
    post_root:      U256,
    start_index:    usize,
}

static PENDING_IDENTITIES: Lazy<Gauge> = Lazy::new(|| {
    register_gauge!("pending_identities", "Identities not submitted on-chain").unwrap()
});

impl RunningInstance {
    fn wake_up(&self) -> AnyhowResult<()> {
        // We're using a 1-element channel for wake-up notifications. It is safe to
        // ignore a full channel, because that means the committer is already scheduled
        // to wake up and will process all requests inserted in the database.
        match self.wake_up_sender.try_send(()) {
            Ok(_) => {
                debug!("Scheduled a committer job.");
                Ok(())
            }
            Err(TrySendError::Full(_)) => {
                debug!("Committer job already scheduled.");
                Ok(())
            }
            Err(TrySendError::Closed(_)) => {
                Err(anyhow!("Committer thread terminated unexpectedly."))
            }
        }
    }

    async fn shutdown(self) -> AnyhowResult<()> {
        info!("Sending a shutdown signal to the committer.");
        // Ignoring errors here, since we have two options: either the channel is full,
        // which is impossible, since this is the only use, and this method takes
        // ownership, or the channel is closed, which means the committer thread is
        // already dead.
        let _ = self.shutdown_sender.send(())?;

        info!("Awaiting committer shutdown.");
        self.process_identities_handle.await?;

        info!("Awaiting miner shutdown.");
        self.mine_identities_handle.await?;

        info!("Awaiting fetcher shutdown.");
        self.fetch_pending_identities_handle.await?;

        Ok(())
    }
}

/// Configuration options for the component responsible for committing
/// identities when queried.
#[derive(Clone, Debug, PartialEq, Eq, Parser)]
#[group(skip)]
pub struct Options {
    /// The maximum number of seconds the sequencer will wait before sending a
    /// batch of identities to the chain, even if the batch is not full.
    #[clap(long, env, default_value = "180")]
    pub batch_timeout_seconds: u64,
}

/// A worker that commits identities to the blockchain.
///
/// This uses the database to keep track of identities that need to be
/// committed. It assumes that there's only one such worker spawned at
/// a time. Spawning multiple worker threads will result in undefined behavior,
/// including data duplication.
pub struct IdentityCommitter {
    /// The instance is kept behind an RwLock<Option<...>> because
    /// when shutdown is called we want to be able to gracefully
    /// await the join handle - which requires ownership of the handle and by
    /// extension the instance.
    instance:                  RwLock<Option<RunningInstance>>,
    database:                  Arc<Database>,
    identity_manager:          SharedIdentityManager,
    tree_state:                TreeState,
    batch_insert_timeout_secs: u64,
}

impl IdentityCommitter {
    pub fn new(
        database: Arc<Database>,
        contracts: SharedIdentityManager,
        tree_state: TreeState,
        options: &Options,
    ) -> Self {
        let batch_insert_timeout_secs = options.batch_timeout_seconds;
        Self {
            instance: RwLock::new(None),
            database,
            identity_manager: contracts,
            tree_state,
            batch_insert_timeout_secs,
        }
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn start(&self) {
        let mut instance = self.instance.write().await;
        if instance.is_some() {
            warn!("Identity committer already running");
            return;
        }

        // We could use the second element of the tuple as `mut shutdown_receiver`,
        // but for symmetry's sake we create it for every task with `.subscribe()`
        let (shutdown_sender, _) = broadcast::channel(1);
        let (wake_up_sender, mut wake_up_receiver) = mpsc::channel(1);
        let (pending_identities_sender, pending_identities_receiver) = mpsc::channel(1);

        // This is used to prevent the committer from starting to process identities
        // before we've submitted all the pending transactions to the channel
        let (start_processing_sender, start_processing_receiver) = oneshot::channel();

        let fetch_pending_identities_handle = {
            let mut shutdown_receiver = shutdown_sender.subscribe();

            let identity_manager = self.identity_manager.clone();
            let batch_tree = self.tree_state.get_batching_tree();
            let pending_identities_sender = pending_identities_sender.clone();

            spawn_or_abort(async move {
                select! {
                    result = Self::fetch_and_enqueue_pending_identities(
                        start_processing_sender,
                        &batch_tree,
                        &identity_manager,
                        &pending_identities_sender,
                    ) => {
                        result?;
                    }
                    _ = shutdown_receiver.recv() => {
                        info!("Woke up by shutdown signal, exiting.");
                        return Ok(());
                    }
                }

                Ok(())
            })
        };

        let process_identities_handle = {
            let mut shutdown_receiver = shutdown_sender.subscribe();

            let database = self.database.clone();
            let identity_manager = self.identity_manager.clone();
            let batch_tree = self.tree_state.get_batching_tree();
            let timeout = self.batch_insert_timeout_secs;

            spawn_or_abort(async move {
                select! {
                    result = Self::process_identities(
                        start_processing_receiver,
                        &database,
                        &identity_manager,
                        &batch_tree,
                        &mut wake_up_receiver,
                        &pending_identities_sender,
                        timeout
                    ) => {
                        result?;
                    }
                    _ = shutdown_receiver.recv() => {
                        info!("Woke up by shutdown signal, exiting.");
                        return Ok(());
                    }
                }
                Ok(())
            })
        };

        let mine_identities_handle = {
            let mut shutdown_receiver = shutdown_sender.subscribe();

            let database = self.database.clone();
            let identity_manager = self.identity_manager.clone();
            let mined_tree = self.tree_state.get_mined_tree();

            spawn_or_abort(async move {
                select! {
                    result = Self::mine_identities(
                        &database,
                        &identity_manager,
                        &mined_tree,
                        pending_identities_receiver,
                    ) => {
                        result?;
                    }
                    _ = shutdown_receiver.recv() => {
                        info!("Woke up by shutdown signal, exiting.");
                        return Ok(());
                    }
                }
                Ok(())
            })
        };

        *instance = Some(RunningInstance {
            process_identities_handle,
            mine_identities_handle,
            fetch_pending_identities_handle,
            wake_up_sender,
            shutdown_sender,
        });
    }

    async fn log_pending_identities_count(database: &Database) -> AnyhowResult<()> {
        let pending_identities = database.count_pending_identities().await?;
        PENDING_IDENTITIES.set(f64::from(pending_identities));
        Ok(())
    }

    pub async fn notify_queued(&self) {
        // Escalate all errors to panics. In the future could perform some
        // restart procedure here.
        self.instance
            .read()
            .await
            .as_ref()
            .expect("Committer not running, terminating.")
            .wake_up()
            .unwrap();
    }

    /// # Errors
    ///
    /// Will return an Error if the committer thread cannot be shut down
    /// gracefully.
    pub async fn shutdown(&self) -> AnyhowResult<()> {
        let mut instance = self.instance.write().await;
        if let Some(instance) = instance.take() {
            instance.shutdown().await?;
        } else {
            info!("Committer not running.");
        }
        Ok(())
    }
}
