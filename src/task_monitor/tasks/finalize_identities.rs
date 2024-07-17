use std::sync::Arc;

use tokio::sync::Notify;

use crate::app::App;

pub async fn finalize_roots(app: Arc<App>, sync_tree_notify: Arc<Notify>) -> anyhow::Result<()> {
    loop {
        app.identity_processor
            .finalize_identities(&sync_tree_notify)
            .await?;

        tokio::time::sleep(app.config.app.time_between_scans).await;
    }
}
