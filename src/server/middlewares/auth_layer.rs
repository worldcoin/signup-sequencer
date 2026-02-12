use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

use crate::utils::auth::{AuthResponseFormatter, AuthResult, AuthValidator};

pub async fn middleware(
    State((validator, response_formatter)): State<(AuthValidator, AuthResponseFormatter)>,
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
            response_formatter(msg)
        }
    }
}
