//! XRPC errors: an HTTP status and a JSON body `{error, message}` with the
//! atproto error names the reference PDS uses.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use locus_auth::session::AuthError;
use locus_core::CoreError;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XrpcError {
    pub status: StatusCode,
    pub error: String,
    pub message: String,
}

impl XrpcError {
    pub fn new(status: StatusCode, error: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            error: error.to_owned(),
            message: message.into(),
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "InvalidRequest", message)
    }

    pub fn named(error: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, error, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalServerError",
            message,
        )
    }

    pub fn auth_required(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "AuthenticationRequired", message)
    }
}

impl IntoResponse for XrpcError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"error": self.error, "message": self.message})),
        )
            .into_response()
    }
}

impl From<CoreError> for XrpcError {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::Invalid(m) | CoreError::Conflict(m) => Self::invalid_request(m),
            CoreError::NotFound(m) => Self::invalid_request(m),
            other => {
                tracing::error!("{other}");
                Self::internal("Internal Server Error")
            }
        }
    }
}

impl From<AuthError> for XrpcError {
    fn from(e: AuthError) -> Self {
        match e {
            AuthError::InvalidToken => Self::named("InvalidToken", "Token could not be verified"),
            AuthError::ExpiredToken => Self::named("ExpiredToken", "Token has expired"),
            AuthError::RevokedToken => Self::named("ExpiredToken", "Token has been revoked"),
            AuthError::AccountNotFound => Self::new(
                StatusCode::FORBIDDEN,
                "AccountNotFound",
                "Account not found",
            ),
            AuthError::AccountTakedown => Self::new(
                StatusCode::UNAUTHORIZED,
                "AccountTakedown",
                "Account has been taken down",
            ),
            AuthError::Storage(m) => {
                tracing::error!("{m}");
                Self::internal("Internal Server Error")
            }
        }
    }
}

pub type XrpcResult<T> = Result<T, XrpcError>;
