use std::sync::Arc;

use crate::app::App;
use tokio::sync::Notify;
use tokio::time;
use tokio::time::MissedTickBehavior;
use tracing::info;

pub async fn finalize_roots(app: Arc<App>, sync_tree_notify: Arc<Notify>) -> anyhow::Result<()> {
    info!("Starting finalize roots task.");

    let mut timer = time::interval(app.config.app.time_between_scans);
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        timer.tick().await;
        info!("Finalize roots woken due to timeout.");

        app.identity_processor
            .finalize_identities(&sync_tree_notify)
            .await?;
    }
}
