use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;

use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use anyhow::Context;
use chrono::Utc;
use k256::ecdsa::SigningKey;
use oz_api::data::transactions::NameOrAddress;
use oz_api::data::transactions::{RelayerTransactionBase, SendBaseTransactionRequestOwned, Status};
use tokio::sync::{mpsc, Mutex};

pub mod server;

const DEFAULT_GAS_LIMIT: u32 = 1_000_000;

pub use self::server::{spawn, ServerHandle};

type PinheadSigner = DynProvider;

#[derive(Clone)]
pub struct Pinhead {
    inner: Arc<PinheadInner>,
}

struct PinheadInner {
    address: alloy::primitives::Address,
    signer: Arc<PinheadSigner>,
    is_running: AtomicBool,
    tx_id_counter: AtomicU64,
    txs_to_execute: mpsc::Sender<String>,
    txs: Mutex<HashMap<String, Arc<Mutex<RelayerTransactionBase>>>>,
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

        match runner_inner(&inner, tx_id).await {
            Ok(_) => {}
            Err(err) => {
                tracing::error!("Pinhead runner error: {:?}", err);
            }
        }
    }

    Ok(())
}

async fn runner_inner(inner: &Arc<PinheadInner>, tx_id: String) -> Result<(), anyhow::Error> {
    tracing::info!("Executing tx: {tx_id}");

    let tx = inner
        .txs
        .lock()
        .await
        .get(&tx_id)
        .expect("Missing tx")
        .clone();

    let typed_tx = {
        let tx_guard = tx.lock().await;

        TransactionRequest {
            to: Some(match &tx_guard.to {
                NameOrAddress::Address(address) => (*address).into(),
                NameOrAddress::Name(_) => {
                    anyhow::bail!("The local relayer requires an address, not an ENS name")
                }
            }),
            value: tx_guard.value,
            gas: Some(tx_guard.gas_limit.into()),
            input: tx_guard.data.clone().unwrap_or_default().into(),
            ..TransactionRequest::default()
        }
    };

    let pending_tx = inner.signer.send_transaction(typed_tx).await?;

    {
        let mut tx_guard = tx.lock().await;

        tx_guard.status = Status::Pending;
        tx_guard.hash = Some(*pending_tx.tx_hash());
    }

    tracing::info!("Awaiting for receipt");

    let receipt = pending_tx.get_receipt().await?;

    let mut tx_guard = tx.lock().await;

    tx_guard.status = if receipt.status() {
        Status::Mined
    } else {
        Status::Failed
    };

    Ok(())
}

impl Pinhead {
    pub async fn new(rpc_url: String, secret_key: SigningKey) -> anyhow::Result<Self> {
        let wallet = PrivateKeySigner::from(secret_key);
        let address = wallet.address();
        let signer = ProviderBuilder::new()
            .wallet(wallet)
            .connect_http(rpc_url.parse()?)
            .erased();

        let is_running = AtomicBool::new(true);
        let tx_id_counter = AtomicU64::new(0);
        let txs = Mutex::new(HashMap::new());

        let (tx_sender, tx_receiver) = mpsc::channel(100);

        let inner = Arc::new(PinheadInner {
            address,
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
            to: tx_request.to.context("Missing to")?,
            value: tx_request.value,
            gas_limit: tx_request
                .gas_limit
                .map(|gas_limit| gas_limit.to::<u32>())
                .unwrap_or(DEFAULT_GAS_LIMIT),
            data: tx_request.data,
            status: Status::Pending,
            hash: None,
            valid_until: tx_request
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
            .context(format!("Transaction {tx_id} not found"))?;

        let tx_guard = tx.lock().await;

        Ok(tx_guard.clone())
    }

    fn next_tx_id(&self) -> String {
        let id = self
            .inner
            .tx_id_counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        format!("tx-{id}")
    }
}
