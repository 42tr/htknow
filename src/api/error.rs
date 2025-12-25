use std::{fmt, result};

use axum::{
    Json, http::StatusCode, response::{IntoResponse, Response}
};
use serde::Serialize;
use sqlx;

/// Unified API error type for handlers to return.
///
/// Example:
/// - `Result<T, ApiError>` is aliased as `ApiResult<T>` below.
/// - Convert database errors with `ApiError::from(sqlx::Error)`.
#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    Unauthorized(String),
    NotFound(String),
    Database(sqlx::Error),
    Internal(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::BadRequest(msg) => write!(f, "BadRequest: {}", msg),
            ApiError::Unauthorized(msg) => write!(f, "Unauthorized: {}", msg),
            ApiError::NotFound(msg) => write!(f, "NotFound: {}", msg),
            ApiError::Database(e) => write!(f, "Database error: {}", e),
            ApiError::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        ApiError::Database(e)
    }
}

/// Standardized JSON body returned for errors.
#[derive(Serialize)]
struct ErrorBody {
    code: u16,
    error: String,
    // optional: you can add `details: Option<String>` if you want more info
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Map each error variant to an HTTP status code and message.
        let (status, message) = match &self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            ApiError::Database(e) => {
                // Log detailed error server-side but avoid leaking internals to clients.
                log::error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".into(),
                )
            }
            ApiError::Internal(msg) => {
                log::error!("Internal error: {}", msg);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".into(),
                )
            }
        };

        let body = ErrorBody { code: status.as_u16(), error: message };

        (status, Json(body)).into_response()
    }
}

/// Convenience alias for handlers' return type.
pub type ApiResult<T> = result::Result<T, ApiError>;

impl ApiError {
    /// Helper to create a `BadRequest` error.
    pub fn bad_request<M: Into<String>>(msg: M) -> Self {
        ApiError::BadRequest(msg.into())
    }

    /// Helper to create a `NotFound` error.
    pub fn not_found<M: Into<String>>(msg: M) -> Self {
        ApiError::NotFound(msg.into())
    }

    /// Helper to create an `Unauthorized` error.
    pub fn unauthorized<M: Into<String>>(msg: M) -> Self {
        ApiError::Unauthorized(msg.into())
    }

    /// Helper to create a generic internal error (also logs).
    pub fn internal<M: Into<String>>(msg: M) -> Self {
        let msg = msg.into();
        log::error!("Internal error created: {}", msg);
        ApiError::Internal(msg)
    }
}
