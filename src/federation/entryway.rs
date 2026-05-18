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

    /// Arc 12 §5.3.8 — POST-method XRPC forward to the entryway.
    /// `nsid` is the lexicon NSID (e.g.
    /// `"com.atproto.identity.signPlcOperation"`); the request URL
    /// is `{base_url}/xrpc/{nsid}`. `headers` (from
    /// `entryway_auth_headers` / `entryway_passthru_headers`) is
    /// merged onto the request. Body is JSON-serialised from `req`.
    /// Returns the upstream response deserialised into `Res`.
    ///
    /// Non-2xx upstream status is surfaced as `PdsError::Internal`
    /// with the upstream body text appended for diagnosis. Network
    /// failures become `PdsError::Internal` with the underlying
    /// error message.
    pub async fn xrpc_post_json<Req, Res>(
        &self,
        nsid: &str,
        headers: axum::http::HeaderMap,
        req: &Req,
    ) -> crate::error::PdsResult<Res>
    where
        Req: serde::Serialize + ?Sized,
        Res: serde::de::DeserializeOwned,
    {
        let url = format!("{}/xrpc/{}", self.base_url.trim_end_matches('/'), nsid);
        let req_headers = axum_to_reqwest_headers(&headers)?;
        let response = self
            .http
            .post(&url)
            .headers(req_headers)
            .json(req)
            .send()
            .await
            .map_err(|e| {
                crate::error::PdsError::Internal(format!(
                    "entryway forward POST {} failed: {}",
                    nsid, e
                ))
            })?;
        ensure_2xx_then_decode(nsid, response).await
    }

    /// Arc 12 §5.3.8 — GET-method XRPC forward to the entryway.
    /// Same as [`xrpc_post_json`] but `method = GET` and no request
    /// body. `query` is appended as URL query parameters (caller
    /// can pass `&[]` for query-less forwards).
    pub async fn xrpc_get_json<Res>(
        &self,
        nsid: &str,
        headers: axum::http::HeaderMap,
        query: &[(&str, &str)],
    ) -> crate::error::PdsResult<Res>
    where
        Res: serde::de::DeserializeOwned,
    {
        let url = format!("{}/xrpc/{}", self.base_url.trim_end_matches('/'), nsid);
        let req_headers = axum_to_reqwest_headers(&headers)?;
        let response = self
            .http
            .get(&url)
            .headers(req_headers)
            .query(query)
            .send()
            .await
            .map_err(|e| {
                crate::error::PdsError::Internal(format!(
                    "entryway forward GET {} failed: {}",
                    nsid, e
                ))
            })?;
        ensure_2xx_then_decode(nsid, response).await
    }
}

/// Convert axum's `HeaderMap` (Step 2.x builders return this) into a
/// `reqwest::HeaderMap` for the outbound HTTP call. They are the
/// same underlying types from the `http` crate but reqwest re-exports
/// its own alias, so an explicit per-entry copy is the portable path.
fn axum_to_reqwest_headers(
    src: &axum::http::HeaderMap,
) -> crate::error::PdsResult<reqwest::header::HeaderMap> {
    let mut out = reqwest::header::HeaderMap::new();
    for (k, v) in src.iter() {
        let name = reqwest::header::HeaderName::from_bytes(k.as_str().as_bytes()).map_err(|e| {
            crate::error::PdsError::Internal(format!("invalid forwarded header name {:?}: {}", k, e))
        })?;
        let value = reqwest::header::HeaderValue::from_bytes(v.as_bytes()).map_err(|e| {
            crate::error::PdsError::Internal(format!("invalid forwarded header value: {}", e))
        })?;
        out.insert(name, value);
    }
    Ok(out)
}

async fn ensure_2xx_then_decode<Res>(
    nsid: &str,
    response: reqwest::Response,
) -> crate::error::PdsResult<Res>
where
    Res: serde::de::DeserializeOwned,
{
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|e| format!("<body read failed: {}>", e));
        return Err(crate::error::PdsError::Internal(format!(
            "entryway forward {} returned {}: {}",
            nsid, status, body
        )));
    }
    response.json::<Res>().await.map_err(|e| {
        crate::error::PdsError::Internal(format!(
            "entryway forward {} response decode failed: {}",
            nsid, e
        ))
    })
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
