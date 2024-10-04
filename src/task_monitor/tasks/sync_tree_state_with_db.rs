use std::sync::Arc;

use sqlx::{Postgres, Transaction};
use tokio::sync::Notify;
use tokio::time::Duration;
use tokio::{select, time};

use crate::database::query::DatabaseQuery;
use crate::identity_tree::{
    ProcessedStatus, ReversibleVersion, TreeState, TreeVersionReadOps, TreeWithNextVersion,
};
use crate::retry_tx;
use crate::task_monitor::App;

pub async fn sync_tree_state_with_db(
    app: Arc<App>,
    sync_tree_notify: Arc<Notify>,
    tree_synced_notify: Arc<Notify>,
) -> anyhow::Result<()> {
    tracing::info!("Awaiting for a clean slate");
    app.identity_processor.await_clean_slate().await?;

    tracing::info!("Awaiting for initialized tree");
    app.tree_state()?;

    let mut timer = time::interval(Duration::from_secs(5));

    loop {
        // We wait either for a timer tick or a full batch
        select! {
            _ = timer.tick() => {
                tracing::info!("Sync TreeState with DB task woken due to timeout");
            }

            () = sync_tree_notify.notified() => {
                tracing::info!("Sync TreeState with DB task woken due to sync request");
            },
        }

        let tree_state = app.tree_state()?;

        retry_tx!(&app.database, tx, sync_tree(&mut tx, tree_state).await).await?;

        tree_synced_notify.notify_one();
    }
}

/// Order of operations in sync tree is very important as it ensures we can
/// apply new updates or rewind them properly.
async fn sync_tree(
    tx: &mut Transaction<'_, Postgres>,
    tree_state: &TreeState,
) -> anyhow::Result<()> {
    let latest_processed_tree_update = tx
        .get_latest_tree_update_by_statuses(vec![
            ProcessedStatus::Processed,
            ProcessedStatus::Mined,
        ])
        .await?;

    let processed_tree = tree_state.processed_tree();
    let batching_tree = tree_state.batching_tree();
    let latest_tree = tree_state.latest_tree();

    // First check if processed tree needs to be rolled back. If so then we must
    // panic to quit to rebuild the tree on startup. This is a time-consuming
    // operation.
    if let Some(ref tree_update) = latest_processed_tree_update {
        let last_sequence_id = processed_tree.get_last_sequence_id();
        assert!(
            tree_update.sequence_id >= last_sequence_id,
            "Processed tree needs to be rolled back."
        );
    };

    let latest_pending_tree_update = tx
        .get_latest_tree_update_by_statuses(vec![
            ProcessedStatus::Pending,
            ProcessedStatus::Processed,
            ProcessedStatus::Mined,
        ])
        .await?;

    let latest_batch = tx.get_latest_batch().await?;
    let latest_batching_tree_update = if let Some(latest_batch) = latest_batch {
        tx.get_tree_update_by_root(&latest_batch.next_root).await?
    } else {
        None
    };

    // Then check if latest tree can be updated forward.
    if let Some(latest_tree_update) = latest_pending_tree_update {
        if latest_tree_update.sequence_id >= latest_tree.get_last_sequence_id() {
            let tree_updates = tx
                .get_commitments_after_id(latest_tree.get_last_sequence_id())
                .await?;
            latest_tree.apply_updates(&tree_updates);

            if let Some(batching_tree_update) = latest_batching_tree_update {
                if batching_tree_update.sequence_id > batching_tree.get_last_sequence_id() {
                    batching_tree.apply_updates_up_to(batching_tree_update.post_root);
                } else if batching_tree_update.sequence_id < batching_tree.get_last_sequence_id() {
                    batching_tree.rewind_updates_up_to(batching_tree_update.post_root);
                }
            }
        } else {
            if let Some(batching_tree_update) = latest_batching_tree_update {
                if batching_tree_update.sequence_id > batching_tree.get_last_sequence_id() {
                    batching_tree.apply_updates_up_to(batching_tree_update.post_root);
                } else if batching_tree_update.sequence_id < batching_tree.get_last_sequence_id() {
                    batching_tree.rewind_updates_up_to(batching_tree_update.post_root);
                }
            }
            latest_tree.rewind_updates_up_to(latest_tree_update.post_root);
        }
    }

    if let Some(ref processed_tree_update) = latest_processed_tree_update {
        if processed_tree_update.sequence_id > processed_tree.get_last_sequence_id() {
            processed_tree.apply_updates_up_to(processed_tree_update.post_root);
        }
    }

    Ok(())
}
