use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

pub async fn middleware<B>(request: Request<B>, next: Next<B>) -> Result<Response, StatusCode> {
    let (parts, body) = request.into_parts();

    cli_batteries::trace_from_headers(&parts.headers);

    let request = Request::from_parts(parts, body);

    let mut response = next.run(request).await;

    cli_batteries::trace_to_headers(response.headers_mut());

    Ok(response)
}
