use std::sync::Arc;

use anyhow::{anyhow, Result as AnyhowResult};
use clap::Parser;
use ethers::types::U256;
use once_cell::sync::Lazy;
use prometheus::{register_gauge, Gauge};
use tokio::{
    select,
    sync::{broadcast, mpsc, mpsc::error::TrySendError, Mutex, RwLock},
    task::JoinHandle,
};
use tracing::{debug, info, instrument, warn};

use crate::{
    contracts::SharedIdentityManager, database::Database, ethereum::write::TransactionId,
    identity_tree::TreeState, utils::spawn_with_exp_backoff,
};

mod tasks;

struct RunningInstance {
    process_identities_handle: JoinHandle<()>,
    mine_identities_handle:    JoinHandle<()>,
    wake_up_sender:            mpsc::Sender<()>,
    shutdown_sender:           broadcast::Sender<()>,
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
        let _ = self.shutdown_sender.send(());

        info!("Awaiting tasks to shutdown.");
        let (process_identities_result, mine_identities_result) =
            tokio::join!(self.process_identities_handle, self.mine_identities_handle);

        process_identities_result?;
        mine_identities_result?;

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
        let (wake_up_sender, wake_up_receiver) = mpsc::channel(1);
        let (pending_identities_sender, pending_identities_receiver) = mpsc::channel(1);

        // We need to maintain mutable access to these receivers from multiple
        // invocations of this task
        let wake_up_receiver = Arc::new(Mutex::new(wake_up_receiver));
        let pending_identities_receiver = Arc::new(Mutex::new(pending_identities_receiver));

        let process_identities_handle = {
            let pending_identities_sender = pending_identities_sender.clone();
            let database = self.database.clone();
            let identity_manager = self.identity_manager.clone();
            let batch_tree = self.tree_state.get_batching_tree();
            let timeout = self.batch_insert_timeout_secs;
            let shutdown_sender = shutdown_sender.clone();

            crate::utils::spawn_with_exp_backoff(move || {
                let wake_up_receiver = wake_up_receiver.clone();
                let pending_identities_sender = pending_identities_sender.clone();
                let batch_tree = batch_tree.clone();
                let database = database.clone();
                let identity_manager = identity_manager.clone();
                let shutdown_sender = shutdown_sender.clone();

                async move {
                    let mut wake_up_receiver = wake_up_receiver.lock().await;
                    let mut shutdown_receiver = shutdown_sender.subscribe();

                    select! {
                        result = Self::process_identities(
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
                            info!("Woke up by shutdown signal,exiting.");
                            return Ok(());
                        }
                    }

                    Ok(())
                }
            })
        };

        let mine_identities_handle = {
            let shutdown_sender = shutdown_sender.clone();

            let database = self.database.clone();
            let identity_manager = self.identity_manager.clone();
            let mined_tree = self.tree_state.get_mined_tree();

            spawn_with_exp_backoff(move || {
                let shutdown_sender = shutdown_sender.clone();
                let pending_identities_receiver = pending_identities_receiver.clone();

                let database = database.clone();
                let identity_manager = identity_manager.clone();
                let mined_tree = mined_tree.clone();

                async move {
                    let mut pending_identities_receiver = pending_identities_receiver.lock().await;
                    let mut shutdown_receiver = shutdown_sender.subscribe();

                    select! {
                        result = Self::mine_identities(
                            &database,
                            &identity_manager,
                            &mined_tree,
                            &mut pending_identities_receiver,
                        ) => {
                            result?;
                        }
                        _ = shutdown_receiver.recv() => {
                            info!("Woke up by shutdown signal, exiting.");
                            return Ok(());
                        }
                    }
                    Ok(())
                }
            })
        };

        *instance = Some(RunningInstance {
            process_identities_handle,
            mine_identities_handle,
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
