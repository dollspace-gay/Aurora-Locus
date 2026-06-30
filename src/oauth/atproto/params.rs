//! Shared authorization-request parameter parsing + validation for the
//! atproto-OAuth provider (Arc 2 Phase β.3, chainlink #420 / LOCKED §3.2).
//!
//! The authorize endpoint (GET query) and the PAR endpoint (POST form) accept
//! the same OAuth + PKCE parameter set, so the raw shape and its validation
//! live here once. Validation enforces the atproto-OAuth profile: PKCE is
//! mandatory and S256-only, the only `response_type` is `code`, and the scope
//! string must parse against the closed [`super::scope`] vocabulary.

use serde::Deserialize;

use super::scope::{AtprotoScope, ScopeParseError, ScopeSet};

/// Raw authorization parameters as received on the wire. Every field is
/// optional at this layer so a missing parameter yields a precise
/// `invalid_request` rather than a deserialisation rejection; `validate`
/// enforces presence. `request_uri` is authorize-only (a PAR reference);
/// it is mutually exclusive with the inline parameters.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct RawAuthParams {
    pub client_id: Option<String>,
    pub response_type: Option<String>,
    pub scope: Option<String>,
    pub redirect_uri: Option<String>,
    pub state: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    /// authorize-only: reference a previously-pushed (PAR) request.
    pub request_uri: Option<String>,
}

/// A validated authorization request's parameters. Constructed only via
/// [`validate`], so the invariants (response_type=code, S256 PKCE present,
/// scope parsed) always hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedParams {
    pub client_id: String,
    pub redirect_uri: String,
    pub scope: ScopeSet,
    pub state: Option<String>,
    pub code_challenge: String,
}

/// Why a set of authorization parameters was rejected. Each variant maps to an
/// RFC 6749 §4.1.2.1 OAuth error code via [`AuthParamError::oauth_code`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthParamError {
    /// A required parameter was absent or empty.
    MissingParameter(&'static str),
    /// `response_type` was something other than `code`.
    UnsupportedResponseType(String),
    /// `code_challenge_method` was something other than `S256`.
    UnsupportedChallengeMethod(String),
    /// `scope` failed to parse against the atproto vocabulary.
    InvalidScope(ScopeParseError),
}

impl AuthParamError {
    /// The OAuth 2.0 error code (RFC 6749) for this failure.
    pub fn oauth_code(&self) -> &'static str {
        match self {
            AuthParamError::MissingParameter(_) => "invalid_request",
            AuthParamError::UnsupportedResponseType(_) => "unsupported_response_type",
            AuthParamError::UnsupportedChallengeMethod(_) => "invalid_request",
            AuthParamError::InvalidScope(_) => "invalid_scope",
        }
    }

    /// A human-readable description, safe to render to the resource owner.
    pub fn description(&self) -> String {
        match self {
            AuthParamError::MissingParameter(p) => format!("missing required parameter: {p}"),
            AuthParamError::UnsupportedResponseType(rt) => {
                format!("unsupported response_type '{rt}' (only 'code' is supported)")
            }
            AuthParamError::UnsupportedChallengeMethod(m) => {
                format!("unsupported code_challenge_method '{m}' (only 'S256' is supported)")
            }
            AuthParamError::InvalidScope(e) => e.to_string(),
        }
    }
}

impl std::fmt::Display for AuthParamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.description())
    }
}

impl std::error::Error for AuthParamError {}

fn require<'a>(value: &'a Option<String>, name: &'static str) -> Result<&'a str, AuthParamError> {
    match value {
        Some(v) if !v.is_empty() => Ok(v.as_str()),
        _ => Err(AuthParamError::MissingParameter(name)),
    }
}

/// Validate a full inline parameter set (the non-PAR-reference path).
///
/// Enforces the atproto-OAuth profile: `response_type=code`,
/// `code_challenge_method=S256`, `code_challenge` present, `client_id` /
/// `redirect_uri` present, and a parseable scope. `state` is optional and
/// passed through.
pub fn validate(raw: &RawAuthParams) -> Result<ValidatedParams, AuthParamError> {
    let client_id = require(&raw.client_id, "client_id")?.to_string();
    let redirect_uri = require(&raw.redirect_uri, "redirect_uri")?.to_string();

    let response_type = require(&raw.response_type, "response_type")?;
    if response_type != "code" {
        return Err(AuthParamError::UnsupportedResponseType(
            response_type.to_string(),
        ));
    }

    let code_challenge = require(&raw.code_challenge, "code_challenge")?.to_string();
    let method = require(&raw.code_challenge_method, "code_challenge_method")?;
    if method != "S256" {
        return Err(AuthParamError::UnsupportedChallengeMethod(method.to_string()));
    }

    let scope_str = require(&raw.scope, "scope")?;
    let scope = AtprotoScope::parse_set(scope_str).map_err(AuthParamError::InvalidScope)?;

    Ok(ValidatedParams {
        client_id,
        redirect_uri,
        scope,
        state: raw.state.clone(),
        code_challenge,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> RawAuthParams {
        RawAuthParams {
            client_id: Some("https://app.example.com/client-metadata.json".to_string()),
            response_type: Some("code".to_string()),
            scope: Some("atproto transition:generic".to_string()),
            redirect_uri: Some("https://app.example.com/cb".to_string()),
            state: Some("xyz".to_string()),
            code_challenge: Some("abc123".to_string()),
            code_challenge_method: Some("S256".to_string()),
            request_uri: None,
        }
    }

    #[test]
    fn validates_a_well_formed_request() {
        let v = validate(&good()).unwrap();
        assert_eq!(v.client_id, "https://app.example.com/client-metadata.json");
        assert_eq!(v.redirect_uri, "https://app.example.com/cb");
        assert_eq!(v.scope.to_canonical_string(), "atproto transition:generic");
        assert_eq!(v.state.as_deref(), Some("xyz"));
        assert_eq!(v.code_challenge, "abc123");
    }

    #[test]
    fn rejects_missing_client_id() {
        let mut raw = good();
        raw.client_id = None;
        assert_eq!(
            validate(&raw),
            Err(AuthParamError::MissingParameter("client_id"))
        );
    }

    #[test]
    fn rejects_non_code_response_type() {
        let mut raw = good();
        raw.response_type = Some("token".to_string());
        let err = validate(&raw).unwrap_err();
        assert_eq!(err.oauth_code(), "unsupported_response_type");
    }

    #[test]
    fn rejects_non_s256_challenge_method() {
        let mut raw = good();
        raw.code_challenge_method = Some("plain".to_string());
        let err = validate(&raw).unwrap_err();
        assert_eq!(err.oauth_code(), "invalid_request");
        assert!(err.description().contains("S256"));
    }

    #[test]
    fn rejects_missing_pkce_challenge() {
        let mut raw = good();
        raw.code_challenge = None;
        assert_eq!(
            validate(&raw),
            Err(AuthParamError::MissingParameter("code_challenge"))
        );
    }

    #[test]
    fn rejects_unparseable_scope() {
        let mut raw = good();
        raw.scope = Some("transition:generic".to_string()); // no base atproto
        let err = validate(&raw).unwrap_err();
        assert_eq!(err.oauth_code(), "invalid_scope");
    }
}
