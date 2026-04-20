use crate::identity_tree::db_sync::{build_sync_plan, SyncTreeResult, TreeStateSnapshot};
use crate::identity_tree::{ProcessedStatus, TreeUpdate};
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

/// Three-phase sync: snapshots tree sequence IDs, fetches a [`SyncPlan`] from
/// the database via [`build_sync_plan`], then applies it via [`SyncPlan::apply`].
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
    let res = plan.apply(&tree_state)?;

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
