use std::sync::Arc;

use anyhow::{anyhow, Result as AnyhowResult};
use tokio::{
    select,
    sync::{
        mpsc::{self, error::TrySendError, Receiver},
        RwLock,
    },
    task::JoinHandle,
};
use tracing::{debug, error, info, instrument, warn};

use crate::{
    contracts::{IdentityManager, SharedIdentityManager},
    database::Database,
    identity_tree::{TreeState, TreeUpdate, TreeVersion},
    utils::spawn_or_abort,
};

struct RunningInstance {
    #[allow(dead_code)]
    handle:          JoinHandle<()>,
    wake_up_sender:  mpsc::Sender<()>,
    shutdown_sender: mpsc::Sender<()>,
}

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
        let _ = self.shutdown_sender.send(()).await;
        info!("Awaiting committer shutdown.");
        self.handle.await?;
        Ok(())
    }
}

/// A worker that commits identities to the blockchain.
///
/// This uses the database to keep track of identities that need to be
/// committed. It assumes that there's only one such worker spawned at
/// a time. Spawning multiple worker threads will result in undefined behavior,
/// including data duplication.
pub struct IdentityCommitter {
    instance:         RwLock<Option<RunningInstance>>,
    database:         Arc<Database>,
    identity_manager: SharedIdentityManager,
    tree_state:       TreeState,
}

impl IdentityCommitter {
    pub fn new(
        database: Arc<Database>,
        contracts: SharedIdentityManager,
        tree_state: TreeState,
    ) -> Self {
        Self {
            instance: RwLock::new(None),
            database,
            identity_manager: contracts,
            tree_state,
        }
    }

    #[instrument(level = "debug", skip_all)]
    pub async fn start(&self) {
        let mut instance = self.instance.write().await;
        if instance.is_some() {
            warn!("Identity committer already running");
            return;
        }
        let (shutdown_sender, mut shutdown_receiver) = mpsc::channel(1);
        let (wake_up_sender, mut wake_up_receiver) = mpsc::channel(1);
        let database = self.database.clone();
        let identity_manager = self.identity_manager.clone();
        let tree = self.tree_state.get_mined_tree();
        let handle = spawn_or_abort(async move {
            select! {
                result = Self::process_identities(&database, &*identity_manager, &tree, &mut wake_up_receiver) => {
                    result?;
                }
                _ = shutdown_receiver.recv() => {
                    info!("Woke up by shutdown signal, exiting.");
                    return Ok(());
                }
            }
            Ok(())
        });
        *instance = Some(RunningInstance {
            handle,
            wake_up_sender,
            shutdown_sender,
        });
    }

    async fn process_identities(
        database: &Database,
        identity_manager: &(dyn IdentityManager + Send + Sync),
        tree: &TreeVersion,
        wake_up_receiver: &mut Receiver<()>,
    ) -> AnyhowResult<()> {
        loop {
            while let Some(update) = tree.peek_next_update().await {
                Self::commit_identity(database, identity_manager, tree, &update).await?;
            }

            wake_up_receiver.recv().await;
            debug!("Woke up by a request.");
        }
    }

    #[instrument(level = "info", skip_all)]
    async fn commit_identity(
        database: &Database,
        identity_manager: &(dyn IdentityManager + Send + Sync),
        tree: &TreeVersion,
        update: &TreeUpdate,
    ) -> AnyhowResult<()> {
        // Send Semaphore transaction
        identity_manager
            .register_identities(vec![update.element])
            .await
            .map_err(|e| {
                error!(?e, "Failed to insert identity to contract.");
                e
            })?;

        database
            .mark_identity_submitted_to_contract(update.leaf_index)
            .await?;

        tree.apply_next_update().await;

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
