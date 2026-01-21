use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::super::data::ErrorResponse;
use crate::utils::jwt::JwtValidator;

fn extract_bearer_token(request: &Request) -> Option<&str> {
    request
        .headers()
        .get("Authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

pub async fn middleware(
    State(validator): State<JwtValidator>,
    request: Request,
    next: Next,
) -> Response {
    let token = extract_bearer_token(&request);

    match token {
        Some(token) => match validator.validate(token) {
            Ok(key_name) => {
                tracing::info!(key = %key_name, "Request authenticated");
                next.run(request).await
            }
            Err(e) => {
                if validator.require_auth() {
                    tracing::warn!(error = %e, "Authentication failed - rejecting request");
                    (
                        StatusCode::UNAUTHORIZED,
                        Json(ErrorResponse::new("unauthorized", "Invalid or missing authentication token")),
                    )
                        .into_response()
                } else {
                    tracing::warn!(error = %e, "Authentication failed - allowing request (require_auth=false)");
                    next.run(request).await
                }
            }
        },
        None => {
            if validator.require_auth() {
                tracing::warn!("No Authorization header - rejecting request");
                (
                    StatusCode::UNAUTHORIZED,
                    Json(ErrorResponse::new("unauthorized", "Invalid or missing authentication token")),
                )
                    .into_response()
            } else {
                tracing::warn!("No Authorization header - allowing request (require_auth=false)");
                next.run(request).await
            }
        }
    }
}
