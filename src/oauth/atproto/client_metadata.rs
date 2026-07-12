//! URL-based OAuth client management for the atproto-OAuth provider (Arc 2
//! Phase β.4, chainlink #420 / LOCKED design §3.2 step 7 / R1 F-3.4).
//!
//! atproto OAuth identifies clients by a `client_id` **URL**: the client
//! serves a `client-metadata.json` document at that URL, and the provider
//! fetches it on demand to learn the client's redirect URIs and capabilities.
//! This is fundamentally different from the legacy [`crate::oauth::client`]
//! `ClientManager` (a static `HashMap` of pre-registered clients), which is
//! retained untouched for operator-internal use — the strangler-fig boundary
//! (SD-A2 = (c)).
//!
//! Fetch discipline: HTTPS-only (a localhost-`http` exception is allowed in
//! debug builds for dev/test harnesses, never in release), a strict request
//! timeout, and a hard `client_id == document.client_id` check so a document
//! cannot claim an identity other than its own URL. Results are cached by the
//! `client_id` URL string (not by resolved host/IP — DNS rebinding cannot
//! desync the cache key from the identity).

use std::time::Duration;

use moka::future::Cache;
use serde::Deserialize;
use thiserror::Error;
use url::Url;

/// Request timeout for fetching a client-metadata document.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// Cache capacity (distinct client_id URLs).
const CACHE_CAPACITY: u64 = 1_000;
/// Cache entry lifetime.
const CACHE_TTL: Duration = Duration::from_secs(3_600);

/// The subset of an atproto OAuth `client-metadata.json` the provider needs.
/// Unknown fields are ignored; absent optionals stay `None`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ClientMetadata {
    /// MUST equal the URL it was fetched from (enforced in [`ClientMetadataFetcher::fetch`]).
    pub client_id: String,
    /// Allowed redirect URIs — the authorize flow matches against these.
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub response_types: Option<Vec<String>>,
    #[serde(default)]
    pub grant_types: Option<Vec<String>>,
    #[serde(default)]
    pub token_endpoint_auth_method: Option<String>,
    #[serde(default)]
    pub dpop_bound_access_tokens: Option<bool>,
    #[serde(default)]
    pub jwks_uri: Option<String>,
    #[serde(default)]
    pub application_type: Option<String>,
}

impl ClientMetadata {
    /// True iff `requested` is an exact member of the registered redirect URIs.
    /// atproto OAuth requires exact-match (no prefix/wildcard) redirect URIs.
    pub fn allows_redirect_uri(&self, requested: &str) -> bool {
        self.redirect_uris.iter().any(|u| u == requested)
    }
}

/// Failure modes of a client-metadata fetch. Distinct variants so the
/// authorize endpoint (β.3) can map each to the right OAuth error.
#[derive(Debug, Error)]
pub enum ClientMetadataError {
    #[error("client_id is not a valid URL: {0}")]
    InvalidUrl(String),
    #[error("client_id must be an https URL: {0}")]
    InsecureScheme(String),
    #[error("failed to fetch client metadata: {0}")]
    Fetch(String),
    #[error("client metadata endpoint returned status {0}")]
    BadStatus(u16),
    #[error("client metadata is not valid JSON: {0}")]
    Parse(String),
    #[error("client metadata client_id '{found}' does not match its URL '{expected}'")]
    ClientIdMismatch { expected: String, found: String },
}

/// Fetches and caches atproto OAuth client-metadata documents on demand.
#[derive(Clone)]
pub struct ClientMetadataFetcher {
    http: reqwest::Client,
    cache: Cache<String, ClientMetadata>,
}

impl Default for ClientMetadataFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientMetadataFetcher {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            // No redirect-following: a client-metadata URL must serve its own
            // document directly, and following redirects would let the
            // client_id URL point somewhere it does not control.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client builds with static config");
        let cache = Cache::builder()
            .max_capacity(CACHE_CAPACITY)
            .time_to_live(CACHE_TTL)
            .build();
        Self { http, cache }
    }

    /// Resolve `client_id` to its metadata document, using the cache when warm.
    ///
    /// Validates: the URL parses and is https (or localhost-http in debug),
    /// the fetch succeeds within the timeout, the body is JSON, and the
    /// document's `client_id` equals the requested URL (anti-spoofing).
    pub async fn fetch(
        &self,
        client_id: &str,
    ) -> Result<ClientMetadata, ClientMetadataError> {
        if let Some(hit) = self.cache.get(client_id).await {
            return Ok(hit);
        }

        let url = Url::parse(client_id)
            .map_err(|e| ClientMetadataError::InvalidUrl(format!("{client_id}: {e}")))?;
        require_secure_url(&url)?;

        let resp = self
            .http
            .get(url.clone())
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| ClientMetadataError::Fetch(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ClientMetadataError::BadStatus(resp.status().as_u16()));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| ClientMetadataError::Fetch(e.to_string()))?;
        let metadata: ClientMetadata = serde_json::from_str(&body)
            .map_err(|e| ClientMetadataError::Parse(e.to_string()))?;

        // The document must claim exactly the identity it was fetched from.
        if metadata.client_id != client_id {
            return Err(ClientMetadataError::ClientIdMismatch {
                expected: client_id.to_string(),
                found: metadata.client_id,
            });
        }

        self.cache
            .insert(client_id.to_string(), metadata.clone())
            .await;
        Ok(metadata)
    }
}

/// Enforce the transport policy: https in all builds; `http` only for
/// loopback hosts and only in debug builds (dev/test harnesses serve
/// client-metadata over http on `127.0.0.1`). Release builds are https-only.
fn require_secure_url(url: &Url) -> Result<(), ClientMetadataError> {
    match url.scheme() {
        "https" => Ok(()),
        "http" if cfg!(debug_assertions) && is_loopback_host(url) => Ok(()),
        _ => Err(ClientMetadataError::InsecureScheme(url.to_string())),
    }
}

fn is_loopback_host(url: &Url) -> bool {
    matches!(url.host_str(), Some("localhost") | Some("127.0.0.1") | Some("[::1]"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(client_id: &str, redirects: &[&str]) -> ClientMetadata {
        ClientMetadata {
            client_id: client_id.to_string(),
            redirect_uris: redirects.iter().map(|s| s.to_string()).collect(),
            client_name: None,
            scope: None,
            response_types: None,
            grant_types: None,
            token_endpoint_auth_method: None,
            dpop_bound_access_tokens: None,
            jwks_uri: None,
            application_type: None,
        }
    }

    #[test]
    fn redirect_uri_is_exact_match() {
        let m = meta("https://app.example.com/cm.json", &["https://app.example.com/cb"]);
        assert!(m.allows_redirect_uri("https://app.example.com/cb"));
        // No prefix / trailing-slash / subpath leniency.
        assert!(!m.allows_redirect_uri("https://app.example.com/cb/"));
        assert!(!m.allows_redirect_uri("https://app.example.com/cb/evil"));
        assert!(!m.allows_redirect_uri("https://evil.example.com/cb"));
    }

    #[test]
    fn transport_policy_https_only_in_release_loopback_http_in_debug() {
        assert!(require_secure_url(&Url::parse("https://app.example.com/cm.json").unwrap()).is_ok());
        // Non-loopback http is always rejected.
        assert!(require_secure_url(&Url::parse("http://app.example.com/cm.json").unwrap()).is_err());
        // Loopback http is accepted only in debug builds (tests run in debug).
        let loopback = require_secure_url(&Url::parse("http://127.0.0.1:9000/cm.json").unwrap());
        assert_eq!(loopback.is_ok(), cfg!(debug_assertions));
    }

    #[test]
    fn metadata_parses_with_unknown_fields_and_defaults() {
        let json = r#"{
            "client_id": "https://app.example.com/client-metadata.json",
            "redirect_uris": ["https://app.example.com/cb"],
            "dpop_bound_access_tokens": true,
            "some_future_field": {"nested": 1}
        }"#;
        let m: ClientMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(m.client_id, "https://app.example.com/client-metadata.json");
        assert_eq!(m.dpop_bound_access_tokens, Some(true));
        assert!(m.scope.is_none() && m.grant_types.is_none());
        assert!(m.allows_redirect_uri("https://app.example.com/cb"));
    }

    #[tokio::test]
    async fn fetch_rejects_non_url_and_insecure() {
        let f = ClientMetadataFetcher::new();
        assert!(matches!(
            f.fetch("not a url").await,
            Err(ClientMetadataError::InvalidUrl(_))
        ));
        assert!(matches!(
            f.fetch("http://app.example.com/cm.json").await,
            Err(ClientMetadataError::InsecureScheme(_))
        ));
    }

    #[tokio::test]
    async fn fetch_resolves_and_caches_then_validates_client_id() {
        // Spin a one-shot localhost http server that serves a client-metadata
        // document; debug-build loopback-http is allowed.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client_id = format!("http://127.0.0.1:{}/client-metadata.json", addr.port());
        let body = format!(
            r#"{{"client_id":"{client_id}","redirect_uris":["https://app/cb"],"dpop_bound_access_tokens":true}}"#
        );
        let server = tokio::spawn(async move {
            // Serve two requests: the first fetch, then a (cache-miss) mismatch
            // probe is not needed — only one real fetch hits the network.
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await.unwrap();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
        });

        let f = ClientMetadataFetcher::new();
        let m = f.fetch(&client_id).await.expect("fetch ok");
        assert_eq!(m.client_id, client_id);
        assert!(m.allows_redirect_uri("https://app/cb"));
        assert_eq!(m.dpop_bound_access_tokens, Some(true));

        // Second fetch is served from cache — the server only accepted one
        // connection, so a network round-trip here would hang/fail.
        let cached = f.fetch(&client_id).await.expect("served from cache");
        assert_eq!(cached, m);

        server.await.unwrap();
    }

    #[tokio::test]
    async fn fetch_rejects_client_id_mismatch() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://127.0.0.1:{}/client-metadata.json", addr.port());
        // Document claims a different client_id than its URL → spoofing.
        let body = r#"{"client_id":"https://evil.example.com/cm.json","redirect_uris":[]}"#;
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await.unwrap();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
        });
        let f = ClientMetadataFetcher::new();
        assert!(matches!(
            f.fetch(&url).await,
            Err(ClientMetadataError::ClientIdMismatch { .. })
        ));
        server.await.unwrap();
    }
}
