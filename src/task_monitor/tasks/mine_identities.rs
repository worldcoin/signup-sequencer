use std::sync::Arc;

use anyhow::Result as AnyhowResult;
use ethers::types::U256;
use tracing::{info, instrument};

use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::database::Database;
use crate::identity_tree::{Intermediate, TreeVersion, TreeWithNextVersion};
use crate::task_monitor::{PendingBatchSubmission, TaskMonitor};
use crate::utils::async_queue::{AsyncPopGuard, AsyncQueue};

pub struct MineIdentities {
    database: Arc<Database>,
    identity_manager: SharedIdentityManager,
    mined_tree: TreeVersion<Intermediate>,
    pending_batch_submissions_queue: AsyncQueue<PendingBatchSubmission>,
    mined_roots_queue: AsyncQueue<U256>,
}

impl MineIdentities {
    pub fn new(
        database: Arc<Database>,
        identity_manager: SharedIdentityManager,
        mined_tree: TreeVersion<Intermediate>,
        pending_batch_submissions_queue: AsyncQueue<PendingBatchSubmission>,
        mined_roots_queue: AsyncQueue<U256>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            identity_manager,
            mined_tree,
            pending_batch_submissions_queue,
            mined_roots_queue,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        mine_identities_loop(
            &self.database,
            &self.identity_manager,
            &self.mined_tree,
            &self.pending_batch_submissions_queue,
            &self.mined_roots_queue,
        )
        .await
    }
}

async fn mine_identities_loop(
    database: &Database,
    identity_manager: &IdentityManager,
    mined_tree: &TreeVersion<Intermediate>,
    pending_batch_submissions_queue: &AsyncQueue<PendingBatchSubmission>,
    mined_roots_queue: &AsyncQueue<U256>,
) -> AnyhowResult<()> {
    loop {
        let pending_identity = pending_batch_submissions_queue.pop().await;

        mine_identities(
            &pending_identity,
            database,
            identity_manager,
            mined_tree,
            mined_roots_queue,
        )
        .await?;

        pending_identity.commit().await;
    }
}

#[instrument(level = "info", skip_all)]
async fn mine_identities(
    pending_identity: &AsyncPopGuard<'_, PendingBatchSubmission>,
    database: &Database,
    identity_manager: &IdentityManager,
    mined_tree: &TreeVersion<Intermediate>,
    mined_roots_queue: &AsyncQueue<U256>,
) -> AnyhowResult<()> {
    let PendingBatchSubmission {
        transaction_id,
        pre_root,
        post_root,
        start_index,
    } = pending_identity.read().await;

    info!(
        start_index,
        ?pre_root,
        ?post_root,
        ?transaction_id,
        "Mining batch"
    );

    if !identity_manager
        .mine_identities(transaction_id.clone())
        .await?
    {
        panic!(
            "Transaction {} failed on chain - sequencer will crash and restart",
            transaction_id
        );
    }

    // With this done, all that remains is to mark them as submitted to the
    // blockchain in the source-of-truth database, and also update the mined tree to
    // agree with the database and chain.
    database.mark_root_as_processed(&post_root.into()).await?;

    info!(start_index, ?pre_root, ?post_root, "Batch mined");

    let updates_count = mined_tree.apply_updates_up_to(post_root.into());

    mined_roots_queue.push(post_root).await;

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
