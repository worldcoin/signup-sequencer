use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result as AnyhowResult};
use chrono::{DateTime, Utc};

use tracing::{info, instrument};

use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::database::Database;
use crate::identity_tree::{Hash, Intermediate, TreeVersion, TreeWithNextVersion};
use crate::task_monitor::{
    PendingBatchDeletion, PendingBatchInsertion, PendingBatchSubmission, TaskMonitor,
};
use crate::utils::async_queue::AsyncQueue;

pub struct MineIdentities {
    database: Arc<Database>,
    identity_manager: SharedIdentityManager,
    mined_tree: TreeVersion<Intermediate>,
    pending_batch_submissions_queue: AsyncQueue<PendingBatchSubmission>,
}

impl MineIdentities {
    pub fn new(
        database: Arc<Database>,
        identity_manager: SharedIdentityManager,
        mined_tree: TreeVersion<Intermediate>,
        pending_batch_submissions_queue: AsyncQueue<PendingBatchSubmission>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            identity_manager,
            mined_tree,
            pending_batch_submissions_queue,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        mine_identities_loop(
            &self.database,
            &self.identity_manager,
            &self.mined_tree,
            &self.pending_batch_submissions_queue,
        )
        .await
    }
}

async fn mine_identities_loop(
    database: &Database,
    identity_manager: &IdentityManager,
    mined_tree: &TreeVersion<Intermediate>,
    pending_batch_submissions_queue: &AsyncQueue<PendingBatchSubmission>,
) -> AnyhowResult<()> {
    loop {
        let pending_identity = pending_batch_submissions_queue.pop().await;

        match pending_identity.read().await {
            PendingBatchSubmission::Insertion(pending_identity_insertion) => {
                mine_insertions(
                    pending_identity_insertion,
                    database,
                    identity_manager,
                    mined_tree,
                )
                .await?;
            }
            PendingBatchSubmission::Deletion(pending_identity_deletion) => {
                mine_deletions(
                    pending_identity_deletion,
                    database,
                    identity_manager,
                    mined_tree,
                )
                .await?;
            }
        }

        pending_identity.commit().await;
    }
}

#[instrument(level = "info", skip_all)]
async fn mine_insertions(
    pending_identity: PendingBatchInsertion,
    database: &Database,
    identity_manager: &IdentityManager,
    mined_tree: &TreeVersion<Intermediate>,
) -> AnyhowResult<()> {
    let PendingBatchInsertion {
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

#[instrument(level = "info", skip_all)]
async fn mine_deletions(
    pending_identity_deletion: PendingBatchDeletion,
    database: &Database,
    identity_manager: &IdentityManager,
    mined_tree: &TreeVersion<Intermediate>,
) -> AnyhowResult<()> {
    let PendingBatchDeletion {
        transaction_id,
        pre_root,
        post_root,
        commitments,
    } = pending_identity_deletion;

    info!(
        ?pre_root,
        ?post_root,
        ?transaction_id,
        "Mining deletion batch"
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

    // Update the latest deletion
    database.update_latest_deletion(Utc::now()).await?;

    info!(?pre_root, ?post_root, "Deletion batch mined");

    let updates_count = mined_tree.apply_updates_up_to(post_root.into());

    info!(updates_count, ?pre_root, ?post_root, "Mined tree updated");

    // Check if any deleted commitments correspond with entries in the
    // recoveries table and insert the new commitment into the unprocessed
    // identities table with the proper eligibility timestamp
    let recoveries = database
        .get_recoveries()
        .await?
        .iter()
        .map(|f| (f.existing_commitment, f.new_commitment))
        .collect::<HashMap<Hash, Hash>>();

    // Fetch the root history expiry time on chain
    let root_history_expiry = identity_manager.root_history_expiry().await?;

    // Use the root history expiry to calcuate the eligibility timestamp for the new
    // insertion
    let eligibility_timestamp = DateTime::from_utc(
        chrono::NaiveDateTime::from_timestamp_opt(
            Utc::now().timestamp() + root_history_expiry.as_u64() as i64,
            0,
        )
        .context("Could not convert eligibility timestamp to NaiveDateTime")?,
        Utc,
    );

    // For each deletion, if there is a corresponding recovery, insert a new
    // identity with the specified eligibility timestamp
    for prev_commitment in commitments {
        if let Some(new_commitment) = recoveries.get(&prev_commitment.into()) {
            database
                .insert_new_identity(*new_commitment, eligibility_timestamp)
                .await?;
        }
    }

    TaskMonitor::log_identities_queues(database).await?;

    Ok(())
}
