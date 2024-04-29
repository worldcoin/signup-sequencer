use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Context;
use chrono::Utc;
use tokio::sync::{Mutex, Notify};
use tracing::info;

use crate::app::App;
use crate::database::types::{BatchType, Commitments, DeletionEntry, LeafIndexes};
use crate::database::DatabaseExt;
use crate::identity_tree::{Hash, TreeVersionReadOps};

// todo(piotrh): ensure deletes runs from time to time
// todo(piotrh): ensure things are batched properly to save $$$ when executed
// on, add check timeour chain
pub async fn delete_identities(
    app: Arc<App>,
    pending_insertions_mutex: Arc<Mutex<()>>,
    wake_up_notify: Arc<Notify>,
) -> anyhow::Result<()> {
    info!("Starting deletion processor.");

    let batch_deletion_timeout = chrono::Duration::from_std(app.config.app.batch_deletion_timeout)
        .context("Invalid batch deletion timeout duration")?;

    loop {
        let deletions = app.database.get_deletions().await?;
        if deletions.is_empty() {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            continue;
        }

        let last_deletion_timestamp = app.database.get_latest_deletion().await?.timestamp;

        // If the minimum deletions batch size is reached or the deletion time interval
        // has elapsed, run a batch of deletions
        if deletions.len() >= app.config.app.min_batch_deletion_size
            || Utc::now() - last_deletion_timestamp > batch_deletion_timeout
        {
            // Dedup deletion entries
            let deletions = deletions.into_iter().collect::<HashSet<DeletionEntry>>();

            let (leaf_indices, previous_commitments): (Vec<usize>, Vec<Hash>) = deletions
                .iter()
                .map(|d| (d.leaf_index, d.commitment))
                .unzip();

            let _guard = pending_insertions_mutex.lock().await;

            // Delete the commitments at the target leaf indices in the latest tree,
            // generating the proof for each update
            let data = app
                .tree_state()?
                .latest_tree()
                .delete_many_as_derived(&leaf_indices);
            let prev_root = app.tree_state()?.latest_tree().get_root();
            let next_root = data
                .last()
                .map(|(root, ..)| root.clone())
                .expect("should be created at least one");

            assert_eq!(
                data.len(),
                leaf_indices.len(),
                "Length mismatch when appending identities to tree"
            );

            let mut tx = app.database.pool.begin().await?;

            // Insert the new items into pending identities
            let items: Vec<_> = data.into_iter().zip(leaf_indices).collect();
            for ((root, _proof), leaf_index) in items.iter() {
                tx.insert_pending_identity(*leaf_index, &Hash::ZERO, &root)
                    .await?;
            }

            // Remove the previous commitments from the deletions table
            tx.remove_deletions(&Commitments(previous_commitments.clone()))
                .await?;

            tx.insert_new_batch(
                &next_root,
                &prev_root,
                BatchType::Deletion,
                &Commitments(previous_commitments),
                &LeafIndexes(items.iter().map(|((..), leaf_index)| *leaf_index).collect()),
            )
            .await?;

            tx.commit().await?;

            wake_up_notify.notify_one();
        } else {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    }
}
