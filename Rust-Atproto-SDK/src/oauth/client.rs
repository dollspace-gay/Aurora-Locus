//! OAuth client implementation
//!
//! Handles the complete OAuth 2.0 authorization code flow with PKCE and DPoP.

use super::dpop::DPopManager;
use super::pkce::PkceParams;
use super::state::StateManager;
use super::types::{
    AuthorizationServerMetadata, ClientMetadata, OAuthError, OAuthErrorResponse, OAuthSession,
    TokenResponse,
};
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

/// OAuth client for ATProto
///
/// Manages the complete OAuth authorization code flow with PKCE and DPoP support.
///
/// ## Flow
///
/// 1. Create client with `OAuthClient::new()`
/// 2. Generate PKCE parameters with `PkceParams::generate()`
/// 3. Build authorization URL with `build_authorization_url()`
/// 4. Redirect user to authorization URL
/// 5. Handle callback and extract authorization code
/// 6. Exchange code for tokens with `exchange_code()`
/// 7. Use tokens to make authenticated requests
/// 8. Refresh tokens when they expire with `refresh_token()`
pub struct OAuthClient {
    /// Client metadata
    metadata: ClientMetadata,

    /// HTTP client for API requests
    http_client: Client,

    /// DPoP manager for generating proof-of-possession tokens
    dpop: Arc<DPopManager>,

    /// State manager for CSRF protection
    state_manager: Arc<StateManager>,
}

impl OAuthClient {
    /// Create a new OAuth client
    ///
    /// ## Arguments
    ///
    /// * `client_id` - URL where client metadata is hosted
    /// * `redirect_uri` - Callback URL for OAuth flow
    ///
    /// ## Example
    ///
    /// ```
    /// use atproto::oauth::OAuthClient;
    ///
    /// let client = OAuthClient::new(
    ///     "https://example.com/client-metadata.json".to_string(),
    ///     "https://example.com/callback".to_string(),
    /// );
    /// ```
    pub fn new(client_id: String, redirect_uri: String) -> Result<Self, OAuthError> {
        let metadata = ClientMetadata::new(
            client_id.clone(),
            redirect_uri,
            "ATProto Rust Client".to_string(),
        );

        let http_client = Client::builder()
            .user_agent("atproto-rust-sdk/0.1.0")
            .build()?;

        let dpop = Arc::new(DPopManager::new()?);
        let state_manager = Arc::new(StateManager::new());

        Ok(Self {
            metadata,
            http_client,
            dpop,
            state_manager,
        })
    }

    /// Discover authorization server metadata from a PDS
    ///
    /// Fetches the OAuth authorization server configuration from the
    /// .well-known/oauth-authorization-server endpoint.
    ///
    /// ## Arguments
    ///
    /// * `pds_url` - The PDS URL (e.g., "https://bsky.social")
    ///
    /// ## Returns
    ///
    /// Authorization server metadata including token and authorization endpoints
    pub async fn discover_server_metadata(
        &self,
        pds_url: &str,
    ) -> Result<AuthorizationServerMetadata, OAuthError> {
        let discovery_url = format!("{}/.well-known/oauth-authorization-server", pds_url);

        let response = self.http_client.get(&discovery_url).send().await?;

        if !response.status().is_success() {
            return Err(OAuthError::InvalidMetadata(format!(
                "Failed to discover server metadata: {}",
                response.status()
            )));
        }

        let metadata: AuthorizationServerMetadata = response.json().await?;
        Ok(metadata)
    }

    /// Resolve a handle to find the user's PDS
    ///
    /// Uses handle resolution to find the user's PDS URL, which is needed
    /// to discover the authorization server.
    ///
    /// ## Arguments
    ///
    /// * `handle` - User's handle (e.g., "alice.bsky.social")
    /// * `fallback_pds` - Fallback PDS if handle resolution fails
    ///
    /// ## Returns
    ///
    /// The PDS URL for the user
    async fn resolve_pds(
        &self,
        _handle: &str,
        fallback_pds: &str,
    ) -> Result<String, OAuthError> {
        // For now, just use the fallback PDS
        // TODO: Implement full handle resolution and DID document lookup
        // to discover the user's PDS endpoint
        Ok(fallback_pds.to_string())
    }

    /// Build an authorization URL for the OAuth flow
    ///
    /// Creates the authorization URL that the user should be redirected to
    /// in order to grant authorization.
    ///
    /// ## Arguments
    ///
    /// * `pds_url` - PDS URL (e.g., "https://bsky.social")
    /// * `handle` - User's handle (for handle resolution)
    /// * `pkce` - PKCE parameters (generated with `PkceParams::generate()`)
    ///
    /// ## Returns
    ///
    /// The authorization URL to redirect the user to
    ///
    /// ## Example
    ///
    /// ```no_run
    /// use atproto::oauth::{OAuthClient, PkceParams};
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let client = OAuthClient::new(
    ///         "https://example.com/client-metadata.json".to_string(),
    ///         "https://example.com/callback".to_string(),
    ///     )?;
    ///
    ///     let pkce = PkceParams::generate();
    ///     let url = client.build_authorization_url(
    ///         "https://bsky.social",
    ///         "alice.bsky.social",
    ///         &pkce,
    ///     ).await?;
    ///
    ///     println!("Redirect user to: {}", url);
    ///     Ok(())
    /// }
    /// ```
    pub async fn build_authorization_url(
        &self,
        pds_url: &str,
        handle: &str,
        pkce: &PkceParams,
    ) -> Result<String, OAuthError> {
        self.build_authorization_url_with_metadata(pds_url, handle, pkce, None)
            .await
    }

    /// Build an authorization URL with optional metadata
    ///
    /// Like `build_authorization_url`, but allows attaching metadata to the state
    /// parameter that can be retrieved later when handling the callback.
    ///
    /// ## Arguments
    ///
    /// * `pds_url` - PDS URL (e.g., "https://bsky.social")
    /// * `handle` - User's handle (for handle resolution)
    /// * `pkce` - PKCE parameters
    /// * `metadata` - Optional JSON metadata to attach to the state
    ///
    /// ## Returns
    ///
    /// The authorization URL to redirect the user to
    pub async fn build_authorization_url_with_metadata(
        &self,
        pds_url: &str,
        handle: &str,
        pkce: &PkceParams,
        metadata: Option<serde_json::Value>,
    ) -> Result<String, OAuthError> {
        // Resolve PDS for the handle
        let resolved_pds = self.resolve_pds(handle, pds_url).await?;

        // Discover authorization server metadata
        let server_metadata = self.discover_server_metadata(&resolved_pds).await?;

        // Generate and store state parameter for CSRF protection
        let state = self.state_manager.generate_state(metadata);

        // Build authorization URL
        let mut url = Url::parse(&server_metadata.authorization_endpoint)?;

        url.query_pairs_mut()
            .append_pair("client_id", &self.metadata.client_id)
            .append_pair("redirect_uri", &self.metadata.redirect_uris[0])
            .append_pair("response_type", "code")
            .append_pair("scope", "atproto")
            .append_pair("state", &state.value)
            .append_pair("code_challenge", &pkce.code_challenge)
            .append_pair("code_challenge_method", &pkce.code_challenge_method);

        Ok(url.to_string())
    }

    /// Exchange an authorization code for tokens
    ///
    /// Exchanges the authorization code received in the OAuth callback
    /// for access and refresh tokens.
    ///
    /// ## Arguments
    ///
    /// * `code` - Authorization code from callback
    /// * `code_verifier` - The code verifier from PKCE (must match the challenge)
    /// * `token_endpoint` - Token endpoint URL from server metadata
    ///
    /// ## Returns
    ///
    /// An OAuth session with access and refresh tokens
    ///
    /// ## Example
    ///
    /// ```no_run
    /// # use atproto::oauth::{OAuthClient, PkceParams};
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let client = OAuthClient::new(
    /// #     "https://example.com/client-metadata.json".to_string(),
    /// #     "https://example.com/callback".to_string(),
    /// # )?;
    /// # let pkce = PkceParams::generate();
    /// // After user authorizes and you receive the callback with code...
    /// let session = client.exchange_code(
    ///     "authorization_code_from_callback",
    ///     &pkce.code_verifier,
    ///     "https://bsky.social/oauth/token",
    /// ).await?;
    ///
    /// println!("Authenticated as: {}", session.did);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn exchange_code(
        &self,
        code: &str,
        code_verifier: &str,
        token_endpoint: &str,
    ) -> Result<OAuthSession, OAuthError> {
        // Generate DPoP proof for token endpoint
        let dpop_proof = self.dpop.generate_proof("POST", token_endpoint)?;

        // Build request body
        let mut params = HashMap::new();
        params.insert("grant_type", "authorization_code");
        params.insert("code", code);
        params.insert("code_verifier", code_verifier);
        params.insert("client_id", &self.metadata.client_id);
        params.insert("redirect_uri", &self.metadata.redirect_uris[0]);

        // Make token request with DPoP header
        let response = self
            .http_client
            .post(token_endpoint)
            .header("DPoP", dpop_proof)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&params)
            .send()
            .await?;

        // Handle error responses
        if !response.status().is_success() {
            let error_response: OAuthErrorResponse = response.json().await?;
            return Err(OAuthError::ServerError {
                error: error_response.error,
                description: error_response
                    .error_description
                    .unwrap_or_else(|| "No description provided".to_string()),
            });
        }

        // Parse successful response
        let token_response: TokenResponse = response.json().await?;

        // Build OAuth session (sub is now a required field)
        Ok(OAuthSession {
            access_token: token_response.access_token,
            refresh_token: token_response.refresh_token,
            token_type: token_response.token_type,
            expires_in: token_response.expires_in,
            did: token_response.sub,
            handle: None, // Will be populated by profile fetch
            email: None,
            email_confirmed: None,
        })
    }

    /// Refresh an access token using a refresh token
    ///
    /// Obtains a new access token using the refresh token.
    ///
    /// ## Arguments
    ///
    /// * `refresh_token` - The refresh token
    /// * `token_endpoint` - Token endpoint URL from server metadata
    ///
    /// ## Returns
    ///
    /// A new OAuth session with updated tokens
    pub async fn refresh_token(
        &self,
        refresh_token: &str,
        token_endpoint: &str,
    ) -> Result<OAuthSession, OAuthError> {
        // Generate DPoP proof for token endpoint
        let dpop_proof = self.dpop.generate_proof("POST", token_endpoint)?;

        // Build request body
        let mut params = HashMap::new();
        params.insert("grant_type", "refresh_token");
        params.insert("refresh_token", refresh_token);
        params.insert("client_id", &self.metadata.client_id);

        // Make token request with DPoP header
        let response = self
            .http_client
            .post(token_endpoint)
            .header("DPoP", dpop_proof)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&params)
            .send()
            .await?;

        // Handle error responses
        if !response.status().is_success() {
            let error_response: OAuthErrorResponse = response.json().await?;
            return Err(OAuthError::ServerError {
                error: error_response.error,
                description: error_response
                    .error_description
                    .unwrap_or_else(|| "No description provided".to_string()),
            });
        }

        // Parse successful response
        let token_response: TokenResponse = response.json().await?;

        // Build OAuth session (sub is now a required field)
        Ok(OAuthSession {
            access_token: token_response.access_token,
            refresh_token: token_response.refresh_token,
            token_type: token_response.token_type,
            expires_in: token_response.expires_in,
            did: token_response.sub,
            handle: None,
            email: None,
            email_confirmed: None,
        })
    }

    /// Validate a state parameter from OAuth callback
    ///
    /// Verifies that the state exists and is not expired. The state is
    /// consumed after validation (can only be used once).
    ///
    /// ## Arguments
    ///
    /// * `state` - The state parameter from the callback URL
    ///
    /// ## Returns
    ///
    /// `true` if the state is valid
    pub fn validate_state(&self, state: &str) -> bool {
        self.state_manager.validate_state(state)
    }

    /// Validate state and retrieve associated metadata
    ///
    /// Like `validate_state`, but also returns any metadata that was
    /// attached when the state was generated.
    ///
    /// ## Arguments
    ///
    /// * `state` - The state parameter from the callback URL
    ///
    /// ## Returns
    ///
    /// The metadata if the state is valid, `None` otherwise
    pub fn validate_state_with_metadata(&self, state: &str) -> Option<serde_json::Value> {
        self.state_manager.validate_and_get_metadata(state)
    }

    /// Handle OAuth callback and exchange code for tokens
    ///
    /// Convenience method that combines callback parsing, state validation,
    /// and code exchange into a single call.
    ///
    /// ## Arguments
    ///
    /// * `callback_url` - The full callback URL with query parameters
    /// * `code_verifier` - The code verifier from PKCE
    /// * `token_endpoint` - Token endpoint URL from server metadata
    ///
    /// ## Returns
    ///
    /// An OAuth session with tokens and user information
    ///
    /// ## Example
    ///
    /// ```no_run
    /// # use atproto::oauth::{OAuthClient, PkceParams};
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let client = OAuthClient::new(
    /// #     "https://example.com/client-metadata.json".to_string(),
    /// #     "https://example.com/callback".to_string(),
    /// # )?;
    /// # let pkce = PkceParams::generate();
    /// // After user is redirected back to your callback URL...
    /// let callback_url = "https://example.com/callback?code=abc&state=xyz";
    /// let session = client.handle_callback(
    ///     callback_url,
    ///     &pkce.code_verifier,
    ///     "https://bsky.social/oauth/token",
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn handle_callback(
        &self,
        callback_url: &str,
        code_verifier: &str,
        token_endpoint: &str,
    ) -> Result<OAuthSession, OAuthError> {
        use super::callback::{CallbackParser, CallbackResult};

        // Parse callback URL
        let parser = CallbackParser::new(self.metadata.redirect_uris[0].clone());
        let result = parser.parse(callback_url)?;

        match result {
            CallbackResult::Success { code, state } => {
                // Validate state for CSRF protection
                if !self.validate_state(&state) {
                    return Err(OAuthError::InvalidResponse(
                        "Invalid or expired state parameter".to_string(),
                    ));
                }

                // Exchange code for tokens
                self.exchange_code(&code, code_verifier, token_endpoint)
                    .await
            }
            CallbackResult::Error {
                error,
                error_description,
                ..
            } => Err(OAuthError::ServerError {
                error,
                description: error_description.unwrap_or_else(|| "No description".to_string()),
            }),
        }
    }

    /// Get the client metadata
    pub fn get_metadata(&self) -> &ClientMetadata {
        &self.metadata
    }

    /// Get the DPoP manager
    pub fn get_dpop(&self) -> Arc<DPopManager> {
        Arc::clone(&self.dpop)
    }

    /// Get the state manager
    pub fn get_state_manager(&self) -> Arc<StateManager> {
        Arc::clone(&self.state_manager)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_client_creation() {
        let client = OAuthClient::new(
            "https://example.com/client-metadata.json".to_string(),
            "https://example.com/callback".to_string(),
        );

        assert!(client.is_ok());
        let client = client.unwrap();
        assert_eq!(
            client.get_metadata().client_id,
            "https://example.com/client-metadata.json"
        );
    }

    #[test]
    fn test_state_management() {
        let client = OAuthClient::new(
            "https://example.com/client-metadata.json".to_string(),
            "https://example.com/callback".to_string(),
        )
        .unwrap();

        // Generate state
        let state = client.get_state_manager().generate_state(None);

        // Should validate successfully once
        assert!(client.validate_state(&state.value));

        // Should fail second time (state consumed)
        assert!(!client.validate_state(&state.value));
    }
}
