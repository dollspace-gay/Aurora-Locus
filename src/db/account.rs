/// Account database models and operations
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Account record in the database
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Account {
    pub did: String,
    pub handle: String,
    pub email: Option<String>,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
    pub email_confirmed: bool,
    pub email_confirmed_at: Option<DateTime<Utc>>,
    pub deactivated_at: Option<DateTime<Utc>>,
    pub taken_down: bool,
    /// PLC rotation key (private key, hex-encoded, 32 bytes)
    pub plc_rotation_key: Option<String>,
    /// PLC rotation key public (compressed public key, hex-encoded, 33 bytes)
    pub plc_rotation_key_public: Option<String>,
    /// Last PLC operation CID
    pub plc_last_operation_cid: Option<String>,
}

/// Session record in the database
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub did: String,
    pub access_token: String,
    pub refresh_token: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub app_password_name: Option<String>,
}

/// Refresh token record
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RefreshToken {
    pub id: String,
    pub did: String,
    pub token: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub used: bool,
    pub used_at: Option<DateTime<Utc>>,
}

/// Email confirmation token
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct EmailToken {
    pub token: String,
    pub did: String,
    pub purpose: String, // "confirm_email" or "reset_password"
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub used: bool,
}

/// Invite code record
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct InviteCode {
    pub code: String,
    pub available_uses: i32,
    pub disabled: bool,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub created_for: Option<String>,
}

/// Invite code usage record
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct InviteCodeUse {
    pub code: String,
    pub used_by: String,
    pub used_at: DateTime<Utc>,
}

/// App password record (for OAuth/third-party apps)
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AppPassword {
    pub did: String,
    pub name: String,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
    pub privileged: bool,
}
