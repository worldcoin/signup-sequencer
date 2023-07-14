use std::sync::Arc;

use anyhow::Result as AnyhowResult;
use ethers::types::U256;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, instrument, warn};

use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::database::Database;
use crate::identity_tree::{Intermediate, TreeVersion, TreeWithNextVersion};
use crate::task_monitor::{PendingIdentities, TaskMonitor};

pub struct MineIdentities {
    database:                    Arc<Database>,
    identity_manager:            SharedIdentityManager,
    mined_tree:                  TreeVersion<Intermediate>,
    pending_identities_receiver: Arc<Mutex<mpsc::Receiver<PendingIdentities>>>,
    mined_roots_sender:          mpsc::Sender<U256>,
}

impl MineIdentities {
    pub fn new(
        database: Arc<Database>,
        identity_manager: SharedIdentityManager,
        mined_tree: TreeVersion<Intermediate>,
        pending_identities_receiver: Arc<Mutex<mpsc::Receiver<PendingIdentities>>>,
        mined_roots_sender: mpsc::Sender<U256>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            identity_manager,
            mined_tree,
            pending_identities_receiver,
            mined_roots_sender,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        let mut pending_identities_receiver = self.pending_identities_receiver.lock().await;

        mine_identities_loop(
            &self.database,
            &self.identity_manager,
            &self.mined_tree,
            &mut pending_identities_receiver,
            &self.mined_roots_sender,
        )
        .await
    }
}

async fn mine_identities_loop(
    database: &Database,
    identity_manager: &IdentityManager,
    mined_tree: &TreeVersion<Intermediate>,
    pending_identities_receiver: &mut mpsc::Receiver<PendingIdentities>,
    mined_roots_sender: &mpsc::Sender<U256>,
) -> AnyhowResult<()> {
    loop {
        let Some(pending_identity) = pending_identities_receiver.recv().await else {
            warn!("Pending identities channel closed, terminating.");
            break;
        };

        mine_identities(
            pending_identity,
            database,
            identity_manager,
            mined_tree,
            mined_roots_sender,
        )
        .await?;
    }

    Ok(())
}

#[instrument(level = "info", skip_all)]
async fn mine_identities(
    pending_identity: PendingIdentities,
    database: &Database,
    identity_manager: &IdentityManager,
    mined_tree: &TreeVersion<Intermediate>,
    mined_roots_sender: &mpsc::Sender<U256>,
) -> AnyhowResult<()> {
    let PendingIdentities {
        transaction_id,
        pre_root,
        post_root,
        start_index,
    } = pending_identity;

    info!(
        start_index,
        ?pre_root,
        ?post_root,
        ?transaction_id,
        "Mining batch"
    );

    let result = identity_manager.mine_identities(transaction_id).await;

    if let Err(err) = result {
        panic!(
            "Failed to insert identity to contract due to error {err}. Restarting sequencer to \
             reconstruct local tree"
        );
    };

    // With this done, all that remains is to mark them as submitted to the
    // blockchain in the source-of-truth database, and also update the mined tree to
    // agree with the database and chain.
    database.mark_root_as_processed(&post_root.into()).await?;

    info!(start_index, ?pre_root, ?post_root, "Batch mined");

    let updates_count = mined_tree.apply_updates_up_to(post_root.into());

    mined_roots_sender.send(post_root).await?;

    info!(
        start_index,
        updates_count,
        ?pre_root,
        ?post_root,
        "Mined tree updated"
    );

    TaskMonitor::log_identities_queues(database).await?;

    Ok(())
}
