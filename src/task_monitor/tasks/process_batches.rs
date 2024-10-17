use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Notify};
use tokio::{select, time};

use crate::app::App;
use crate::database::methods::DbMethods as _;
use crate::identity::processor::TransactionId;

pub async fn process_batches(
    app: Arc<App>,
    monitored_txs_sender: Arc<mpsc::Sender<TransactionId>>,
    next_batch_notify: Arc<Notify>,
    wake_up_notify: Arc<Notify>,
) -> anyhow::Result<()> {
    tracing::info!("Awaiting for a clean slate");
    app.identity_processor.await_clean_slate().await?;

    // This is a tricky way to know that we are not changing data during tree
    // initialization process.
    _ = app.tree_state()?;
    tracing::info!("Starting identity processor.");

    let mut timer = time::interval(Duration::from_secs(5));

    loop {
        // We wait either for a timer tick or a full batch
        select! {
            _ = timer.tick() => {
                tracing::info!("Identity processor woken due to timeout");
            }

            () = next_batch_notify.notified() => {
                tracing::trace!("Identity processor woken due to next batch creation");
            },

            () = wake_up_notify.notified() => {
                tracing::trace!("Identity processor woken due to request");
            },
        }

        let next_batch = app.database.get_next_batch_without_transaction().await?;
        let Some(next_batch) = next_batch else {
            continue;
        };

        let tx_id = app
            .identity_processor
            .commit_identities(&next_batch)
            .await?;

        monitored_txs_sender.send(tx_id.clone()).await?;

        app.database
            .insert_new_transaction(&tx_id, &next_batch.next_root)
            .await?;

        // We want to check if there's a full batch available immediately
        wake_up_notify.notify_one();
    }
}
