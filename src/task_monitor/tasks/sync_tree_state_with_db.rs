use crate::identity_tree::db_sync::{sync_tree, SyncTreeResult};
use crate::identity_tree::{ProcessedStatus, TreeUpdate};
use crate::retry_tx;
use crate::task_monitor::App;
use chrono::Utc;
use semaphore_rs_poseidon::poseidon;
use std::sync::Arc;
use tokio::sync::watch::Sender;
use tokio::sync::Notify;
use tokio::time::{Duration, MissedTickBehavior};
use tokio::{select, time};

pub async fn sync_tree_state_with_db(
    app: Arc<App>,
    sync_tree_notify: Arc<Notify>,
    tree_synced_tx: Sender<()>,
) -> anyhow::Result<()> {
    tracing::info!("Starting Sync TreeState with DB.");

    let mut timer = time::interval(Duration::from_secs(5));
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

async fn run_sync_tree(app: &Arc<App>) -> anyhow::Result<SyncTreeResult> {
    let tree_state = app.tree_state()?;

    let res = retry_tx!(&app.database, tx, sync_tree(&mut tx, tree_state).await).await?;

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
