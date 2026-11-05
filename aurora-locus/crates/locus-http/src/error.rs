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
    /// A `WWW-Authenticate` challenge (OAuth and DPoP errors).
    pub www_authenticate: Option<String>,
}

impl XrpcError {
    pub fn new(status: StatusCode, error: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            error: error.to_owned(),
            message: message.into(),
            www_authenticate: None,
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
        let mut res = (
            self.status,
            Json(json!({"error": self.error, "message": self.message})),
        )
            .into_response();
        if let Some(w) = &self.www_authenticate
            && let Ok(v) = axum::http::HeaderValue::from_str(w)
        {
            res.headers_mut()
                .insert(axum::http::header::WWW_AUTHENTICATE, v);
            res.headers_mut().append(
                axum::http::header::ACCESS_CONTROL_EXPOSE_HEADERS,
                axum::http::HeaderValue::from_static("WWW-Authenticate"),
            );
        }
        res
    }
}

impl From<locus_auth::oauth::OAuthError> for XrpcError {
    /// As the reference's resource server: the OAuth error code as the
    /// XRPC error name, with its status and challenge.
    fn from(e: locus_auth::oauth::OAuthError) -> Self {
        let e = e.for_resource();
        Self {
            status: StatusCode::from_u16(e.status).unwrap_or(StatusCode::UNAUTHORIZED),
            error: e.error,
            message: e.description,
            www_authenticate: e.www_authenticate,
        }
    }
}

impl From<locus_auth::scopes::ScopeMissing> for XrpcError {
    fn from(e: locus_auth::scopes::ScopeMissing) -> Self {
        Self::new(StatusCode::FORBIDDEN, "ScopeMissingError", e.to_string())
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
