use crate::{
    app::{Hash, SharedTreeState, TreeState},
    contracts::Contracts,
    database::Database,
    timed_read_progress_lock::TimedReadProgressLock,
    utils::spawn_or_abort,
};
use anyhow::{anyhow, Result as AnyhowResult};
use std::sync::Arc;
use tokio::{
    select,
    sync::{mpsc, mpsc::error::TrySendError, RwLock},
    task::JoinHandle,
};
use tracing::{debug, error, info, instrument, warn};

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
            Err(TrySendError::Closed(_)) => Err(anyhow!("Committer thread terminated unexpectedly.")),
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

/// A worker that commits identities to the tree.
///
/// This uses the database to keep track of identities that need to be
/// committed. It assumes that there's only one such worker spawned at
/// a time. Spawning multiple worker threads will result in undefined behavior,
/// including data duplication.
pub struct IdentityCommitter {
    instance:   RwLock<Option<RunningInstance>>,
    database:   Arc<Database>,
    contracts:  Arc<Contracts>,
    tree_state: SharedTreeState,
}

impl IdentityCommitter {
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
            warn!("Identity committer already running");
            return;
        }
        let (shutdown_sender, mut shutdown_receiver) = mpsc::channel(1);
        let (wake_up_sender, mut wake_up_receiver) = mpsc::channel(1);
        let database = self.database.clone();
        let tree_state = self.tree_state.clone();
        let contracts = self.contracts.clone();
        let handle = spawn_or_abort(async move {
            loop {
                while let Some((group_id, commitment)) =
                    database.get_oldest_unprocessed_identity().await?
                {
                    if (shutdown_receiver.try_recv()).is_ok() {
                        info!("Shutdown signal received, not processing remaining items.");
                        return Ok(());
                    }
                    Self::commit_identity(&database, &contracts, &tree_state, group_id, commitment)
                        .await?;
                }

                select! {
                    _ = wake_up_receiver.recv() => {
                        debug!("Woke up by a request.");
                    }
                    _ = shutdown_receiver.recv() => {
                        info!("Woke up by shutdown signal, exiting.");
                        return Ok(());
                    }
                }
            }
        });
        *instance = Some(RunningInstance {
            handle,
            wake_up_sender,
            shutdown_sender,
        });
    }

    #[instrument(level = "info", skip_all)]
    async fn commit_identity(
        database: &Database,
        contracts: &Contracts,
        tree_state: &TimedReadProgressLock<TreeState>,
        group_id: usize,
        commitment: Hash,
    ) -> AnyhowResult<()> {
        // Get a progress lock on the tree for the duration of this operation. Holding a
        // progress lock ensures no other thread calls tries to insert an identity into
        // the contract, as that is an order dependent operation.
        let tree = tree_state.progress().await.map_err(|e| {
            error!(?e, "Failed to obtain tree lock in commit_identity.");
            e
        })?;

        // Send Semaphore transaction
        let receipt = contracts.insert_identity(commitment).await.map_err(|e| {
            error!(?e, "Failed to insert identity to contract.");
            e
        })?;

        let mut tree = tree.upgrade_to_write().await.map_err(|e| {
            error!(?e, "Failed to obtain tree lock in insert_identity.");
            e
        })?;

        // Update  merkle tree
        let identity_index = tree.next_leaf;
        tree.merkle_tree.set(identity_index, commitment);
        tree.next_leaf += 1;

        // Downgrade write lock to progress lock – tree is now up to date, but still
        // need to update the database.
        let tree = tree.downgrade_to_progress();

        let block = receipt
            .block_number
            .expect("Transaction is mined, block number must be present.");
        info!(
            "Identity inserted in block {} at index {}.",
            block, identity_index
        );
        database
            .mark_identity_inserted(group_id, &commitment, block.as_usize(), identity_index)
            .await?;

        // Check tree root
        contracts
            .assert_valid_root(tree.merkle_tree.root())
            .await
            .map_err(|error| {
                error!(
                    computed_root = ?tree.merkle_tree.root(),
                    ?error,
                    "Root mismatch between tree and contract."
                );
                error
            })?;

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
