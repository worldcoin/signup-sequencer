use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::Router;
use hyper::StatusCode;
use tokio::net::TcpListener;
use tower_http::catch_panic::{CatchPanicLayer, ResponseForPanic};
use tracing::info;

use crate::app::App;
use crate::config::ServerConfig;
use crate::shutdown::Shutdown;

pub mod api_v1;
mod api_v2;

/// # Errors
///
/// Will return `Err` if `options.server` URI is not http, incorrectly includes
/// a path beyond `/`, or cannot be cast into an IP address. Also returns an
/// `Err` if the server cannot bind to the given address.
pub async fn run(app: Arc<App>, config: ServerConfig, shutdown: Shutdown) -> anyhow::Result<()> {
    info!("Will listen on {}", config.address);
    let listener = TcpListener::bind(config.address).await?;

    bind_from_listener(app, config.serve_timeout, listener, shutdown).await?;

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

/// # Errors
///
/// Will return `Err` if the provided `listener` address cannot be accessed or
/// if the server fails to bind to the given address.
pub async fn bind_from_listener(
    app: Arc<App>,
    serve_timeout: Duration,
    listener: TcpListener,
    shutdown: Shutdown,
) -> anyhow::Result<()> {
    let router = Router::new()
        .merge(api_v1::api_v1_router(app.clone(), serve_timeout))
        .merge(api_v2::api_v2_router(app.clone(), serve_timeout))
        .layer(CatchPanicLayer::custom(PanicHandler {}))
        ;

    let _shutdown_handle = shutdown.handle();

    let server = axum::serve(listener, router).with_graceful_shutdown(async move {
        shutdown.await_shutdown_begin().await;
    });

    server.await?;

    info!("Server gracefully shutdown");

    Ok(())
}
