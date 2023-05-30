use std::sync::Arc;

use anyhow::Result as AnyhowResult;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, instrument, warn};

use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::database::Database;
use crate::identity_tree::{Canonical, TreeVersion, TreeWithNextVersion};
use crate::task_monitor::{PendingIdentities, TaskMonitor};

pub struct MineIdentities {
    database:                    Arc<Database>,
    identity_manager:            SharedIdentityManager,
    mined_tree:                  TreeVersion<Canonical>,
    pending_identities_receiver: Arc<Mutex<mpsc::Receiver<PendingIdentities>>>,
}

impl MineIdentities {
    pub fn new(
        database: Arc<Database>,
        identity_manager: SharedIdentityManager,
        mined_tree: TreeVersion<Canonical>,
        pending_identities_receiver: Arc<Mutex<mpsc::Receiver<PendingIdentities>>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            identity_manager,
            mined_tree,
            pending_identities_receiver,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        let mut pending_identities_receiver = self.pending_identities_receiver.lock().await;

        mine_identities_loop(
            &self.database,
            &self.identity_manager,
            &self.mined_tree,
            &mut pending_identities_receiver,
        )
        .await
    }
}

async fn mine_identities_loop(
    database: &Database,
    identity_manager: &IdentityManager,
    mined_tree: &TreeVersion<Canonical>,
    pending_identities_receiver: &mut mpsc::Receiver<PendingIdentities>,
) -> AnyhowResult<()> {
    loop {
        let Some(pending_identity) = pending_identities_receiver.recv().await else {
            warn!("Pending identities channel closed, terminating.");
            break;
        };

        mine_identities(pending_identity, database, identity_manager, mined_tree).await?;
    }

    Ok(())
}

#[instrument(level = "info", skip_all)]
async fn mine_identities(
    pending_identity: PendingIdentities,
    database: &Database,
    identity_manager: &IdentityManager,
    mined_tree: &TreeVersion<Canonical>,
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

    identity_manager.mine_identities(transaction_id).await?;

    // With this done, all that remains is to mark them as submitted to the
    // blockchain in the source-of-truth database, and also update the mined tree to
    // agree with the database and chain.
    database.mark_root_as_mined(&post_root.into()).await?;

    info!(start_index, ?pre_root, ?post_root, "Batch mined");

    let updates_count = mined_tree.apply_updates_up_to(post_root.into());

    info!(
        start_index,
        updates_count,
        ?pre_root,
        ?post_root,
        "Tree updated"
    );

    TaskMonitor::log_pending_identities_count(database).await?;

    Ok(())
}
