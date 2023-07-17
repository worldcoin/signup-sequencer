use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;

use anyhow::Context;
use chrono::Utc;
use ethers::prelude::k256::ecdsa::SigningKey;
use ethers::prelude::k256::SecretKey;
use ethers::prelude::SignerMiddleware;
use ethers::providers::{Http, Middleware, Provider};
use ethers::signers::LocalWallet;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::Eip1559TransactionRequest;
use oz_api::data::transactions::{RelayerTransactionBase, SendBaseTransactionRequestOwned, Status};
use tokio::sync::{mpsc, Mutex};

pub mod server;

pub use self::server::{spawn, ServerHandle};

type Signer = SignerMiddleware<Provider<Http>, LocalWallet>;

#[derive(Clone)]
pub struct Pinhead {
    inner: Arc<PinheadInner>,
}

struct PinheadInner {
    signer:         Arc<Signer>,
    is_running:     AtomicBool,
    tx_id_counter:  AtomicU64,
    txs_to_execute: mpsc::Sender<String>,
    txs:            Mutex<HashMap<String, Arc<Mutex<RelayerTransactionBase>>>>,
}

impl Drop for PinheadInner {
    fn drop(&mut self) {
        self.is_running
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

async fn runner(
    inner: Arc<PinheadInner>,
    mut txs_to_execute: mpsc::Receiver<String>,
) -> anyhow::Result<()> {
    loop {
        let Some(tx_id) = txs_to_execute.recv().await else {
            break;
        };

        let tx = inner
            .txs
            .lock()
            .await
            .get(&tx_id)
            .expect("Missing tx")
            .clone();

        let mut typed_tx = {
            let tx_guard = tx.lock().await;

            TypedTransaction::Eip1559(Eip1559TransactionRequest {
                to: Some(tx_guard.to.clone()),
                value: tx_guard.value.clone(),
                gas: Some(tx_guard.gas_limit.into()),
                data: tx_guard.data.clone(),
                ..Eip1559TransactionRequest::default()
            })
        };

        inner.signer.fill_transaction(&mut typed_tx, None).await?;

        let pending_tx = inner.signer.send_transaction(typed_tx, None).await?;

        {
            let mut tx_guard = tx.lock().await;

            tx_guard.status = Status::Pending;
            tx_guard.hash = Some(pending_tx.tx_hash());
        }

        let receipt = pending_tx.await?;

        let mut tx_guard = tx.lock().await;

        if let Some(_receipt) = receipt {
            tx_guard.status = Status::Mined;
        } else {
            tx_guard.status = Status::Failed;
        }
    }

    Ok(())
}

impl Pinhead {
    pub async fn new(rpc_url: String, secret_key: SigningKey) -> anyhow::Result<Self> {
        let provider = Provider::<Http>::try_from(rpc_url)?;
        let wallet = LocalWallet::from(secret_key);

        let signer = SignerMiddleware::new(provider, wallet);

        let is_running = AtomicBool::new(true);
        let tx_id_counter = AtomicU64::new(0);
        let txs = Mutex::new(HashMap::new());

        let (tx_sender, tx_receiver) = mpsc::channel(100);

        let inner = Arc::new(PinheadInner {
            signer: Arc::new(signer),
            tx_id_counter,
            is_running,
            txs_to_execute: tx_sender,
            txs,
        });

        tokio::spawn(runner(inner.clone(), tx_receiver));

        Ok(Self { inner })
    }

    pub async fn send_transaction(
        &self,
        tx_request: SendBaseTransactionRequestOwned,
    ) -> anyhow::Result<RelayerTransactionBase> {
        let mut txs = self.inner.txs.lock().await;

        let tx_id = self.next_tx_id();

        let tx = RelayerTransactionBase {
            transaction_id: tx_id.clone(),
            to:             tx_request.to.context("Missing to")?,
            value:          tx_request.value,
            gas_limit:      tx_request.gas_limit.context("Missing gas limit")?.as_u32(),
            data:           tx_request.data,
            status:         Status::Pending,
            hash:           None,
            valid_until:    tx_request
                .valid_until
                .unwrap_or(Utc::now() + chrono::Duration::hours(24)),
        };

        txs.insert(tx_id.clone(), Arc::new(Mutex::new(tx.clone())));

        self.inner.txs_to_execute.send(tx_id).await?;

        Ok(tx)
    }

    pub async fn list_transactions(
        &self,
        status: Option<Status>,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<RelayerTransactionBase>> {
        let txs = self.inner.txs.lock().await;

        let mut txs_to_return = vec![];

        for tx in txs.values() {
            let tx_guard = tx.lock().await;

            if let Some(status) = status {
                if tx_guard.status != status {
                    continue;
                }
            }

            txs_to_return.push(tx_guard.clone());

            if let Some(limit) = limit {
                if txs_to_return.len() >= limit {
                    break;
                }
            }
        }

        Ok(txs_to_return)
    }

    pub async fn query_transaction(&self, tx_id: &str) -> anyhow::Result<RelayerTransactionBase> {
        let txs = self.inner.txs.lock().await;

        let tx = txs
            .get(tx_id)
            .context(format!("Transaction {} not found", tx_id))?;

        let tx_guard = tx.lock().await;

        Ok(tx_guard.clone())
    }

    fn next_tx_id(&self) -> String {
        let id = self
            .inner
            .tx_id_counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        format!("tx-{}", id)
    }
}
