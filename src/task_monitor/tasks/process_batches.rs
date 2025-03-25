use std::sync::Arc;
use std::time::Duration;

use crate::app::App;
use crate::database::methods::DbMethods as _;
use crate::identity::processor::TransactionId;
use tokio::sync::{mpsc, Notify};
use tokio::time::MissedTickBehavior;
use tokio::{select, time};

const MAX_BUFFERED_TRANSACTIONS: i32 = 5;

pub async fn process_batches(
    app: Arc<App>,
    monitored_txs_sender: Arc<mpsc::Sender<TransactionId>>,
    next_batch_notify: Arc<Notify>,
) -> anyhow::Result<()> {
    tracing::info!("Starting identity processor.");

    let mut timer = time::interval(Duration::from_secs(5));
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let check_next_batch_notify = Notify::new();

    loop {
        // We wait either for a timer tick or a full batch
        select! {
            _ = timer.tick() => {
                tracing::info!("Process batches woken due to timeout");
            }

            () = next_batch_notify.notified() => {
                tracing::trace!("Process batches woken due to next batch request");
            },

            () = check_next_batch_notify.notified() => {
                tracing::trace!("Process batches woken due instant check for next batch");
            },
        }

        if run(&app, &monitored_txs_sender).await? {
            // We want to check if there's a full batch available immediately
            check_next_batch_notify.notify_one();
        }
    }
}

async fn run(
    app: &Arc<App>,
    monitored_txs_sender: &Arc<mpsc::Sender<TransactionId>>,
) -> anyhow::Result<bool> {
    let mut tx = app.database.pool.begin().await?;

    sqlx::query("LOCK TABLE transactions IN ACCESS EXCLUSIVE MODE;")
        .execute(&mut *tx)
        .await?;

    let buffered_transactions = tx.count_not_finalized_batches().await?;
    if buffered_transactions >= MAX_BUFFERED_TRANSACTIONS {
        tx.commit().await?;
        return Ok(false);
    }

    let next_batch = tx.get_next_batch_without_transaction().await?;
    let Some(next_batch) = next_batch else {
        tx.commit().await?;
        return Ok(false);
    };

    let tx_id = app
        .identity_processor
        .commit_identities(&next_batch)
        .await?;

    monitored_txs_sender.send(tx_id.clone()).await?;

    tx.insert_new_transaction(&tx_id, &next_batch.next_root)
        .await?;

    tx.commit().await?;

    Ok(true)
}
