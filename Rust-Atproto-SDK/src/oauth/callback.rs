//! OAuth callback URL parsing and validation
//!
//! Handles parsing and validating OAuth authorization callback URLs,
//! extracting authorization codes and error responses.

use url::Url;
use super::types::OAuthError;

/// OAuth callback result
///
/// Contains either a successful authorization code or an error.
#[derive(Debug, Clone)]
pub enum CallbackResult {
    /// Successful authorization with code and state
    Success {
        /// Authorization code to exchange for tokens
        code: String,

        /// State parameter for CSRF validation
        state: String,
    },

    /// Authorization error from server
    Error {
        /// Error code (e.g., "access_denied")
        error: String,

        /// Error description (if provided)
        error_description: Option<String>,

        /// Error URI (if provided)
        error_uri: Option<String>,

        /// State parameter (if provided)
        state: Option<String>,
    },
}

/// OAuth callback parser
///
/// Parses OAuth authorization callback URLs and extracts
/// authorization codes or error responses.
///
/// ## Example
///
/// ```
/// use atproto::oauth::callback::CallbackParser;
///
/// let parser = CallbackParser::new("https://example.com/callback".to_string());
///
/// // Parse successful callback
/// let callback_url = "https://example.com/callback?code=abc123&state=xyz789";
/// let result = parser.parse(callback_url).unwrap();
///
/// match result {
///     atproto::oauth::callback::CallbackResult::Success { code, state } => {
///         println!("Got code: {}, state: {}", code, state);
///     }
///     atproto::oauth::callback::CallbackResult::Error { error, .. } => {
///         eprintln!("Authorization failed: {}", error);
///     }
/// }
/// ```
pub struct CallbackParser {
    /// Expected redirect URI base (for validation)
    redirect_uri: String,
}

impl CallbackParser {
    /// Create a new callback parser
    ///
    /// ## Arguments
    ///
    /// * `redirect_uri` - The expected redirect URI base
    pub fn new(redirect_uri: String) -> Self {
        Self { redirect_uri }
    }

    /// Parse a callback URL
    ///
    /// Extracts the authorization code or error from the callback URL
    /// and validates that the URL matches the expected redirect URI.
    ///
    /// ## Arguments
    ///
    /// * `callback_url` - The full callback URL with query parameters
    ///
    /// ## Returns
    ///
    /// A `CallbackResult` containing either the authorization code or an error
    ///
    /// ## Errors
    ///
    /// Returns an error if:
    /// - The URL is malformed
    /// - The URL doesn't match the expected redirect URI
    /// - Required parameters are missing
    pub fn parse(&self, callback_url: &str) -> Result<CallbackResult, OAuthError> {
        let url = Url::parse(callback_url)?;

        // Validate that the callback URL matches our redirect URI
        self.validate_redirect_uri(&url)?;

        // Extract query parameters
        let params: std::collections::HashMap<String, String> = url
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        // Check for error response first
        if let Some(error) = params.get("error") {
            return Ok(CallbackResult::Error {
                error: error.clone(),
                error_description: params.get("error_description").cloned(),
                error_uri: params.get("error_uri").cloned(),
                state: params.get("state").cloned(),
            });
        }

        // Extract authorization code and state
        let code = params
            .get("code")
            .ok_or_else(|| {
                OAuthError::InvalidResponse("Missing 'code' parameter in callback".to_string())
            })?
            .clone();

        let state = params
            .get("state")
            .ok_or_else(|| {
                OAuthError::InvalidResponse("Missing 'state' parameter in callback".to_string())
            })?
            .clone();

        Ok(CallbackResult::Success { code, state })
    }

    /// Validate that the callback URL matches the expected redirect URI
    ///
    /// Checks that the scheme, host, port, and path match.
    fn validate_redirect_uri(&self, callback_url: &Url) -> Result<(), OAuthError> {
        let expected_url = Url::parse(&self.redirect_uri)?;

        // Check scheme
        if callback_url.scheme() != expected_url.scheme() {
            return Err(OAuthError::InvalidResponse(format!(
                "Redirect URI scheme mismatch: expected {}, got {}",
                expected_url.scheme(),
                callback_url.scheme()
            )));
        }

        // Check host
        if callback_url.host_str() != expected_url.host_str() {
            return Err(OAuthError::InvalidResponse(format!(
                "Redirect URI host mismatch: expected {:?}, got {:?}",
                expected_url.host_str(),
                callback_url.host_str()
            )));
        }

        // Check port
        if callback_url.port() != expected_url.port() {
            return Err(OAuthError::InvalidResponse(format!(
                "Redirect URI port mismatch: expected {:?}, got {:?}",
                expected_url.port(),
                callback_url.port()
            )));
        }

        // Check path
        if callback_url.path() != expected_url.path() {
            return Err(OAuthError::InvalidResponse(format!(
                "Redirect URI path mismatch: expected {}, got {}",
                expected_url.path(),
                callback_url.path()
            )));
        }

        Ok(())
    }

    /// Parse callback URL without strict redirect URI validation
    ///
    /// Useful for development or when the redirect URI might vary slightly.
    ///
    /// ## Arguments
    ///
    /// * `callback_url` - The callback URL to parse
    ///
    /// ## Returns
    ///
    /// A `CallbackResult` containing either the authorization code or an error
    pub fn parse_relaxed(callback_url: &str) -> Result<CallbackResult, OAuthError> {
        let url = Url::parse(callback_url)?;

        let params: std::collections::HashMap<String, String> = url
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        // Check for error response
        if let Some(error) = params.get("error") {
            return Ok(CallbackResult::Error {
                error: error.clone(),
                error_description: params.get("error_description").cloned(),
                error_uri: params.get("error_uri").cloned(),
                state: params.get("state").cloned(),
            });
        }

        // Extract code and state
        let code = params
            .get("code")
            .ok_or_else(|| {
                OAuthError::InvalidResponse("Missing 'code' parameter".to_string())
            })?
            .clone();

        let state = params
            .get("state")
            .ok_or_else(|| {
                OAuthError::InvalidResponse("Missing 'state' parameter".to_string())
            })?
            .clone();

        Ok(CallbackResult::Success { code, state })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_successful_callback() {
        let parser = CallbackParser::new("https://example.com/callback".to_string());

        let result = parser
            .parse("https://example.com/callback?code=auth_code_123&state=state_xyz")
            .unwrap();

        match result {
            CallbackResult::Success { code, state } => {
                assert_eq!(code, "auth_code_123");
                assert_eq!(state, "state_xyz");
            }
            CallbackResult::Error { .. } => panic!("Expected success result"),
        }
    }

    #[test]
    fn test_parse_error_callback() {
        let parser = CallbackParser::new("https://example.com/callback".to_string());

        let result = parser
            .parse("https://example.com/callback?error=access_denied&error_description=User%20denied&state=state_xyz")
            .unwrap();

        match result {
            CallbackResult::Error {
                error,
                error_description,
                state,
                ..
            } => {
                assert_eq!(error, "access_denied");
                assert_eq!(error_description, Some("User denied".to_string()));
                assert_eq!(state, Some("state_xyz".to_string()));
            }
            CallbackResult::Success { .. } => panic!("Expected error result"),
        }
    }

    #[test]
    fn test_parse_missing_code() {
        let parser = CallbackParser::new("https://example.com/callback".to_string());

        let result = parser.parse("https://example.com/callback?state=state_xyz");

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_missing_state() {
        let parser = CallbackParser::new("https://example.com/callback".to_string());

        let result = parser.parse("https://example.com/callback?code=auth_code_123");

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_scheme_mismatch() {
        let parser = CallbackParser::new("https://example.com/callback".to_string());

        let result = parser.parse("http://example.com/callback?code=abc&state=xyz");

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_host_mismatch() {
        let parser = CallbackParser::new("https://example.com/callback".to_string());

        let result = parser.parse("https://evil.com/callback?code=abc&state=xyz");

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_path_mismatch() {
        let parser = CallbackParser::new("https://example.com/callback".to_string());

        let result = parser.parse("https://example.com/wrong?code=abc&state=xyz");

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_port_mismatch() {
        let parser = CallbackParser::new("https://example.com:8080/callback".to_string());

        let result = parser.parse("https://example.com:9090/callback?code=abc&state=xyz");

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_relaxed() {
        // Relaxed parsing doesn't validate redirect URI
        let result = CallbackParser::parse_relaxed(
            "https://any-host.com/any-path?code=auth_code_123&state=state_xyz",
        )
        .unwrap();

        match result {
            CallbackResult::Success { code, state } => {
                assert_eq!(code, "auth_code_123");
                assert_eq!(state, "state_xyz");
            }
            CallbackResult::Error { .. } => panic!("Expected success result"),
        }
    }

    #[test]
    fn test_parse_relaxed_error() {
        let result = CallbackParser::parse_relaxed(
            "https://any-host.com/any-path?error=invalid_request&error_description=Bad%20request",
        )
        .unwrap();

        match result {
            CallbackResult::Error {
                error,
                error_description,
                ..
            } => {
                assert_eq!(error, "invalid_request");
                assert_eq!(error_description, Some("Bad request".to_string()));
            }
            CallbackResult::Success { .. } => panic!("Expected error result"),
        }
    }

    #[test]
    fn test_parse_with_additional_params() {
        let parser = CallbackParser::new("https://example.com/callback".to_string());

        // Should work even with additional query parameters
        let result = parser
            .parse("https://example.com/callback?code=abc&state=xyz&extra=param")
            .unwrap();

        match result {
            CallbackResult::Success { code, state } => {
                assert_eq!(code, "abc");
                assert_eq!(state, "xyz");
            }
            CallbackResult::Error { .. } => panic!("Expected success result"),
        }
    }
}
