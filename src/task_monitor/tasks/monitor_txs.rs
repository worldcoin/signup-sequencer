use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};

use crate::contracts::{IdentityManager, SharedIdentityManager};
use crate::ethereum::write::TransactionId;

pub struct MonitorTxs {
    identity_manager:       SharedIdentityManager,
    monitored_txs_receiver: Arc<Mutex<mpsc::Receiver<TransactionId>>>,
}

impl MonitorTxs {
    pub fn new(
        identity_manager: SharedIdentityManager,
        monitored_txs_receiver: mpsc::Receiver<TransactionId>,
    ) -> Arc<Self> {
        Arc::new(Self {
            identity_manager,
            monitored_txs_receiver: Arc::new(Mutex::new(monitored_txs_receiver)),
        })
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        monitor_txs_loop(&self.identity_manager, &self.monitored_txs_receiver).await?;

        Ok(())
    }
}

async fn monitor_txs_loop(
    identity_manager: &IdentityManager,
    monitored_txs_receiver: &Mutex<mpsc::Receiver<TransactionId>>,
) -> anyhow::Result<()> {
    let mut monitored_txs_receiver = monitored_txs_receiver.lock().await;

    while let Some(tx) = monitored_txs_receiver.recv().await {
        assert!(
            (identity_manager.mine_transaction(tx.clone()).await?),
            "Failed to mine transaction: {}",
            tx
        );
    }

    Ok(())
}
