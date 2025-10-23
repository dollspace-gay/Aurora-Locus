/// Account management system
///
/// Handles user account creation, authentication, sessions, and related operations.

mod manager;

pub use manager::AccountManager;

use serde::{Deserialize, Serialize};

/// Account creation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAccountRequest {
    pub handle: String,
    pub email: Option<String>,
    pub password: String,
    pub invite_code: Option<String>,
}

/// Account creation response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAccountResponse {
    pub did: String,
    pub handle: String,
    pub access_jwt: String,
    pub refresh_jwt: String,
}

/// Login request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub identifier: String, // handle or email
    pub password: String,
}

/// Session response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResponse {
    pub did: String,
    pub handle: String,
    pub access_jwt: String,
    pub refresh_jwt: String,
    pub email: Option<String>,
    pub email_confirmed: Option<bool>,
}

/// Session info (for getSession)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub did: String,
    pub handle: String,
    pub email: Option<String>,
    pub email_confirmed: Option<bool>,
}

/// Token refresh request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshSessionRequest {
    pub refresh_jwt: String,
}

/// Validated session from bearer token
#[derive(Debug, Clone)]
pub struct ValidatedSession {
    pub did: String,
    pub session_id: String,
    pub is_app_password: bool,
}

/// App password info (without the actual password)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPasswordInfo {
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub privileged: bool,
}

/// Create app password request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAppPasswordRequest {
    pub name: String,
    pub privileged: Option<bool>,
}

/// Create app password response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAppPasswordResponse {
    pub app_password: String,
}

/// List app passwords response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAppPasswordsResponse {
    pub passwords: Vec<AppPasswordInfo>,
}

/// Revoke app password request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokeAppPasswordRequest {
    pub name: String,
}
