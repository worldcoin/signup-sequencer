use axum::{
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};
use hyper::{body::HttpBody, Body, Method};
use tracing::{error, info};

pub async fn middleware<B>(request: Request<B>, next: Next<Body>) -> Result<Response, StatusCode>
where
    B: HttpBody,
    <B as HttpBody>::Error: std::error::Error,
{
    let (parts, body) = request.into_parts();

    let uri_path = parts.uri.path().to_string();
    let request_method = parts.method.clone();
    let request_query = parts.uri.query().map(|s| s.to_string());

    if let Method::GET = request_method {
        info!(
            uri_path,
            ?request_method,
            ?request_query,
            "Processing request"
        );

        let body = Body::empty();
        let request = Request::from_parts(parts, body);

        let response = next.run(request).await;

        let response = handle_response(
            &uri_path,
            &request_method,
            request_query.as_deref(),
            response,
        )
        .await?;

        return Ok(response);
    } else {
        let body = body_to_string(body).await?;

        info!(
            uri_path,
            ?request_method,
            ?request_query,
            body,
            "Processing request"
        );

        let body = Body::from(body);
        let request = Request::from_parts(parts, body);

        let response = next.run(request).await;

        let response = handle_response(
            &uri_path,
            &request_method,
            request_query.as_deref(),
            response,
        )
        .await?;

        Ok(response)
    }
}

async fn handle_response(
    uri_path: &str,
    request_method: &Method,
    request_query: Option<&str>,
    response: Response,
) -> Result<Response, StatusCode> {
    let (parts, body) = response.into_parts();

    let response_status = parts.status;

    let response = if response_status.is_client_error() || response_status.is_server_error() {
        let response_body = body_to_string(body).await?;

        error!(
            uri_path,
            ?request_method,
            ?request_query,
            ?response_status,
            ?response_body,
            "Error processing request"
        );

        let body = axum::body::boxed(Body::from(response_body));
        let response = Response::from_parts(parts, body);

        response
    } else {
        let response = Response::from_parts(parts, body);

        response
    };

    info!(
        uri_path,
        ?request_method,
        ?request_query,
        ?response_status,
        "Finished processing request"
    );

    Ok(response)
}

async fn body_to_string<B>(body: B) -> Result<String, StatusCode>
where
    B: HttpBody,
    <B as HttpBody>::Error: std::error::Error,
{
    let body_bytes = hyper::body::to_bytes(body).await;

    let body_bytes = match body_bytes {
        Ok(bytes) => bytes,
        Err(error) => {
            error!("Error reading body: {}", error);
            return Err(StatusCode::UNPROCESSABLE_ENTITY);
        }
    };

    let s = match String::from_utf8(body_bytes.to_vec()) {
        Ok(s) => s,
        Err(error) => {
            error!("Error converting body to string: {}", error);
            return Err(StatusCode::UNPROCESSABLE_ENTITY);
        }
    };

    Ok(s)
}
