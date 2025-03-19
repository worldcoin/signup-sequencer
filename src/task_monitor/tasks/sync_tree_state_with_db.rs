use crate::identity_tree::db_sync::{sync_tree, SyncTreeResult};
use crate::identity_tree::ProcessedStatus;
use crate::retry_tx;
use crate::task_monitor::App;
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::watch::Sender;
use tokio::sync::Notify;
use tokio::time::Duration;
use tokio::{select, time};

pub async fn sync_tree_state_with_db(
    app: Arc<App>,
    sync_tree_notify: Arc<Notify>,
    tree_synced_tx: Sender<()>,
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

        let res = run_sync_tree(&app).await?;

        for tree_update in res.latest_tree_updates {
            let took = tree_update
                .received_at
                .clone()
                .map(|v| Utc::now().timestamp_millis() - v.timestamp_millis());
            tracing::info!(commitment = format!("{:x}", tree_update.element), status = ?ProcessedStatus::Pending, took = ?took, "Commitment added to latest tree.");
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
