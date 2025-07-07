use crate::database::methods::DbMethods;
use crate::identity_tree::{
    Canonical, Intermediate, Latest, ProcessedStatus, ReversibleVersion, TreeState, TreeUpdate,
    TreeVersion, TreeVersionReadOps, TreeWithNextVersion,
};
use anyhow::bail;
use sqlx::{Postgres, Transaction};
use std::cmp::Ordering;
use tokio::sync::MutexGuard;
use tracing::debug;

pub struct SyncTreeResult {
    pub latest_tree_updates: Vec<TreeUpdate>,
}

/// Order of operations in sync tree is very important as it ensures we can
/// apply new updates or rewind them properly.
pub async fn sync_tree(
    tx: &mut Transaction<'_, Postgres>,
    tree_state: &MutexGuard<'_, TreeState>,
) -> anyhow::Result<SyncTreeResult> {
    let mined_tree = tree_state.mined_tree();
    let processed_tree = tree_state.processed_tree();
    let batching_tree = tree_state.batching_tree();
    let latest_tree = tree_state.latest_tree();

    let latest_mined_tree_update = tx
        .get_latest_tree_update_by_statuses(vec![ProcessedStatus::Mined])
        .await?;

    // First check if mined tree needs to be rolled back. If so then we must
    // panic to quit to rebuild the tree on startup. This is a time-consuming
    // operation.
    if let Some(ref mined_tree_update) = latest_mined_tree_update {
        assert!(
            mined_tree_update.sequence_id >= mined_tree.get_last_sequence_id(),
            "Mined tree needs to be rolled back."
        );
    };

    // Get all other roots from database
    let latest_processed_tree_update = tx
        .get_latest_tree_update_by_statuses(vec![
            ProcessedStatus::Processed,
            ProcessedStatus::Mined,
        ])
        .await?;

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
        latest_processed_tree_update.clone()
    };

    // And then update trees
    let latest_tree_updates =
        update_latest_tree(tx, latest_tree, &latest_pending_tree_update, || {
            update_batching_tree(batching_tree, &latest_batching_tree_update, || {
                update_processed_tree(processed_tree, &latest_processed_tree_update, || {
                    update_mined_tree(mined_tree, &latest_mined_tree_update)
                })
            })
        })
        .await?;

    Ok(SyncTreeResult {
        latest_tree_updates,
    })
}

async fn update_latest_tree<F: Fn() -> anyhow::Result<()>>(
    tx: &mut Transaction<'_, Postgres>,
    latest_tree: &TreeVersion<Latest>,
    latest_tree_update: &Option<TreeUpdate>,
    update_batching_tree: F,
) -> anyhow::Result<Vec<TreeUpdate>> {
    let Some(latest_tree_update) = latest_tree_update else {
        debug!("No latest tree update.");
        update_batching_tree()?;

        return Ok(vec![]);
    };

    let current_sequence_id = latest_tree.get_last_sequence_id();
    let new_sequence_id = latest_tree_update.sequence_id;

    let tree_updates = match new_sequence_id.cmp(&current_sequence_id) {
        Ordering::Greater => {
            debug!("Applying latest tree updates up to {}", new_sequence_id);
            let tree_updates = tx
                .get_tree_updates_after_id(latest_tree.get_last_sequence_id())
                .await?;
            latest_tree.apply_updates(&tree_updates);

            update_batching_tree()?;

            tree_updates
        }
        Ordering::Less => {
            debug!("Rewinding latest tree updates up to {}", new_sequence_id);
            update_batching_tree()?;

            latest_tree.rewind_updates_up_to(latest_tree_update.post_root);

            vec![]
        }
        Ordering::Equal => {
            debug!("Latest tree already up to date {}", new_sequence_id);

            update_batching_tree()?;

            vec![]
        }
    };

    Ok(tree_updates)
}

fn update_batching_tree<F: Fn() -> anyhow::Result<()>>(
    batching_tree: &TreeVersion<Intermediate>,
    batching_tree_update: &Option<TreeUpdate>,
    update_processed_tree: F,
) -> anyhow::Result<()> {
    let Some(batching_tree_update) = batching_tree_update else {
        debug!("No batching tree update.");
        update_processed_tree()?;

        return Ok(());
    };

    let current_sequence_id = batching_tree.get_last_sequence_id();
    let new_sequence_id = batching_tree_update.sequence_id;

    match new_sequence_id.cmp(&current_sequence_id) {
        Ordering::Greater => {
            debug!("Applying batching tree updates up to {}", new_sequence_id);
            batching_tree.apply_updates_up_to(batching_tree_update.post_root);

            update_processed_tree()?;
        }
        Ordering::Less => {
            debug!("Rewinding batching tree updates up to {}", new_sequence_id);
            update_processed_tree()?;

            batching_tree.rewind_updates_up_to(batching_tree_update.post_root);
        }
        Ordering::Equal => {
            debug!("Batching tree already up to date {}", new_sequence_id);

            update_processed_tree()?;
        }
    }

    Ok(())
}

fn update_processed_tree<F: Fn() -> anyhow::Result<()>>(
    processed_tree: &TreeVersion<Intermediate>,
    processed_tree_update: &Option<TreeUpdate>,
    update_mined_tree: F,
) -> anyhow::Result<()> {
    let Some(processed_tree_update) = processed_tree_update else {
        debug!("No processed tree update.");
        update_mined_tree()?;

        return Ok(());
    };

    let current_sequence_id = processed_tree.get_last_sequence_id();
    let new_sequence_id = processed_tree_update.sequence_id;

    match new_sequence_id.cmp(&current_sequence_id) {
        Ordering::Greater => {
            debug!("Applying processed tree updates up to {}", new_sequence_id);
            processed_tree.apply_updates_up_to(processed_tree_update.post_root);

            update_mined_tree()?;
        }
        Ordering::Less => {
            debug!("Rewinding processed tree updates up to {}", new_sequence_id);
            update_mined_tree()?;

            processed_tree.rewind_updates_up_to(processed_tree_update.post_root);
        }
        Ordering::Equal => {
            debug!("Processed tree already up to date {}", new_sequence_id);

            update_mined_tree()?;
        }
    }

    Ok(())
}

fn update_mined_tree(
    mined_tree: &TreeVersion<Canonical>,
    mined_tree_update: &Option<TreeUpdate>,
) -> anyhow::Result<()> {
    let Some(mined_tree_update) = mined_tree_update else {
        debug!("No mined tree update.");
        return Ok(());
    };

    let current_sequence_id = mined_tree.get_last_sequence_id();
    let new_sequence_id = mined_tree_update.sequence_id;

    match new_sequence_id.cmp(&current_sequence_id) {
        Ordering::Greater => {
            debug!("Applying mined tree updates up to {}", new_sequence_id);
            mined_tree.apply_updates_up_to(mined_tree_update.post_root);
        }
        Ordering::Less => {
            bail!("This should never happened. It is checked by assert done before calling.");
        }
        Ordering::Equal => {
            debug!("Mined tree already up to date {}", new_sequence_id);
        }
    }

    Ok(())
}
