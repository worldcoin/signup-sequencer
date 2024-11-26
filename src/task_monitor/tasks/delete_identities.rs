use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use chrono::Utc;
use tokio::sync::{Mutex, Notify};
use tokio::time::MissedTickBehavior;
use tokio::{select, time};
use tracing::info;

use crate::app::App;
use crate::database::methods::DbMethods;
use crate::database::types::DeletionEntry;
use crate::identity_tree::{Hash, TreeVersionReadOps};

// Deletion here differs from insert_identites task. This is because two
// different flows are created for both tasks. Due to how our prover works
// (can handle only a batch of same operations types - insertion or deletion)
// we want to group together insertions and deletions. We are doing it by
// grouping deletions (as the not need to be put into tree immediately as
// insertions) and putting them into the tree
pub async fn delete_identities(
    app: Arc<App>,
    pending_insertions_mutex: Arc<Mutex<()>>,
    wake_up_notify: Arc<Notify>,
) -> anyhow::Result<()> {
    info!("Starting deletion processor.");

    let batch_deletion_timeout = chrono::Duration::from_std(app.config.app.batch_deletion_timeout)
        .context("Invalid batch deletion timeout duration")?;

    let mut timer = time::interval(Duration::from_secs(5));
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        select! {
            _ = timer.tick() => {
                info!("Deletion processor woken due to timeout");
            }
        }

        let deletions = app.database.get_deletions().await?;
        if deletions.is_empty() {
            continue;
        }

        let last_deletion_timestamp = app.database.get_latest_deletion().await?.timestamp;

        // If the minimum deletions batch size is not reached and the deletion time
        // interval has not elapsed then we can skip
        if deletions.len() < app.config.app.min_batch_deletion_size
            && Utc::now() - last_deletion_timestamp <= batch_deletion_timeout
        {
            continue;
        }

        // Dedup deletion entries
        let deletions = deletions.into_iter().collect::<HashSet<DeletionEntry>>();
        let mut deletions = deletions.into_iter().collect::<Vec<DeletionEntry>>();

        // Check if the deletion batch could potentially create:
        // - duplicate root on the tree when inserting to identities
        // - duplicate root on batch
        // Such situation may happen only when deletions are done from the last inserted leaf in
        // decreasing order (each next leaf is decreased by 1) - same root for identities, or when
        // deletions are going to create same tree state - continuous deletions.
        // To avoid such situation we sort then in ascending order and only check the scenario when
        // they are continuous ending with last leaf index
        deletions.sort_by(|d1, d2| d1.leaf_index.cmp(&d2.leaf_index));

        if let Some(last_leaf_index) = app.tree_state()?.latest_tree().next_leaf().checked_sub(1) {
            let indices_are_continuous = deletions
                .windows(2)
                .all(|w| w[1].leaf_index == w[0].leaf_index + 1);

            if indices_are_continuous && deletions.last().unwrap().leaf_index == last_leaf_index {
                tracing::warn!(
                    "Deletion batch could potentially create a duplicate root batch. Deletion \
                 batch will be postponed"
                );
                continue;
            }
        }

        let (leaf_indices, previous_commitments): (Vec<usize>, Vec<Hash>) = deletions
            .iter()
            .map(|d| (d.leaf_index, d.commitment))
            .unzip();

        let _guard = pending_insertions_mutex.lock().await;

        let mut pre_root = app.tree_state()?.latest_tree().get_root();
        // Delete the commitments at the target leaf indices in the latest tree,
        // generating the proof for each update
        let data = app.tree_state()?.latest_tree().delete_many(&leaf_indices);

        assert_eq!(
            data.len(),
            leaf_indices.len(),
            "Length mismatch when appending identities to tree"
        );

        // Insert the new items into pending identities
        let items = data.into_iter().zip(leaf_indices);
        for ((root, _proof), leaf_index) in items {
            app.database
                .insert_pending_identity(leaf_index, &Hash::ZERO, &root, &pre_root)
                .await?;
            pre_root = root;
        }

        // Remove the previous commitments from the deletions table
        app.database.remove_deletions(&previous_commitments).await?;
        wake_up_notify.notify_one();
    }
}
