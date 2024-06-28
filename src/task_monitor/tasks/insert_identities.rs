use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use tokio::time;
use tracing::info;

use crate::app::App;
use crate::database::query::DatabaseQuery as _;
use crate::identity_tree::UnprocessedStatus;

pub async fn insert_identities(
    app: Arc<App>,
    pending_insertions_mutex: Arc<Mutex<()>>,
    wake_up_notify: Arc<Notify>,
) -> anyhow::Result<()> {
    info!("Starting insertion processor task.");

    let mut timer = time::interval(Duration::from_secs(5));

    loop {
        _ = timer.tick().await;
        info!("Insertion processor woken due to timeout.");

        // get commits from database
        let unprocessed = app
            .database
            .get_eligible_unprocessed_commitments(UnprocessedStatus::New)
            .await?;
        if unprocessed.is_empty() {
            continue;
        }

        app.database
            .insert_identities_batch_tx(
                app.tree_state()?.latest_tree(),
                unprocessed,
                &pending_insertions_mutex,
            )
            .await?;

        // Notify the identity processing task, that there are new identities
        wake_up_notify.notify_one();
    }
}
