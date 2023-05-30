use std::time::Duration;

use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

pub async fn middleware<B>(
    State(timeout_duration): State<Duration>,
    request: Request<B>,
    next: Next<B>,
) -> Result<Response, StatusCode> {
    match tokio::time::timeout(timeout_duration, next.run(request)).await {
        Ok(response) => Ok(response),
        Err(_elapsed) => Err(StatusCode::REQUEST_TIMEOUT),
    }
}
