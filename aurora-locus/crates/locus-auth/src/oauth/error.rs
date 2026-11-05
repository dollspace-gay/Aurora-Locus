//! OAuth errors as the reference's `OAuthError` family: `{error,
//! error_description}` bodies, a status, and (for resource and DPoP errors)
//! a `WWW-Authenticate` challenge.

use super::jose::VERIFY_ALGS;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{error}: {description}")]
pub struct OAuthError {
    pub error: String,
    pub description: String,
    pub status: u16,
    /// The `WWW-Authenticate` header value, when the error has one.
    pub www_authenticate: Option<String>,
}

fn challenge(scheme: &str, error: &str, description: &str) -> String {
    // Parameters holding a quote are left out, as the reference does.
    let mut params = Vec::new();
    if scheme == "DPoP" {
        params.push(format!("algs=\"{}\"", VERIFY_ALGS.join(" ")));
    }
    for (k, v) in [("error", error), ("error_description", description)] {
        if !v.is_empty() && !v.contains('"') {
            params.push(format!("{k}=\"{v}\""));
        }
    }
    if params.is_empty() {
        scheme.to_owned()
    } else {
        format!("{scheme} {}", params.join(", "))
    }
}

impl OAuthError {
    #[must_use]
    pub fn new(error: &str, description: impl Into<String>, status: u16) -> Self {
        Self {
            error: error.to_owned(),
            description: description.into(),
            status,
            www_authenticate: None,
        }
    }

    #[must_use]
    pub fn invalid_request(description: impl Into<String>) -> Self {
        Self::new("invalid_request", description, 400)
    }

    #[must_use]
    pub fn invalid_grant(description: impl Into<String>) -> Self {
        Self::new("invalid_grant", description, 400)
    }

    #[must_use]
    pub fn invalid_client(description: impl Into<String>) -> Self {
        Self::new("invalid_client", description, 400)
    }

    #[must_use]
    pub fn invalid_client_metadata(description: impl Into<String>) -> Self {
        Self::new("invalid_client_metadata", description, 400)
    }

    #[must_use]
    pub fn invalid_scope(description: impl Into<String>) -> Self {
        Self::new("invalid_scope", description, 400)
    }

    #[must_use]
    pub fn access_denied(description: impl Into<String>) -> Self {
        Self::new("access_denied", description, 400)
    }

    #[must_use]
    pub fn server_error(description: impl Into<String>) -> Self {
        Self::new("server_error", description, 500)
    }

    /// A 401 with a challenge for `scheme` (`DPoP` or `Bearer`).
    #[must_use]
    pub fn www(scheme: &str, error: &str, description: impl Into<String>) -> Self {
        let description = description.into();
        Self {
            www_authenticate: Some(challenge(scheme, error, &description)),
            error: error.to_owned(),
            description,
            status: 401,
        }
    }

    #[must_use]
    pub fn invalid_dpop_proof(description: impl Into<String>) -> Self {
        Self::www("DPoP", "invalid_dpop_proof", description)
    }

    #[must_use]
    pub fn invalid_token(token_type: &str, description: impl Into<String>) -> Self {
        Self::www(token_type, "invalid_token", description)
    }

    #[must_use]
    pub fn invalid_dpop_key_binding() -> Self {
        Self::www("DPoP", "invalid_token", "Invalid DPoP key binding")
    }

    /// The authorization server's "use a nonce" (400, no challenge); a
    /// resource server turns it into a 401 challenge with
    /// [`OAuthError::for_resource`].
    #[must_use]
    pub fn use_dpop_nonce(description: Option<&str>) -> Self {
        Self::new(
            "use_dpop_nonce",
            description.unwrap_or("Authorization server requires nonce in DPoP proof"),
            400,
        )
    }

    /// The error as a resource server answers it (a nonce request becomes a
    /// DPoP challenge).
    #[must_use]
    pub fn for_resource(self) -> Self {
        if self.error == "use_dpop_nonce" {
            Self::www("DPoP", "use_dpop_nonce", self.description)
        } else {
            self
        }
    }

    /// The JSON body.
    #[must_use]
    pub fn body(&self) -> serde_json::Value {
        serde_json::json!({"error": self.error, "error_description": self.description})
    }
}
