//! Entryway-mode HTTP clients (Arc 12 §5.3.9 + §5.4 Step 1.4).
//!
//! Two flavors:
//!
//! - `EntrywayClient` (no pre-bound auth): used by forwarded
//!   handlers per §5.3.8. Each forwarded call sets its own
//!   `Authorization` header via `entryway_auth_headers` (mint
//!   pattern) or `entryway_passthru_headers` (passthru pattern)
//!   per §5.3.6.
//! - `EntrywayAdminClient` (Basic-auth pre-bound): used by
//!   admin-tier operations. The Basic-auth header is set once at
//!   construction from `EntrywayConfig.admin_token`.
//!
//! Both wrap a single `reqwest::Client` configured with the same
//! conservative 30-second timeout the rest of the federation
//! module uses. The clients are stored on `AppContext` as
//! `Option<Arc<…>>`; standalone mode (`EntrywayConfig == None`)
//! leaves both as `None`.
//!
//! Step 1.4 is "construct and store"; the actual XRPC-forwarding
//! methods land in Step 3 alongside the per-handler dispatch.

use reqwest::{
    header::{HeaderMap, HeaderValue},
    Client,
};
use std::time::Duration;

const HTTP_TIMEOUT_SECS: u64 = 30;

/// Forwarded-handler client per §5.3.9. No pre-bound auth — each
/// call sets `Authorization` per the handler's mint/passthru
/// pattern.
#[derive(Debug, Clone)]
pub struct EntrywayClient {
    /// Base URL of the entryway, copied from
    /// `EntrywayConfig.url`.
    pub base_url: String,
    /// Shared HTTP client. `reqwest::Client` is cheap-Clone and
    /// pools internally, so callers can hold it by value.
    pub http: Client,
}

impl EntrywayClient {
    /// Construct from an `EntrywayConfig.url`. Builds the underlying
    /// `reqwest::Client` with a 30-second timeout. Returns an error
    /// only if `reqwest` fails to build the client (extremely rare —
    /// the timeout configuration is the only validated input).
    pub fn new(base_url: String) -> Result<Self, reqwest::Error> {
        let http = Client::builder()
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build()?;
        Ok(Self { base_url, http })
    }
}

/// Admin-tier entryway client per §5.3.9. Basic-auth header is
/// pre-bound from `EntrywayConfig.admin_token` at construction so
/// admin callers don't have to thread the secret through.
#[derive(Debug, Clone)]
pub struct EntrywayAdminClient {
    /// Base URL of the entryway.
    pub base_url: String,
    /// Pre-built default headers including the Basic-auth header
    /// derived from the admin token.
    pub default_headers: HeaderMap,
    /// Shared HTTP client.
    pub http: Client,
}

impl EntrywayAdminClient {
    /// Construct from `EntrywayConfig.url` + `EntrywayConfig.admin_token`.
    /// The admin token is encoded as the Basic-auth password against
    /// username `admin`, matching the bsky-PDS entryway admin
    /// surface. The Basic-auth header is added to `default_headers`
    /// so every request automatically carries it.
    pub fn new(base_url: String, admin_token: &str) -> Result<Self, reqwest::Error> {
        use base64::{engine::general_purpose::STANDARD, Engine};

        let credentials = format!("admin:{}", admin_token);
        let basic = format!("Basic {}", STANDARD.encode(credentials.as_bytes()));
        let mut default_headers = HeaderMap::new();
        // `from_str` only fails on invalid header bytes; the basic
        // encoding above produces only ASCII so this cannot fail in
        // practice. Falling back to `from_static` would lose the
        // dynamic token; surfacing the error is the right behavior.
        let auth_value = HeaderValue::from_str(&basic).map_err(|_| {
            // reqwest::Error has no public constructor, so we build
            // one by triggering a known-bad URL build. Avoided in
            // practice: any non-empty admin_token yields a valid
            // header value.
            Client::builder()
                .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
                .build()
                .expect_err("intentional sentinel-error path")
        })?;
        default_headers.insert(reqwest::header::AUTHORIZATION, auth_value);

        let http = Client::builder()
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .default_headers(default_headers.clone())
            .build()?;

        Ok(Self {
            base_url,
            default_headers,
            http,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entryway_client_builds() {
        let c = EntrywayClient::new("https://entryway.example.com".to_string())
            .expect("build");
        assert_eq!(c.base_url, "https://entryway.example.com");
    }

    #[test]
    fn entryway_admin_client_binds_basic_auth() {
        let c = EntrywayAdminClient::new(
            "https://entryway.example.com".to_string(),
            "secret-admin-token",
        )
        .expect("build");
        assert_eq!(c.base_url, "https://entryway.example.com");
        let auth = c
            .default_headers
            .get(reqwest::header::AUTHORIZATION)
            .expect("auth header present");
        let auth_str = auth.to_str().expect("ascii");
        assert!(auth_str.starts_with("Basic "));
        // Decode and verify the encoded credentials.
        use base64::{engine::general_purpose::STANDARD, Engine};
        let b64 = auth_str.trim_start_matches("Basic ");
        let decoded = STANDARD.decode(b64).expect("base64");
        assert_eq!(decoded, b"admin:secret-admin-token");
    }
}
