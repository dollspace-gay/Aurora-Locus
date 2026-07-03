//! OAuth data models (shared surface).
//!
//! Post-Phase-ζ this holds only the two types the LIVE
//! [`crate::oauth::flow_state_adapter::OAuthFlowStateAdapter`] consumes — the
//! authorization-request row + its creation data for the distributed
//! OAuth-flow-state substrate. The device / client / token-request / authorize-
//! query models were retired with the legacy `/oauth/*` provider in Phase ζ.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// An OAuth authorization-request row.
///
/// Tracks the PKCE challenge, requested scopes, and client information for the
/// distributed OAuth-flow-state adapter (the `authorization_request` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationRequest {
    /// Auto-increment primary key
    pub id: i64,

    /// Unique request identifier (UUID)
    pub request_id: String,

    /// Account DID being authorized
    pub did: String,

    /// OAuth client ID
    pub client_id: String,

    /// PKCE code challenge (SHA-256 hash of verifier)
    pub code_challenge: String,

    /// PKCE challenge method (always 'S256' for OAuth 2.1)
    pub code_challenge_method: String,

    /// Authorization code (generated after user consent)
    pub authorization_code: Option<String>,

    /// Requested OAuth scopes (space-separated)
    pub scope: String,

    /// Client redirect URI
    pub redirect_uri: String,

    /// State parameter for CSRF protection
    pub state: Option<String>,

    /// When request was created
    pub created_at: DateTime<Utc>,

    /// When authorization code expires (typically 10 minutes)
    pub expires_at: DateTime<Utc>,

    /// Has the authorization code been used?
    pub code_used: bool,

    /// When code was consumed
    pub code_used_at: Option<DateTime<Utc>>,
}

/// Authorization request creation data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationRequestData {
    /// Account DID
    pub did: String,

    /// OAuth client ID
    pub client_id: String,

    /// PKCE code challenge
    pub code_challenge: String,

    /// PKCE challenge method
    pub code_challenge_method: String,

    /// Requested scopes
    pub scope: String,

    /// Redirect URI
    pub redirect_uri: String,

    /// CSRF protection state
    pub state: Option<String>,
}
