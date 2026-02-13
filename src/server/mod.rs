use std::sync::Arc;

use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use hyper::StatusCode;
use prometheus::{Encoder, TextEncoder};
use thiserror::Error;
use tokio::net::TcpListener;
use tower_http::catch_panic::{CatchPanicLayer, ResponseForPanic};
use tracing::info;

use crate::app::App;
use crate::config::ServerConfig;
use crate::shutdown::Shutdown;
use crate::utils::auth::AuthValidator;

pub mod api_v1;
mod api_v2;
mod api_v3;
mod middlewares;

/// # Errors
///
/// Will return `Err` if `options.server` URI is not http, incorrectly includes
/// a path beyond `/`, or cannot be cast into an IP address. Also returns an
/// `Err` if the server cannot bind to the given address.
pub async fn run(app: Arc<App>, config: ServerConfig, shutdown: Shutdown) -> anyhow::Result<()> {
    info!("Will listen on {}", config.address);
    let listener = TcpListener::bind(config.address).await?;

    bind_from_listener(app, &config, listener, shutdown).await?;

    Ok(())
}

#[derive(Clone)]
struct PanicHandler {}

impl ResponseForPanic for PanicHandler {
    type ResponseBody = Body;

    fn response_for_panic(
        &mut self,
        error: Box<dyn std::any::Any + Send + 'static>,
    ) -> hyper::Response<Self::ResponseBody> {
        tracing::error!(?error, "request panicked");
        hyper::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::empty())
            .unwrap()
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Hyper(#[from] hyper::Error),
    #[error(transparent)]
    Http(#[from] hyper::http::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Error {
    fn to_status_code(&self) -> StatusCode {
        match self {
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        let status_code = self.to_status_code();

        let body = if let Self::Other(err) = self {
            format!("{err}")
        } else {
            self.to_string()
        };

        (status_code, body).into_response()
    }
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

/// # Errors
///
/// Will return `Err` if the provided `listener` address cannot be accessed or
/// if the server fails to bind to the given address.
pub async fn bind_from_listener(
    app: Arc<App>,
    config: &ServerConfig,
    listener: TcpListener,
    shutdown: Shutdown,
) -> anyhow::Result<()> {
    let auth_validator = AuthValidator::new(
        config.auth_mode.clone(),
        config.basic_auth_credentials.clone(),
        &config.authorized_keys,
    )?;

    info!(
        "Authentication: mode={:?}, basic_auth_users={}, jwt_keys={}",
        config.auth_mode,
        config.basic_auth_credentials.len(),
        config.authorized_keys.len()
    );

    let router = Router::new()
        .merge(api_v1::api_v1_router(
            app.clone(),
            config.serve_timeout,
            auth_validator.clone(),
        ))
        .merge(api_v2::api_v2_router(
            app.clone(),
            config.serve_timeout,
            auth_validator.clone(),
        ))
        .merge(api_v3::api_v3_router(
            app.clone(),
            config.serve_timeout,
            auth_validator.clone(),
        ))
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .layer(CatchPanicLayer::custom(PanicHandler {}));

    let _shutdown_handle = shutdown.handle();

    let server = axum::serve(listener, router).with_graceful_shutdown(async move {
        shutdown.await_shutdown_begin().await;
    });

    server.await?;

    info!("Server gracefully shutdown");

    Ok(())
}
