use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;

pub async fn middleware(
    State(timeout_duration): State<Duration>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    match tokio::time::timeout(timeout_duration, next.run(request)).await {
        Ok(response) => Ok(response),
        Err(_elapsed) => Err(StatusCode::REQUEST_TIMEOUT),
    }
}
