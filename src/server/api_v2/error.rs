use axum::http::{StatusCode};
use axum::response::{IntoResponse, Response};
use thiserror::Error;
use axum::Json;
use crate::server::api_v2::data::ErrorResponse;

#[derive(Debug, Error)]
pub enum Error {
    #[error("bad request")]
    BadRequest(ErrorResponse),
    #[error("not found")]
    NotFound(ErrorResponse),
    #[error("conflict")]
    Conflict(ErrorResponse),
    #[error("gone")]
    Gone(ErrorResponse),
    #[error("internal server error")]
    InternalServerError(ErrorResponse),
}


impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self {
            Self::BadRequest(error_response) => {
                (
                    StatusCode::BAD_REQUEST,
                    Json(&error_response),
                )
                    .into_response()
            },
            Self::NotFound(error_response) => {
                (
                    StatusCode::NOT_FOUND,
                    Json(&error_response),
                )
                    .into_response()
            },
            Self::Conflict(error_response) => {
                (
                    StatusCode::CONFLICT,
                    Json(&error_response),
                )
                    .into_response()
            },
            Self::Gone(error_response) => {
                (
                    StatusCode::GONE,
                    Json(&error_response),
                )
                    .into_response()
            },
            Self::InternalServerError(error_response) => {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(&error_response),
                )
                    .into_response()
            },
        }
    }
}


