use axum::http::header::AUTHORIZATION;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

pub async fn middleware<B>(mut request: Request<B>, next: Next<B>) -> Result<Response, StatusCode> {
    request.headers_mut().remove(AUTHORIZATION);

    let response = next.run(request).await;
    Ok(response)
}
