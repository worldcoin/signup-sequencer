use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::utils::auth::{AuthResult, AuthValidator};

pub async fn middleware(
    State(validator): State<AuthValidator>,
    request: Request,
    next: Next,
) -> Response {
    match validator.validate(&request) {
        AuthResult::Allowed => next.run(request).await,
        AuthResult::AllowedWithWarning(msg) => {
            tracing::warn!("{}", msg);
            next.run(request).await
        }
        AuthResult::Denied(msg) => {
            tracing::warn!("Authentication failed: {}", msg);
            (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
        }
    }
}
