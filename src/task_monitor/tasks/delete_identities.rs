use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result as AnyhowResult;
use tokio::sync::Notify;
use tokio::time::{Duration, Instant};
use tracing::info;

use crate::contracts::SharedIdentityManager;
use crate::database::types::DeletionEntry;
use crate::database::Database;
use crate::identity_tree::{Hash, Latest, TreeVersion};

pub struct DeleteIdentities {
    database:                Arc<Database>,
    latest_tree:             TreeVersion<Latest>,
    deletion_time_interval:  u64,
    min_deletion_batch_size: usize,
    wake_up_notify:          Arc<Notify>,
}

impl DeleteIdentities {
    pub fn new(
        database: Arc<Database>,
        latest_tree: TreeVersion<Latest>,
        deletion_time_interval: u64,
        min_deletion_batch_size: usize,
        wake_up_notify: Arc<Notify>,
    ) -> Arc<Self> {
        Arc::new(Self {
            database,
            latest_tree,
            deletion_time_interval,
            min_deletion_batch_size,
            wake_up_notify,
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        delete_identities(
            &self.database,
            &self.latest_tree,
            self.deletion_time_interval,
            self.min_deletion_batch_size,
            self.wake_up_notify.clone(),
        )
        .await
    }
}

// TODO: we might want to keep track of the last time we completed a deletion
// batch and make sure that there is a batch run at least once every n days to
// be compliant
async fn delete_identities(
    database: &Database,
    latest_tree: &TreeVersion<Latest>,
    deletion_time_interval: u64,
    min_deletion_batch_size: usize,
    wake_up_notify: Arc<Notify>,
) -> AnyhowResult<()> {
    info!("Starting deletion processor.");

    let deletion_time_interval = Duration::from_secs(deletion_time_interval);

    // TODO: we should track this with persistance to eunsure we are deleting at
    // least once every n days if there are deletions in the queue
    let mut last_deletion = Instant::now();

    loop {
        let deletions = database.get_deletions().await?;
        if deletions.is_empty() {
            // Sleep for one hour
            tokio::time::sleep(Duration::from_secs(3600)).await;
            continue;
        }

        // If the minimum deletions batch size is reached or the deletion time interval
        // has elapsed, run a batch of deletions
        if deletions.len() >= min_deletion_batch_size
            || Instant::now() - last_deletion > deletion_time_interval
        {
            // Dedup deletion entries
            let deletions = deletions
                .into_iter()
                .map(|f| f)
                .collect::<HashSet<DeletionEntry>>();

            let (leaf_indices, previous_commitments): (Vec<usize>, Vec<Hash>) = deletions
                .iter()
                .map(|d| (d.leaf_index, d.commitment))
                .unzip();

            // Delete the commitments at the target leaf indices in the latest tree,
            // generating the proof for each update
            let data = latest_tree.delete_many(&leaf_indices);

            assert_eq!(
                data.len(),
                leaf_indices.len(),
                "Length mismatch when appending identities to tree"
            );

            // Insert the new items into pending identities
            let items = data.into_iter().zip(leaf_indices.into_iter());
            for ((root, _proof), leaf_index) in items {
                database
                    .insert_pending_identity(leaf_index, &Hash::ZERO, &root)
                    .await?;
            }

            // Remove the previous commitments from the deletions table
            database.remove_deletions(previous_commitments).await?;
            wake_up_notify.notify_one();

            // Update the last deletion time
            last_deletion = Instant::now();
        }
    }
}
