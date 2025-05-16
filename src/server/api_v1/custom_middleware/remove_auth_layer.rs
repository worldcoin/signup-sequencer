use axum::extract::Request;
use axum::http::header::AUTHORIZATION;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;

pub async fn middleware(mut request: Request, next: Next) -> Result<Response, StatusCode> {
    request.headers_mut().remove(AUTHORIZATION);

    let response = next.run(request).await;

    Ok(response)
}
