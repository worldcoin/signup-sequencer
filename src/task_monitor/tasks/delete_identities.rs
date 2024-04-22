use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Context;
use chrono::Utc;
use tokio::sync::{Mutex, Notify};
use tracing::info;

use crate::app::App;
use crate::database::types::{Commitments, DeletionEntry};
use crate::database::DatabaseExt;
use crate::identity_tree::Hash;

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
                    .insert_pending_identity(leaf_index, &Hash::ZERO, &root)
                    .await?;
            }

            // Remove the previous commitments from the deletions table
            app.database
                .remove_deletions(&Commitments(previous_commitments))
                .await?;
            wake_up_notify.notify_one();
        } else {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    }
}
