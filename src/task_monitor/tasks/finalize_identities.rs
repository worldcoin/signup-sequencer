use std::sync::Arc;

use crate::app::App;

pub async fn finalize_roots(app: Arc<App>) -> anyhow::Result<()> {
    let mut transaction_finalizer = app
        .transaction_manager
        .produce_transaction_finalizer()
        .await?;

    loop {
        transaction_finalizer
            .finalize_identities(
                app.tree_state()?.processed_tree(),
                app.tree_state()?.mined_tree(),
            )
            .await?;

        tokio::time::sleep(app.config.app.time_between_scans).await;
    }
}
