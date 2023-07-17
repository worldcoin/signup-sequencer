use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::sync::Arc;

use anyhow::Context;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use ethers::prelude::k256::ecdsa::SigningKey;
use ethers::types::Address;
use oz_api::data::transactions::{RelayerTransactionBase, SendBaseTransactionRequestOwned, Status};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::Pinhead;

async fn send_transaction(
    State(pinhead): State<Pinhead>,
    Json(request): Json<SendBaseTransactionRequestOwned>,
) -> Result<Json<RelayerTransactionBase>, StatusCode> {
    let result = pinhead.send_transaction(request).await;

    match result {
        Ok(tx) => Ok(Json(tx)),
        Err(err) => {
            tracing::error!("Pinhead send_transaction error: {:?}", err);

            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ListTransactionsQuery {
    #[serde(default)]
    status: Option<Status>,
    #[serde(default)]
    limit:  Option<usize>,
}

async fn list_transactions(
    State(pinhead): State<Pinhead>,
    Query(query): Query<ListTransactionsQuery>,
) -> Result<Json<Vec<RelayerTransactionBase>>, StatusCode> {
    let txs = pinhead.list_transactions(query.status, query.limit).await;

    match txs {
        Ok(txs) => Ok(Json(txs)),
        Err(err) => {
            tracing::error!("Pinhead list_transactions error: {:?}", err);

            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn query_transaction(
    State(pinhead): State<Pinhead>,
    Path(tx_id): Path<String>,
) -> Result<Json<RelayerTransactionBase>, StatusCode> {
    let tx = pinhead.query_transaction(&tx_id).await;

    match tx {
        Ok(tx) => Ok(Json(tx)),
        Err(err) => {
            tracing::error!("Pinhead query_transaction error: {:?}", err);

            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

pub struct ServerHandle {
    pinhead:            Pinhead,
    addr:               SocketAddr,
    shutdown_notify:    Arc<Notify>,
    server_join_handle: JoinHandle<Result<(), hyper::Error>>,
}

impl ServerHandle {
    pub fn address(&self) -> Address {
        self.pinhead.inner.signer.address()
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub async fn shutdown(self) {
        self.shutdown_notify.notify_waiters();

        if let Err(e) = self.server_join_handle.await {
            tracing::error!("Server error: {:?}", e);
        }
    }
}

pub async fn spawn(rpc_url: String, secret_key: SigningKey) -> anyhow::Result<ServerHandle> {
    let pinhead = Pinhead::new(rpc_url, secret_key).await?;

    let router = Router::new()
        .route("/txs", post(send_transaction).get(list_transactions))
        .route("/txs/:tx_id", get(query_transaction))
        .with_state(pinhead.clone());

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let listener = TcpListener::bind(addr).context("Failed to bind random port")?;
    let local_addr = listener.local_addr()?;

    let shutdown_notify = Arc::new(Notify::new());

    let server = axum::Server::from_tcp(listener)?
        .serve(router.into_make_service())
        .with_graceful_shutdown({
            let shutdown_notify = shutdown_notify.clone();
            async move {
                shutdown_notify.notified().await;
            }
        });

    let server_join_handle = tokio::spawn(server);

    Ok(ServerHandle {
        pinhead,
        addr: local_addr,
        shutdown_notify,
        server_join_handle,
    })
}
