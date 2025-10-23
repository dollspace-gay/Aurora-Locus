/// Unified error types for Aurora Locus PDS
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Main error type for the PDS
#[derive(Error, Debug)]
pub enum PdsError {
    /// Database errors
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    /// Authentication errors
    #[error("Authentication failed: {0}")]
    Authentication(String),

    /// Authorization errors
    #[error("Not authorized: {0}")]
    Authorization(String),

    /// Validation errors
    #[error("Validation error: {0}")]
    Validation(String),

    /// Repository errors
    #[error("Repository error: {0}")]
    Repository(String),

    /// Blob storage errors
    #[error("Blob storage error: {0}")]
    BlobStorage(String),

    /// DID resolution errors
    #[error("DID resolution error: {0}")]
    DidResolution(String),

    /// Identity resolution errors
    #[error("Identity resolution error: {0}")]
    IdentityResolution(String),

    /// Rate limiting errors
    #[error("Rate limit exceeded")]
    RateLimitExceeded { retry_after: std::time::Duration },

    /// Not found errors
    #[error("Not found: {0}")]
    NotFound(String),

    /// Conflict errors (e.g., duplicate account)
    #[error("Conflict: {0}")]
    Conflict(String),

    /// Internal server errors
    #[error("Internal error: {0}")]
    Internal(String),

    /// ATProto SDK errors
    #[error("ATProto error: {0}")]
    AtProto(String),

    /// IO errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JWT errors
    #[error("JWT error: {0}")]
    Jwt(String),

    /// Account taken down
    #[error("Account taken down: {0}")]
    AccountTakenDown(String),

    /// Account suspended
    #[error("Account suspended: {0}")]
    AccountSuspended(String),
}

/// XRPC error response format
#[derive(Debug, Serialize, Deserialize)]
pub struct XrpcErrorResponse {
    pub error: String,
    pub message: String,
}

/// Convert PdsError to HTTP response
impl IntoResponse for PdsError {
    fn into_response(self) -> Response {
        let (status, error_code, message) = match self {
            PdsError::Authentication(_) => (
                StatusCode::UNAUTHORIZED,
                "AuthenticationRequired",
                self.to_string(),
            ),
            PdsError::Authorization(_) => (
                StatusCode::FORBIDDEN,
                "Forbidden",
                self.to_string(),
            ),
            PdsError::Validation(_) => (
                StatusCode::BAD_REQUEST,
                "InvalidRequest",
                self.to_string(),
            ),
            PdsError::NotFound(_) => (
                StatusCode::NOT_FOUND,
                "NotFound",
                self.to_string(),
            ),
            PdsError::Conflict(_) => (
                StatusCode::CONFLICT,
                "Conflict",
                self.to_string(),
            ),
            PdsError::RateLimitExceeded { .. } => (
                StatusCode::TOO_MANY_REQUESTS,
                "RateLimitExceeded",
                "Rate limit exceeded".to_string(),
            ),
            PdsError::AccountTakenDown(_) => (
                StatusCode::FORBIDDEN,
                "AccountTakedown",
                self.to_string(),
            ),
            PdsError::AccountSuspended(_) => (
                StatusCode::FORBIDDEN,
                "AccountSuspended",
                self.to_string(),
            ),
            PdsError::Database(_) | PdsError::Internal(_) | PdsError::Io(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError",
                "Internal server error".to_string(), // Don't leak details
            ),
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError",
                self.to_string(),
            ),
        };

        let body = Json(XrpcErrorResponse {
            error: error_code.to_string(),
            message,
        });

        (status, body).into_response()
    }
}

/// Result type alias for PDS operations
pub type PdsResult<T> = Result<T, PdsError>;
