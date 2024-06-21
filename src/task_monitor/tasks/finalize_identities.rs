use std::sync::Arc;

use crate::app::App;

pub async fn finalize_roots(app: Arc<App>) -> anyhow::Result<()> {
    loop {
        app.identity_processor.finalize_identities().await?;

        tokio::time::sleep(app.config.app.time_between_scans).await;
    }
}
