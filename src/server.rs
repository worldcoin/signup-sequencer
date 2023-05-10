use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener},
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, ensure, Error as EyreError, Result as AnyhowResult};
use axum::{extract::State, middleware, response::IntoResponse, routing::post, Json, Router};
use clap::Parser;
use cli_batteries::await_shutdown;
use hyper::StatusCode;
use semaphore::{protocol::Proof, Field};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tower_http::trace::TraceLayer;
use tracing::{error, instrument};
use url::{Host, Url};

use crate::{
    app::{App, InclusionProofResponse, VerifySemaphoreProofResponse},
    database,
    identity_tree::Hash,
};

mod api_metrics_layer;
mod extract_trace_layer;
mod timeout_layer;

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

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct InsertCommitmentRequest {
    identity_commitment: Hash,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct InclusionProofRequest {
    pub identity_commitment: Hash,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct VerifySemaphoreProofRequest {
    pub root:                    Field,
    pub signal_hash:             Field,
    pub nullifier_hash:          Field,
    pub external_nullifier_hash: Field,
    pub proof:                   Proof,
}

pub trait ToResponseCode {
    fn to_response_code(&self) -> StatusCode;
}

impl ToResponseCode for () {
    fn to_response_code(&self) -> StatusCode {
        StatusCode::OK
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid http method")]
    InvalidMethod,
    #[error("invalid path")]
    InvalidPath,
    #[error("invalid content type")]
    InvalidContentType,
    #[error("invalid group id")]
    InvalidGroupId,
    #[error("invalid root")]
    InvalidRoot,
    #[error("invalid semaphore proof")]
    InvalidProof,
    #[error("provided identity index out of bounds")]
    IndexOutOfBounds,
    #[error("provided identity commitment not found")]
    IdentityCommitmentNotFound,
    #[error("provided identity commitment is invalid")]
    InvalidCommitment,
    #[error("provided identity commitment is not in reduced form")]
    UnreducedCommitment,
    #[error("provided identity commitment is already included")]
    DuplicateCommitment,
    #[error("Root mismatch between tree and contract.")]
    RootMismatch,
    #[error("invalid JSON request: {0}")]
    InvalidSerialization(#[from] serde_json::Error),
    #[error(transparent)]
    Database(#[from] database::Error),
    #[error(transparent)]
    Hyper(#[from] hyper::Error),
    #[error(transparent)]
    Http(#[from] hyper::http::Error),
    #[error("not semaphore manager")]
    NotManager,
    #[error(transparent)]
    Elapsed(#[from] tokio::time::error::Elapsed),
    #[error("prover error")]
    ProverError,
    #[error("Failed to insert identity")]
    FailedToInsert,
    #[error(transparent)]
    Other(#[from] EyreError),
}

impl Error {
    fn to_status_code(&self) -> StatusCode {
        match self {
            Self::InvalidMethod => StatusCode::METHOD_NOT_ALLOWED,
            Self::InvalidPath => StatusCode::NOT_FOUND,
            Self::InvalidContentType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::IndexOutOfBounds
            | Self::IdentityCommitmentNotFound
            | Self::InvalidCommitment
            | Self::InvalidSerialization(_) => StatusCode::BAD_REQUEST,
            Self::DuplicateCommitment => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        let status_code = self.to_status_code();

        let body = if let Self::Other(err) = self {
            format!("{err:?}")
        } else {
            self.to_string()
        };

        (status_code, body).into_response()
    }
}

#[instrument(level = "info", skip_all)]
async fn inclusion_proof(
    State(app): State<Arc<App>>,
    Json(inclusion_proof_request): Json<InclusionProofRequest>,
) -> Result<Json<InclusionProofResponse>, Error> {
    let result = app
        .inclusion_proof(&inclusion_proof_request.identity_commitment)
        .await?;

    Ok(Json(result))
}

#[instrument(level = "info", skip_all)]
async fn insert_identity(
    State(app): State<Arc<App>>,
    Json(insert_identity_request): Json<InsertCommitmentRequest>,
) -> Result<Json<InclusionProofResponse>, Error> {
    let result = app
        .insert_identity(insert_identity_request.identity_commitment)
        .await?;

    Ok(Json(result))
}

#[instrument(level = "info", skip_all)]
async fn verify_semaphore_proof(
    State(app): State<Arc<App>>,
    Json(verify_semaphore_proof_request): Json<VerifySemaphoreProofRequest>,
) -> Result<Json<VerifySemaphoreProofResponse>, Error> {
    let result = app
        .verify_semaphore_proof(&verify_semaphore_proof_request)
        .await?;

    Ok(Json(result))
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
        .layer(TraceLayer::new_for_http())
        // Custom layers
        .layer(middleware::from_fn(api_metrics_layer::middleware))
        .layer(middleware::from_fn_with_state(
            serve_timeout,
            timeout_layer::middleware,
        ))
        .layer(middleware::from_fn(extract_trace_layer::middleware))
        .with_state(app.clone());

    let server = axum::Server::from_tcp(listener)?
        .serve(router.into_make_service())
        .with_graceful_shutdown(await_shutdown());

    server.await?;

    Ok(())
}
