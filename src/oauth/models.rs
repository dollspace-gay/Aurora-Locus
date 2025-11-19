//! OAuth Data Models

// Allow dead_code - public APIs defined for future feature completion
#![allow(dead_code)]
//!
//! Defines the core data structures for OAuth 2.1 implementation

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Device information for multi-device OAuth support
///
/// Tracks client devices with DPoP key binding for secure token management.
/// Each device represents a unique client (browser, mobile app, etc.) that
/// can maintain its own OAuth session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    /// Unique device identifier (UUID)
    pub id: String,

    /// Session/token ID this device is associated with
    pub session_id: String,

    /// User agent string for device identification
    pub user_agent: Option<String>,

    /// IP address from which device was registered
    pub ip_address: Option<String>,

    /// Last time this device was active
    pub last_seen_at: DateTime<Utc>,

    /// DPoP public key (JWK format) for token binding
    /// This binds access tokens to this specific device via cryptographic proof
    pub dpop_public_key: Option<String>,

    /// When device was created
    pub created_at: DateTime<Utc>,
}

/// Device data for creation/update operations
///
/// A lightweight version of Device used for database operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceData {
    /// Session ID this device belongs to
    pub session_id: String,

    /// User agent string
    pub user_agent: Option<String>,

    /// IP address
    pub ip_address: Option<String>,

    /// Last activity timestamp
    pub last_seen_at: DateTime<Utc>,

    /// DPoP public key (optional, for DPoP-bound tokens)
    pub dpop_public_key: Option<String>,
}

/// Account-Device association
///
/// Maps accounts to their authorized devices for device management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountDevice {
    /// Auto-increment primary key
    pub id: i64,

    /// Account DID
    pub did: String,

    /// Device identifier
    pub device_id: String,

    /// When device was authorized for this account
    pub authorized_at: DateTime<Utc>,

    /// When device was last used
    pub last_used_at: Option<DateTime<Utc>>,

    /// User-defined device nickname
    pub device_name: Option<String>,

    /// Is this device currently active?
    pub is_active: bool,

    /// When device was revoked (if applicable)
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Device list response for API endpoints
#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceListResponse {
    /// List of devices
    pub devices: Vec<DeviceInfo>,

    /// Cursor for pagination
    pub cursor: Option<String>,
}

/// Device information for public API
#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Device identifier
    pub id: String,

    /// User-defined device name
    pub name: Option<String>,

    /// Device type inferred from user agent
    pub device_type: String,

    /// Browser/app name
    pub browser: Option<String>,

    /// Operating system
    pub os: Option<String>,

    /// When device was last active
    pub last_seen_at: DateTime<Utc>,

    /// When device was authorized
    pub authorized_at: DateTime<Utc>,

    /// Is this the current device?
    pub is_current: bool,
}

/// OAuth Authorization Request
///
/// Stores the OAuth 2.1 authorization request state during the authorization code flow.
/// Tracks PKCE challenge, requested scopes, and client information.
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

/// Authorization request creation data
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

/// Authorization endpoint query parameters
#[derive(Debug, Deserialize)]
pub struct AuthorizeQuery {
    /// Response type (must be 'code' for authorization code flow)
    pub response_type: String,

    /// OAuth client identifier
    pub client_id: String,

    /// Redirect URI after authorization
    pub redirect_uri: String,

    /// Requested OAuth scopes (space-separated)
    pub scope: String,

    /// PKCE code challenge
    pub code_challenge: String,

    /// PKCE challenge method (should be 'S256')
    pub code_challenge_method: String,

    /// CSRF protection state parameter
    pub state: Option<String>,
}

/// Token endpoint request body
#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    /// Grant type (authorization_code or refresh_token)
    pub grant_type: String,

    /// Authorization code (for grant_type=authorization_code)
    pub code: Option<String>,

    /// PKCE code verifier (for grant_type=authorization_code)
    pub code_verifier: Option<String>,

    /// Client ID
    pub client_id: String,

    /// Redirect URI (must match the one used in authorization request)
    pub redirect_uri: Option<String>,

    /// Refresh token (for grant_type=refresh_token)
    pub refresh_token: Option<String>,
}

/// Token endpoint response
#[derive(Debug, Serialize, Deserialize)]
pub struct TokenResponse {
    /// Access token
    pub access_token: String,

    /// Token type (always "DPoP" for DPoP-bound tokens)
    pub token_type: String,

    /// Access token expiration in seconds
    pub expires_in: i64,

    /// Refresh token for getting new access tokens
    pub refresh_token: String,

    /// Granted scopes (space-separated)
    pub scope: String,
}

/// OAuth Client Configuration
///
/// Defines a registered OAuth client (application) that can request authorization.
/// For Phase 1, clients are statically configured. Future phases may support
/// dynamic client registration per RFC 7591.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthClient {
    /// Client identifier (URL to client metadata)
    pub client_id: String,

    /// Human-readable client name
    pub client_name: String,

    /// Whitelisted redirect URIs for this client
    pub redirect_uris: Vec<String>,

    /// Default scopes granted to this client
    pub default_scopes: Vec<String>,

    /// Client logo URL (optional)
    pub logo_uri: Option<String>,

    /// Client policy/terms URL (optional)
    pub policy_uri: Option<String>,

    /// Is this a trusted first-party client?
    pub is_trusted: bool,
}

/// Authorized Client (database record)
///
/// Tracks which OAuth clients have been authorized by users.
/// Enables "remember this device" and client revocation functionality.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizedClient {
    /// Auto-increment primary key
    pub id: i64,

    /// Account DID
    pub did: String,

    /// OAuth client ID
    pub client_id: String,

    /// Granted scopes (space-separated)
    pub scope: String,

    /// When client was first authorized
    pub first_authorized_at: DateTime<Utc>,

    /// When client was last used
    pub last_used_at: Option<DateTime<Utc>>,

    /// Is authorization still active?
    pub is_active: bool,

    /// When authorization was revoked
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Authorized client information for public API
#[derive(Debug, Serialize, Deserialize)]
pub struct AuthorizedClientInfo {
    /// Client ID
    pub client_id: String,

    /// Client name
    pub client_name: String,

    /// Client logo URL
    pub logo_uri: Option<String>,

    /// Granted scopes
    pub scopes: Vec<String>,

    /// When first authorized
    pub first_authorized_at: DateTime<Utc>,

    /// When last used
    pub last_used_at: Option<DateTime<Utc>>,
}

/// Client list response for API endpoints
#[derive(Debug, Serialize, Deserialize)]
pub struct ClientListResponse {
    /// List of authorized clients
    pub clients: Vec<AuthorizedClientInfo>,

    /// Cursor for pagination
    pub cursor: Option<String>,
}
