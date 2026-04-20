use crate::identity_tree::db_sync::{
    apply_sync_plan, build_sync_plan, sync_tree, SyncTreeResult, TreeStateSnapshot,
};
use crate::identity_tree::{ProcessedStatus, TreeUpdate, TreeVersionReadOps};
use crate::retry_tx;
use crate::task_monitor::App;
use chrono::Utc;
use semaphore_rs_poseidon::poseidon;
use std::sync::Arc;
use tokio::sync::watch::Sender;
use tokio::sync::Notify;
use tokio::time::MissedTickBehavior;
use tokio::{select, time};

pub async fn sync_tree_state_with_db(
    app: Arc<App>,
    sync_tree_notify: Arc<Notify>,
    tree_synced_tx: Sender<()>,
) -> anyhow::Result<()> {
    tracing::info!("Starting Sync TreeState with DB.");

    let mut timer = time::interval(app.config.app.tree_sync_interval);
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

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

        let res = run_sync_tree(&app).await?;

        for tree_update in res.latest_tree_updates {
            log_synced_commitment(tree_update);
        }

        tree_synced_tx.send(())?;
    }
}

/// Two-phase sync that keeps the `TreeState` mutex held only for brief,
/// CPU-bound operations, never across database I/O.
///
/// Phase 1 – Snapshot (lock held ~microseconds):
///   Acquire the `TreeState` mutex, read the current sequence IDs from all
///   four tree tiers, then immediately release the lock.
///
/// Phase 2 – DB reads (no lock held):
///   Issue all necessary Postgres queries inside a `retry_tx!` loop. The
///   results are packaged into a [`SyncPlan`] that contains no references to
///   the `MutexGuard`.
///
/// Phase 3 – Apply (lock held ~microseconds):
///   Acquire the `TreeState` mutex again. Verify the sequence IDs still match
///   the snapshot (another writer could have changed things between phases 1
///   and 3). If they match, apply the pre-computed plan to the in-memory
///   trees and release the lock. If they don't match, skip — the next timed
///   cycle will start fresh.
async fn run_sync_tree(app: &Arc<App>) -> anyhow::Result<SyncTreeResult> {
    // ── Phase 1: snapshot sequence IDs while briefly holding the lock ──────
    let snapshot = {
        let tree_state = app.tree_state().await?;
        TreeStateSnapshot::from_tree_state(&tree_state)
        // MutexGuard is dropped here
    };

    // ── Phase 2: DB reads — no lock held ──────────────────────────────────
    let plan = retry_tx!(&app.database, tx, build_sync_plan(&mut tx, &snapshot).await).await?;

    // ── Phase 3: apply plan — lock held briefly for in-memory writes only ──
    let tree_state = app.tree_state().await?;
    let res = apply_sync_plan(&tree_state, plan)?;

    tracing::info!("TreeState synced with DB");

    Ok(res)
}

fn log_synced_commitment(tree_update: TreeUpdate) {
    let took = tree_update
        .received_at
        .map(|v| Utc::now().timestamp_millis() - v.timestamp_millis());
    let hashed_commitment_str = format!("{:x}", poseidon::hash1(tree_update.element));
    if let Some(took) = took {
        tracing::info!(
            hashed_commitment = hashed_commitment_str,
            status = ?ProcessedStatus::Pending,
            took,
            "Commitment added to latest tree."
        );
    } else {
        tracing::info!(
            commitment = hashed_commitment_str,
            status = ?ProcessedStatus::Pending,
            "Commitment added to latest tree."
        );
    }
}
