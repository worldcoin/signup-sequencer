use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};

use crate::app::App;
use crate::identity::processor::TransactionId;

pub async fn monitor_txs(
    app: Arc<App>,
    monitored_txs_receiver: Arc<Mutex<mpsc::Receiver<TransactionId>>>,
) -> anyhow::Result<()> {
    let mut monitored_txs_receiver = monitored_txs_receiver.lock().await;

    while let Some(tx) = monitored_txs_receiver.recv().await {
        assert!(
            app.identity_processor.mine_transaction(tx.clone()).await?,
            "Failed to mine transaction: {}",
            tx
        );
    }

    Ok(())
}
