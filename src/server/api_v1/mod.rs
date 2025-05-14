use std::sync::Arc;
use std::time::Duration;

use crate::app::App;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{middleware, Json, Router};
use data::VerifyCompressedSemaphoreProofRequest;
use error::Error;
use hyper::header::CONTENT_TYPE;
use hyper::StatusCode;
use prometheus::{Encoder, TextEncoder};

mod custom_middleware;
pub mod data;
pub mod error;

use self::data::{
    AddBatchSizeRequest, DeletionRequest, InclusionProofRequest, InclusionProofResponse,
    InsertCommitmentRequest, ListBatchSizesResponse, RemoveBatchSizeRequest, ToResponseCode,
    VerifySemaphoreProofQuery, VerifySemaphoreProofRequest, VerifySemaphoreProofResponse,
};

async fn inclusion_proof(
    State(app): State<Arc<App>>,
    Json(inclusion_proof_request): Json<InclusionProofRequest>,
) -> Result<(StatusCode, Json<InclusionProofResponse>), Error> {
    let result = app
        .inclusion_proof(&inclusion_proof_request.identity_commitment)
        .await?;

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

    Ok((result.to_response_code(), Json(result)))
}

async fn verify_compressed_semaphore_proof(
    State(app): State<Arc<App>>,
    Query(verify_semaphore_proof_query): Query<VerifySemaphoreProofQuery>,
    Json(verify_semaphore_proof_request): Json<VerifyCompressedSemaphoreProofRequest>,
) -> Result<(StatusCode, Json<VerifySemaphoreProofResponse>), Error> {
    let verify_semaphore_proof_request = verify_semaphore_proof_request.decompress()?;

    verify_semaphore_proof(
        State(app),
        Query(verify_semaphore_proof_query),
        Json(verify_semaphore_proof_request),
    )
    .await
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
    let batches = app.list_batch_sizes().await?;
    let result = batches;

    Ok((result.to_response_code(), Json(result)))
}

async fn health() -> Result<(), Error> {
    Ok(())
}

async fn metrics() -> Result<Response<Body>, Error> {
    let encoder = TextEncoder::new();

    let metric_families = prometheus::gather();
    let mut buffer = vec![];
    encoder
        .encode(&metric_families, &mut buffer)
        .map_err(|e| Error::Other(e.into()))?;

    let response = Response::builder()
        .status(200)
        .header(CONTENT_TYPE, encoder.format_type())
        .body(Body::from(buffer))?;

    Ok(response)
}

pub fn api_v1_router(app: Arc<App>, serve_timeout: Duration) -> Router {
    Router::new()
        // Operate on identity commitments
        .route("/verifySemaphoreProof", post(verify_semaphore_proof))
        .route(
            "/verifyCompressedSemaphoreProof",
            post(verify_compressed_semaphore_proof),
        )
        .route("/inclusionProof", post(inclusion_proof))
        .route("/insertIdentity", post(insert_identity))
        .route("/deleteIdentity", post(delete_identity))
        // Operate on batch sizes
        .route("/addBatchSize", post(add_batch_size))
        .route("/removeBatchSize", post(remove_batch_size))
        .route("/listBatchSizes", get(list_batch_sizes))
        // Health check, return 200 OK
        .route("/health", get(health))
        .route("/metrics", get(metrics))
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
        .with_state(app.clone())
}
