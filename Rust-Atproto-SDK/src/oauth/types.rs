//! OAuth types and data structures

use serde::{Deserialize, Serialize};
use crate::types::AtpSessionData;

/// OAuth error types
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("JSON parsing failed: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("OAuth server error: {error}, description: {description}")]
    ServerError {
        error: String,
        description: String,
    },

    #[error("Invalid client metadata: {0}")]
    InvalidMetadata(String),

    #[error("Invalid authorization response: {0}")]
    InvalidResponse(String),

    #[error("DPoP error: {0}")]
    DPopError(#[from] crate::oauth::dpop::DPopError),

    #[error("URL parsing error: {0}")]
    UrlError(#[from] url::ParseError),

    #[error("Missing required field: {0}")]
    MissingField(String),
}

/// OAuth session data
///
/// Contains the access token, refresh token, and user information
/// obtained through OAuth authorization code flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthSession {
    /// Access token (JWT, short-lived ~1 hour)
    pub access_token: String,

    /// Refresh token (long-lived, used to get new access tokens)
    pub refresh_token: Option<String>,

    /// Token type (always "DPoP" for ATProto)
    pub token_type: String,

    /// Token expiration in seconds
    pub expires_in: Option<i64>,

    /// User's DID (Decentralized Identifier)
    pub did: String,

    /// User's handle (e.g., "alice.bsky.social")
    pub handle: Option<String>,

    /// User's email (if available)
    pub email: Option<String>,

    /// Whether email is confirmed
    pub email_confirmed: Option<bool>,
}

impl OAuthSession {
    /// Convert OAuth session to AtpSessionData
    ///
    /// This allows OAuth sessions to be used with the existing Agent API.
    pub fn to_atp_session_data(&self) -> AtpSessionData {
        AtpSessionData {
            access_jwt: self.access_token.clone(),
            refresh_jwt: self.refresh_token.clone().unwrap_or_default(),
            handle: self.handle.clone().unwrap_or_default(),
            did: self.did.clone(),
            email: self.email.clone(),
            email_confirmed: self.email_confirmed,
            email_auth_factor: None,
            active: true,
            status: None,
        }
    }
}

/// Client metadata for OAuth
///
/// Must be hosted at a public URL and referenced by client_id.
/// See: https://www.rfc-editor.org/rfc/rfc7591.html
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientMetadata {
    /// Client identifier (must be a URL pointing to this metadata)
    pub client_id: String,

    /// Human-readable client name
    pub client_name: String,

    /// Client homepage URL
    pub client_uri: String,

    /// Logo URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo_uri: Option<String>,

    /// Terms of service URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tos_uri: Option<String>,

    /// Privacy policy URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_uri: Option<String>,

    /// Array of redirect URIs
    pub redirect_uris: Vec<String>,

    /// OAuth scopes (always ["atproto"] for ATProto)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,

    /// Grant types supported
    pub grant_types: Vec<String>,

    /// Response types supported
    pub response_types: Vec<String>,

    /// Token endpoint authentication method
    pub token_endpoint_auth_method: String,

    /// Application type ("web" or "native")
    pub application_type: String,

    /// Whether to use DPoP-bound access tokens
    pub dpop_bound_access_tokens: bool,
}

impl ClientMetadata {
    /// Create default client metadata
    ///
    /// Creates a web application with DPoP support for ATProto.
    ///
    /// ## Arguments
    ///
    /// * `client_id` - URL where this metadata will be hosted
    /// * `redirect_uri` - Callback URL for OAuth flow
    /// * `client_name` - Human-readable application name
    pub fn new(client_id: String, redirect_uri: String, client_name: String) -> Self {
        Self {
            client_id: client_id.clone(),
            client_name,
            client_uri: client_id.clone(),
            logo_uri: None,
            tos_uri: None,
            policy_uri: None,
            redirect_uris: vec![redirect_uri],
            scope: Some("atproto".to_string()),
            grant_types: vec!["authorization_code".to_string(), "refresh_token".to_string()],
            response_types: vec!["code".to_string()],
            token_endpoint_auth_method: "none".to_string(),
            application_type: "web".to_string(),
            dpop_bound_access_tokens: true,
        }
    }
}

/// Token response from authorization server
///
/// For ATProto OAuth, the token response has these required fields per spec:
/// - `access_token`: The access token (JWT)
/// - `token_type`: Must be "DPoP" for ATProto
/// - `sub`: User's DID (required by ATProto spec)
/// - `scope`: Granted scope (required, should contain "atproto")
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    /// Access token
    pub access_token: String,

    /// Token type (should be "DPoP" for ATProto)
    pub token_type: String,

    /// User's DID (required for ATProto OAuth)
    pub sub: String,

    /// Scope granted (required, typically "atproto")
    pub scope: String,

    /// Token expiration in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<i64>,

    /// Refresh token (for getting new access tokens)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}


























/// OAuth error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthErrorResponse {
    /// Error code
    pub error: String,

    /// Error description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_description: Option<String>,

    /// Error URI (link to more info)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_uri: Option<String>,
}

/// Authorization server metadata (discovered from .well-known/oauth-authorization-server)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationServerMetadata {
    /// Issuer identifier
    pub issuer: String,

    /// Authorization endpoint URL
    pub authorization_endpoint: String,

    /// Token endpoint URL
    pub token_endpoint: String,

    /// Supported grant types
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_types_supported: Option<Vec<String>>,

    /// Supported response types
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_types_supported: Option<Vec<String>>,

    /// Supported scopes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes_supported: Option<Vec<String>>,

    /// Supported token endpoint auth methods
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_endpoint_auth_methods_supported: Option<Vec<String>>,

    /// Whether DPoP is supported
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpop_signing_alg_values_supported: Option<Vec<String>>,

    /// Supported code challenge methods (should include "S256")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_challenge_methods_supported: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_metadata_creation() {
        let metadata = ClientMetadata::new(
            "https://example.com/client-metadata.json".to_string(),
            "https://example.com/callback".to_string(),
            "Test App".to_string(),
        );

        assert_eq!(metadata.client_id, "https://example.com/client-metadata.json");
        assert_eq!(metadata.client_name, "Test App");
        assert_eq!(metadata.redirect_uris[0], "https://example.com/callback");
        assert!(metadata.dpop_bound_access_tokens);
        assert_eq!(metadata.token_endpoint_auth_method, "none");
    }

    #[test]
    fn test_oauth_session_to_atp_session() {
        let oauth_session = OAuthSession {
            access_token: "access123".to_string(),
            refresh_token: Some("refresh456".to_string()),
            token_type: "DPoP".to_string(),
            expires_in: Some(3600),
            did: "did:plc:test123".to_string(),
            handle: Some("alice.bsky.social".to_string()),
            email: Some("alice@example.com".to_string()),
            email_confirmed: Some(true),
        };

        let atp_session = oauth_session.to_atp_session_data();

        assert_eq!(atp_session.access_jwt, "access123");
        assert_eq!(atp_session.refresh_jwt, "refresh456");
        assert_eq!(atp_session.did, "did:plc:test123");
        assert_eq!(atp_session.handle, "alice.bsky.social");
        assert_eq!(atp_session.email, Some("alice@example.com".to_string()));
        assert_eq!(atp_session.email_confirmed, Some(true));
    }
}
