pub mod error;

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, ensure, Result as AnyhowResult};
use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{middleware, Json, Router};
use clap::Parser;
use cli_batteries::await_shutdown;
use error::Error;
use hyper::StatusCode;
use tracing::info;
use url::{Host, Url};

use crate::app::App;

mod custom_middleware;
pub mod data;

use self::data::{
    AddBatchSizeRequest, DeletionRequest, IdentityHistoryRequest, IdentityHistoryResponse,
    InclusionProofRequest, InclusionProofResponse, InsertCommitmentRequest, ListBatchSizesResponse,
    RecoveryRequest, RemoveBatchSizeRequest, ToResponseCode, VerifySemaphoreProofQuery,
    VerifySemaphoreProofRequest, VerifySemaphoreProofResponse,
};

#[derive(Clone, Debug, PartialEq, Eq, Parser)]
#[group(skip)]
pub struct Options {
    // TODO: This should be a `SocketAddr`. It makes no sense for us to allow a full on URL here
    /// API Server url
    #[clap(long, env, default_value = "http://127.0.0.1:8080/")]
    pub server: Url,

    /// Request handling timeout (seconds)
    #[clap(long, env, default_value = "300")]
    pub serve_timeout: u64,
}

async fn inclusion_proof(
    State(app): State<Arc<App>>,
    Json(inclusion_proof_request): Json<InclusionProofRequest>,
) -> Result<(StatusCode, Json<InclusionProofResponse>), Error> {
    let result = app
        .inclusion_proof(&inclusion_proof_request.identity_commitment)
        .await?;

    let result = result.hide_processed_status();

    Ok((result.to_response_code(), Json(result)))
}

async fn insert_identity(
    State(app): State<Arc<App>>,
    Json(insert_identity_request): Json<InsertCommitmentRequest>,
) -> Result<(), Error> {
    app.insert_identity(insert_identity_request.identity_commitment)
        .await?;

    Ok(())
}

async fn verify_semaphore_proof(
    State(app): State<Arc<App>>,
    Query(verify_semaphore_proof_query): Query<VerifySemaphoreProofQuery>,
    Json(verify_semaphore_proof_request): Json<VerifySemaphoreProofRequest>,
) -> Result<(StatusCode, Json<VerifySemaphoreProofResponse>), Error> {
    let result = app
        .verify_semaphore_proof(
            &verify_semaphore_proof_request,
            &verify_semaphore_proof_query,
        )
        .await?;

    let result = result.hide_processed_status();

    Ok((result.to_response_code(), Json(result)))
}

async fn add_batch_size(
    State(app): State<Arc<App>>,
    Json(req): Json<AddBatchSizeRequest>,
) -> Result<(), Error> {
    app.add_batch_size(
        req.url,
        req.batch_size,
        req.timeout_seconds,
        req.prover_type,
    )
    .await?;

    Ok(())
}

async fn delete_identity(
    State(app): State<Arc<App>>,
    Json(req): Json<DeletionRequest>,
) -> Result<(), Error> {
    app.delete_identity(&req.identity_commitment).await?;
    Ok(())
}

async fn recover_identity(
    State(app): State<Arc<App>>,
    Json(req): Json<RecoveryRequest>,
) -> Result<(), Error> {
    app.recover_identity(
        &req.previous_identity_commitment,
        &req.new_identity_commitment,
    )
    .await?;

    Ok(())
}

async fn identity_history(
    State(app): State<Arc<App>>,
    Json(req): Json<IdentityHistoryRequest>,
) -> Result<Json<IdentityHistoryResponse>, Error> {
    let history = app.identity_history(&req.identity_commitment).await?;

    Ok(Json(IdentityHistoryResponse { history }))
}

async fn remove_batch_size(
    State(app): State<Arc<App>>,
    Json(req): Json<RemoveBatchSizeRequest>,
) -> Result<(), Error> {
    app.remove_batch_size(req.batch_size, req.prover_type)
        .await?;

    Ok(())
}

async fn list_batch_sizes(
    State(app): State<Arc<App>>,
) -> Result<(StatusCode, Json<ListBatchSizesResponse>), Error> {
    let result = app.list_batch_sizes().await?;

    Ok((result.to_response_code(), Json(result)))
}

/// # Errors
///
/// Will return `Err` if `options.server` URI is not http, incorrectly includes
/// a path beyond `/`, or cannot be cast into an IP address. Also returns an
/// `Err` if the server cannot bind to the given address.
pub async fn main(app: Arc<App>, options: Options) -> AnyhowResult<()> {
    ensure!(
        options.server.scheme() == "http",
        "Only http:// is supported in {}",
        options.server
    );
    ensure!(
        options.server.path() == "/",
        "Only / is supported in {}",
        options.server
    );

    let ip: IpAddr = match options.server.host() {
        Some(Host::Ipv4(ip)) => ip.into(),
        Some(Host::Ipv6(ip)) => ip.into(),
        Some(_) => bail!("Cannot bind {}", options.server),
        None => Ipv4Addr::LOCALHOST.into(),
    };
    let port = options.server.port().unwrap_or(9998);
    let addr = SocketAddr::new(ip, port);

    info!("Will listen on {}", addr);
    let listener = TcpListener::bind(addr)?;

    let serve_timeout = Duration::from_secs(options.serve_timeout);
    bind_from_listener(app, serve_timeout, listener).await?;

    Ok(())
}

/// # Errors
///
/// Will return `Err` if the provided `listener` address cannot be accessed or
/// if the server fails to bind to the given address.
pub async fn bind_from_listener(
    app: Arc<App>,
    serve_timeout: Duration,
    listener: TcpListener,
) -> AnyhowResult<()> {
    let router = Router::new()
        .route("/verifySemaphoreProof", post(verify_semaphore_proof))
        .route("/inclusionProof", post(inclusion_proof))
        .route("/insertIdentity", post(insert_identity))
        .route("/deleteIdentity", post(delete_identity))
        .route("/recoverIdentity", post(recover_identity))
        .route("/identityHistory", post(identity_history))
        // Operate on batch sizes
        .route("/addBatchSize", post(add_batch_size))
        .route("/removeBatchSize", post(remove_batch_size))
        .route("/listBatchSizes", get(list_batch_sizes))
        .layer(middleware::from_fn(
            custom_middleware::api_metrics_layer::middleware,
        ))
        .layer(middleware::from_fn_with_state(
            serve_timeout,
            custom_middleware::timeout_layer::middleware,
        ))
        .layer(middleware::from_fn(
            custom_middleware::logging_layer::middleware,
        ))
        .layer(middleware::from_fn(
            custom_middleware::remove_auth_layer::middleware,
        ))
        .with_state(app.clone());

    let server = axum::Server::from_tcp(listener)?
        .serve(router.into_make_service())
        .with_graceful_shutdown(await_shutdown());

    server.await?;

    Ok(())
}
