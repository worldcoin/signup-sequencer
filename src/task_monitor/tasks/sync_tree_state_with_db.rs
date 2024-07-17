use crate::identity_tree::db_sync::sync_tree;
use crate::retry_tx;
use crate::task_monitor::App;
use std::sync::Arc;
use tokio::sync::watch::Sender;
use tokio::sync::Notify;
use tokio::time::Duration;
use tokio::{select, time};

pub async fn sync_tree_state_with_db(
    app: Arc<App>,
    sync_tree_notify: Arc<Notify>,
    tree_synced_tx: Sender<bool>,
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

        tracing::info!("TreeState synced with DB");

        tree_synced_tx.send(true)?;
    }
}
