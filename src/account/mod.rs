//! Account management system
//!
//! Handles user account creation, authentication, sessions, and related operations.

mod manager;

pub use manager::{AccountManager, ConsumeResult};

use serde::{Deserialize, Serialize};

/// Account creation request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAccountRequest {
    pub handle: String,
    pub email: Option<String>,
    pub password: String,
    #[serde(default)]
    pub invite_code: Option<String>,
    /// Pre-created DID (if user already created their DID via PLC)
    #[serde(default)]
    pub did: Option<String>,
    /// Recovery key (did:key format) for the account
    #[serde(default)]
    pub recovery_key: Option<String>,
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
    /// The caller's operator role (`"moderator"` / `"admin"` / `"superadmin"`)
    /// when the DID holds a non-revoked `admin_roles` grant; absent for regular
    /// accounts. The admin UI reads this off `getSession` to resolve the live
    /// operator tier for its sidebar/route gating (#297) — the role is looked
    /// up per request (not token-baked), so a role change is reflected on the
    /// next `getSession` without re-login (§8.1.6). `#[serde(skip)]` when None
    /// keeps the standard atproto session shape unchanged for non-operators.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// Validated session from bearer token
#[derive(Debug, Clone)]
pub struct ValidatedSession {
    pub did: String,
    pub session_id: String,
    pub is_app_password: bool,
}

/// Identity resolved from a refresh token by side-effect-free validation.
///
/// Returned by [`AccountManager::validate_refresh_token`]. Carries only the
/// identity needed to act on the token's session — `used`/`next_id` are
/// deliberately omitted (Arc 4 design §3.1, round-1 M-3): `delete_session`
/// honors user intent and does not branch on grace-period state, and
/// `refresh_session` reads those fields from its own row-read.
#[derive(Debug, Clone)]
pub struct RefreshTokenIdentity {
    pub did: String,
    pub session_id: String,
    pub token_id: String,
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

#[cfg(test)]
mod session_info_tests {
    use super::*;

    // #297 — the admin UI reads `role` off the getSession response to resolve
    // the live operator tier. Pin the wire contract: present when the caller
    // is an operator, omitted (standard atproto session shape) otherwise.
    #[test]
    fn role_is_serialized_under_camelcase_role_key_when_present() {
        let s = SessionInfo {
            did: "did:plc:op".to_string(),
            handle: "op.localhost".to_string(),
            email: None,
            email_confirmed: Some(false),
            role: Some("superadmin".to_string()),
        };
        let v = serde_json::to_value(&s).expect("serialize");
        assert_eq!(v["role"], serde_json::json!("superadmin"));
    }

    #[test]
    fn role_is_omitted_for_non_operators() {
        let s = SessionInfo {
            did: "did:plc:user".to_string(),
            handle: "user.localhost".to_string(),
            email: None,
            email_confirmed: Some(false),
            role: None,
        };
        let v = serde_json::to_value(&s).expect("serialize");
        assert!(
            v.get("role").is_none(),
            "role must be omitted (not null) for regular accounts so the standard \
             session shape is unchanged",
        );
    }
}
