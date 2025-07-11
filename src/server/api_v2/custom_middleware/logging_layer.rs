#![allow(clippy::cast_possible_truncation)]

use axum::body::Body;
use axum::extract::{MatchedPath, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use bytes::{BufMut, Bytes};
use futures::StreamExt;
use hyper::{body::Body as _, Method};
use telemetry_batteries::tracing::{trace_from_headers, trace_to_headers};
use tracing::{error, info, info_span, warn, Instrument};

// 1 MiB
const MAX_REQUEST_BODY_SIZE: u64 = 1024 * 1024;

pub async fn middleware(request: Request, next: Next) -> Result<Response, StatusCode> {
    let (parts, body) = request.into_parts();

    let uri_path = parts.uri.path().to_string();
    let request_method = parts.method.clone();
    let request_query = parts.uri.query().map(ToString::to_string);

    let matched_path = parts
        .extensions
        .get::<MatchedPath>()
        .map(|mp| mp.as_str().to_string())
        .unwrap_or_else(|| uri_path.clone());

    if let Method::GET = request_method {
        let span = info_span!("request", ?uri_path, ?request_method, ?request_query);

        async {
            trace_from_headers(&parts.headers);

            info!(
                uri_path,
                ?request_method,
                ?request_query,
                "Processing request"
            );

            let body = Body::empty();
            let request = Request::from_parts(parts, body);

            let response = next.run(request).await;

            let mut response = handle_response(
                &matched_path,
                &uri_path,
                &request_method,
                request_query.as_deref(),
                response,
            )
            .await?;

            trace_to_headers(response.headers_mut());

            Ok(response)
        }
        .instrument(span)
        .await
    } else {
        let body = body_to_string(body).await?;

        let span = info_span!("request", ?uri_path, ?request_method, ?request_query, ?body);

        async {
            trace_from_headers(&parts.headers);

            info!(
                ?uri_path,
                ?request_method,
                ?request_query,
                ?body,
                "Processing request"
            );

            let body = Body::from(body);
            let request = Request::from_parts(parts, body);

            let response = next.run(request).await;

            let mut response = handle_response(
                &matched_path,
                &uri_path,
                &request_method,
                request_query.as_deref(),
                response,
            )
            .await?;

            trace_to_headers(response.headers_mut());

            Ok(response)
        }
        .instrument(span)
        .await
    }
}

async fn handle_response(
    matched_path: &str,
    uri_path: &str,
    request_method: &Method,
    request_query: Option<&str>,
    response: Response,
) -> Result<Response, StatusCode> {
    let (parts, body) = response.into_parts();

    let response_status = parts.status;

    let response = if response_status.is_client_error() || response_status.is_server_error() {
        let response_body = body_to_string(body).await?;

        if response_status.is_client_error() {
            warn!(
                uri_path,
                ?request_method,
                ?request_query,
                ?response_status,
                ?response_body,
                "Error processing request"
            );
        } else {
            error!(
                uri_path,
                ?request_method,
                ?request_query,
                ?response_status,
                ?response_body,
                "Error processing request"
            );
        }

        let body = Body::from(response_body);

        Response::from_parts(parts, body)
    } else {
        Response::from_parts(parts, body)
    };

    info!(
        matched_path,
        uri_path,
        ?request_method,
        ?request_query,
        ?response_status,
        "Finished processing request"
    );

    Ok(response)
}

/// Reads the body of a request into a `Bytes` object chunk by chunk
/// and returns an error if the body is larger than `MAX_REQUEST_BODY_SIZE`.
async fn body_to_bytes_safe(body: Body) -> Result<Bytes, StatusCode> {
    let size_hint = body
        .size_hint()
        .upper()
        .unwrap_or_else(|| body.size_hint().lower());

    if size_hint > MAX_REQUEST_BODY_SIZE {
        error!(
            "Request body too large: {} bytes (max: {} bytes)",
            size_hint, MAX_REQUEST_BODY_SIZE
        );

        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    let mut body_bytes = Vec::with_capacity(size_hint as usize);
    let mut data_stream = body.into_data_stream();

    while let Some(chunk) = data_stream.next().await {
        let chunk = chunk.map_err(|error| {
            error!("Error reading body: {}", error);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        body_bytes.put(chunk);

        if body_bytes.len() > MAX_REQUEST_BODY_SIZE as usize {
            error!(
                "Request body too large: {} bytes (max: {} bytes)",
                body_bytes.len(),
                MAX_REQUEST_BODY_SIZE
            );

            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
    }

    Ok(body_bytes.into())
}

async fn body_to_string(body: Body) -> Result<String, StatusCode> {
    let body_bytes = body_to_bytes_safe(body).await?;

    let s = match String::from_utf8(body_bytes.to_vec()) {
        Ok(s) => s,
        Err(error) => {
            error!("Error converting body to string: {}", error);
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    Ok(s)
}
