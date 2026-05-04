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
    #[allow(dead_code)] // Future repository errors
    Repository(String),

    /// Blob storage errors
    #[error("Blob storage error: {0}")]
    BlobStorage(String),

    /// DID resolution errors
    #[error("DID resolution error: {0}")]
    #[allow(dead_code)] // Future DID resolution errors
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
    #[allow(dead_code)] // Future ATProto errors
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

    /// Sequencer leader is on a different instance — caller should retry
    /// (load balancer will route to the leader on retry). Mapped to HTTP
    /// 503 Service Unavailable. See chainlink #89 / docs/AURORA_DESIGN.md §5.4.1.
    #[error("Sequencer leader is on a different instance: {0}")]
    NotLeader(String),
}

/// Manual PartialEq implementation for PdsError
/// Note: Database and Io variants cannot be truly compared due to underlying types
impl PartialEq for PdsError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (PdsError::Authentication(a), PdsError::Authentication(b)) => a == b,
            (PdsError::Authorization(a), PdsError::Authorization(b)) => a == b,
            (PdsError::Validation(a), PdsError::Validation(b)) => a == b,
            (PdsError::Repository(a), PdsError::Repository(b)) => a == b,
            (PdsError::BlobStorage(a), PdsError::BlobStorage(b)) => a == b,
            (PdsError::DidResolution(a), PdsError::DidResolution(b)) => a == b,
            (PdsError::IdentityResolution(a), PdsError::IdentityResolution(b)) => a == b,
            (
                PdsError::RateLimitExceeded { retry_after: a },
                PdsError::RateLimitExceeded { retry_after: b },
            ) => a == b,
            (PdsError::NotFound(a), PdsError::NotFound(b)) => a == b,
            (PdsError::Conflict(a), PdsError::Conflict(b)) => a == b,
            (PdsError::Internal(a), PdsError::Internal(b)) => a == b,
            (PdsError::AtProto(a), PdsError::AtProto(b)) => a == b,
            (PdsError::Jwt(a), PdsError::Jwt(b)) => a == b,
            (PdsError::AccountTakenDown(a), PdsError::AccountTakenDown(b)) => a == b,
            (PdsError::AccountSuspended(a), PdsError::AccountSuspended(b)) => a == b,
            (PdsError::NotLeader(a), PdsError::NotLeader(b)) => a == b,
            // Database and Io errors cannot be compared, so we use error message comparison
            (PdsError::Database(a), PdsError::Database(b)) => a.to_string() == b.to_string(),
            (PdsError::Io(a), PdsError::Io(b)) => a.to_string() == b.to_string(),
            _ => false,
        }
    }
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
            PdsError::Authorization(_) => (StatusCode::FORBIDDEN, "Forbidden", self.to_string()),
            PdsError::Validation(_) => {
                (StatusCode::BAD_REQUEST, "InvalidRequest", self.to_string())
            }
            PdsError::NotFound(_) => (StatusCode::NOT_FOUND, "NotFound", self.to_string()),
            PdsError::Conflict(_) => (StatusCode::CONFLICT, "Conflict", self.to_string()),
            PdsError::RateLimitExceeded { .. } => (
                StatusCode::TOO_MANY_REQUESTS,
                "RateLimitExceeded",
                "Rate limit exceeded".to_string(),
            ),
            PdsError::AccountTakenDown(_) => {
                (StatusCode::FORBIDDEN, "AccountTakedown", self.to_string())
            }
            PdsError::AccountSuspended(_) => {
                (StatusCode::FORBIDDEN, "AccountSuspended", self.to_string())
            }
            PdsError::NotLeader(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "NotLeader",
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
