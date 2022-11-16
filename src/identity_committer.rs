use crate::{
    app::{Hash, TreeState},
    contracts::Contracts,
    database::Database,
};
use eyre::eyre;
use std::sync::{atomic::Ordering, Arc};
use tokio::{
    sync::{mpsc, mpsc::error::TrySendError, RwLock},
    task::JoinHandle,
};
use tracing::{debug, error, info, instrument};

/// A type that cannot be constructed. Should be replaced with `!` when it gets
/// stabilized.
pub enum Never {}

struct RunningInstance {
    #[allow(dead_code)]
    handle:         JoinHandle<eyre::Result<Never>>,
    wake_up_sender: mpsc::Sender<()>,
}

impl RunningInstance {
    fn wake_up(&self) -> eyre::Result<()> {
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
            Err(TrySendError::Closed(_)) => Err(eyre!("Committer thread terminated unexpectedly.")),
        }
    }
}

pub struct IdentityCommitter {
    instance:   RwLock<Option<RunningInstance>>,
    database:   Arc<Database>,
    contracts:  Arc<Contracts>,
    tree_state: Arc<TreeState>,
}

impl IdentityCommitter {
    pub fn new(
        database: Arc<Database>,
        contracts: Arc<Contracts>,
        tree_state: Arc<TreeState>,
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
            info!("Identity committer already running");
            return;
        }
        let (wake_up_sender, mut wake_up_receiver) = mpsc::channel(1);
        let database = self.database.clone();
        let tree_state = self.tree_state.clone();
        let contracts = self.contracts.clone();
        let handle = tokio::spawn(async move {
            loop {
                while let Some((group_id, commitment)) =
                    database.get_oldest_unprocessed_identity().await?
                {
                    Self::commit_identity(&database, &contracts, &tree_state, group_id, commitment)
                        .await?;
                }

                wake_up_receiver.recv().await;
            }
        });
        *instance = Some(RunningInstance {
            handle,
            wake_up_sender,
        });
    }

    #[instrument(level = "info", skip_all)]
    async fn commit_identity(
        database: &Database,
        contracts: &Contracts,
        tree_state: &TreeState,
        group_id: usize,
        commitment: Hash,
    ) -> eyre::Result<()> {
        // Get a progress lock on the tree for the duration of this operation. Holding a
        // progress lock ensures no other thread calls tries to insert an identity into
        // the contract.
        let tree = tree_state.merkle_tree.progress().await.map_err(|e| {
            error!(?e, "Failed to obtain tree lock in commit_identity.");
            e
        })?;

        // Fetch next leaf index
        let identity_index = tree_state.next_leaf.fetch_add(1, Ordering::AcqRel);

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
        tree.set(identity_index, commitment);

        // Downgrade write lock to progress lock
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
            .assert_valid_root(tree.root())
            .await
            .map_err(|error| {
                error!(
                    computed_root = ?tree.root(),
                    ?error,
                    "Root mismatch between tree and contract."
                );
                error
            })?;

        // Immediately write the tree to storage, before anyone else can
        // TODO: Store tree in database

        Ok(())
    }

    pub async fn notify_queued(&self) {
        // Escalate all errors to panics. In the future could perform some cleanup /
        // restart procedure here.
        self.instance
            .read()
            .await
            .as_ref()
            .expect("Committer not running, terminating.")
            .wake_up()
            .unwrap();
    }
}
