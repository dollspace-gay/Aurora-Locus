//! Aurora-owned admin OAuth client flow (RFC 9126 PAR + RFC 6749 code grant +
//! refresh, all DPoP-bound per RFC 9449).
//!
//! Phase 2 of the admin OAuth decoupling arc (chainlink #439). Drives the
//! browser-loopback admin login ceremony against Aurora's OWN authorization
//! server (`oauth::atproto`): push the authorization request, build the
//! authorize URL the browser is sent to, exchange the returned code for a
//! DPoP-bound token pair, and rotate that pair on refresh. Every back-channel
//! request carries a fresh DPoP proof built by [`super::dpop::DpopProofBuilder`]
//! (Phase 1) — no proto-blue-oauth in the loop.
//!
//! The request/response shapes here are pinned to Aurora's own AS endpoints:
//! - PAR      `POST {as}/oauth/atproto/par`   → `{request_uri, expires_in}` (201)
//! - authorize `GET {as}/oauth/atproto/authorize?client_id=…&request_uri=…`
//! - token    `POST {as}/oauth/atproto/token` → `{access_token, refresh_token,
//!   token_type, expires_in, scope}` (200); both the `authorization_code` and
//!   `refresh_token` grants.
//!
//! DPoP nonce: Aurora's AS does not currently challenge with a `DPoP-Nonce`
//! (its `verify_dpop_required` never issues one), so the nonce round-trip below
//! is forward-compatible rather than exercised against Aurora today — but a
//! spec-conformant AS may demand one, so the client absorbs any `DPoP-Nonce`
//! response header and retries once on a `use_dpop_nonce` challenge.
//!
//! Consumed by the admin OAuth callback (`api::oauth_admin`), which drives this
//! client end to end: PAR at `/admin-oauth/login`, code exchange at
//! `/admin-oauth/callback`.

use serde::Deserialize;

use super::dpop::DpopProofBuilder;
use crate::config::ServerConfig;

/// The admin OAuth scope. Matches what the existing admin flow
/// (`api::oauth_admin`) registers in its client metadata, so a session minted
/// through this client carries the same grant.
const ADMIN_OAUTH_SCOPE: &str = "atproto transition:generic";

/// Per-request timeout for the back-channel calls to the AS.
const HTTP_TIMEOUT_SECS: u64 = 30;

/// Parsed PAR response (RFC 9126 §2.2).
///
/// `expires_in` is part of the parsed response contract (and asserted by tests)
/// even though the admin callback — the current in-tree consumer — reads only
/// `request_uri`; `allow(dead_code)` keeps the full response type honest without
/// a synthetic read.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ParResponse {
    /// Opaque reference the browser hands to the authorize endpoint.
    pub request_uri: String,
    /// Seconds the `request_uri` remains usable.
    pub expires_in: i64,
}

/// Parsed token response (authorization_code exchange or refresh rotation).
///
/// The admin flow reads only `access_token` (to resolve the DID); the remaining
/// fields are the full token-response contract, exercised by tests and available
/// to other consumers. `allow(dead_code)` keeps the type faithful to the wire
/// response rather than trimming it to one caller's needs.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    /// `DPoP` for atproto tokens.
    #[serde(default)]
    pub token_type: String,
    /// Access-token lifetime in seconds.
    #[serde(default)]
    pub expires_in: i64,
    #[serde(default)]
    pub scope: String,
}

/// Errors from the admin OAuth client flow.
#[derive(Debug, thiserror::Error)]
pub enum OAuthClientError {
    /// A DPoP proof could not be constructed for the request.
    #[error("DPoP proof construction failed: {0}")]
    Dpop(#[from] super::dpop::DpopError),
    /// The back-channel HTTP request itself failed (connect/timeout/transport).
    #[error("HTTP transport error: {0}")]
    Http(String),
    /// The AS returned a non-2xx OAuth error response.
    #[error("authorization server error {status}: {error} ({description})")]
    Server {
        status: u16,
        error: String,
        description: String,
    },
    /// A 2xx response body could not be parsed into the expected shape.
    #[error("malformed authorization-server response: {0}")]
    Parse(String),
}

/// Aurora-owned client for the admin OAuth ceremony against Aurora's own AS.
///
/// Holds one [`DpopProofBuilder`] — hence one ephemeral P-256 key — for the
/// whole ceremony, so the PAR, code-exchange, and refresh proofs are all signed
/// by the key the issued tokens are bound to. Construct one per admin login
/// attempt.
pub struct AdminOAuthClient {
    /// The admin client's `client_id` — a URL to its client-metadata document.
    client_id: String,
    /// Base URL of the authorization server (Aurora's own public URL), with any
    /// trailing slash trimmed.
    as_base_url: String,
    /// The scope requested for the admin session.
    scope: String,
    /// Proof builder holding this ceremony's ephemeral DPoP key + any nonce.
    dpop: DpopProofBuilder,
    http: reqwest::Client,
}

impl AdminOAuthClient {
    /// Build a client for an explicit `client_id` + AS base URL.
    ///
    /// This is the form the tests use to point at a mock AS; production code
    /// uses [`AdminOAuthClient::from_config`], which derives both from the
    /// running service config.
    pub fn new(client_id: String, as_base_url: String) -> Result<Self, OAuthClientError> {
        let dpop = DpopProofBuilder::new()?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build()
            .map_err(|e| OAuthClientError::Http(e.to_string()))?;
        Ok(Self {
            client_id,
            as_base_url: as_base_url.trim_end_matches('/').to_string(),
            scope: ADMIN_OAUTH_SCOPE.to_string(),
            dpop,
            http,
        })
    }

    /// Derive a client from the running service config: the AS is Aurora's own
    /// public URL (loopback to its own `oauth::atproto` endpoints), and the
    /// `client_id` is the configured admin OAuth client id.
    pub fn from_config(config: &ServerConfig) -> Result<Self, OAuthClientError> {
        Self::new(
            config.authentication.oauth.client_id.clone(),
            config.service.effective_public_url(),
        )
    }

    /// Push the authorization request to the AS (RFC 9126). Returns the opaque
    /// `request_uri` to hand to [`AdminOAuthClient::build_authorize_url`].
    pub async fn pushed_authorization_request(
        &mut self,
        state: &str,
        code_challenge: &str,
        redirect_uri: &str,
    ) -> Result<ParResponse, OAuthClientError> {
        let url = format!("{}/oauth/atproto/par", self.as_base_url);
        // Clone the client-owned fields so the form slice doesn't hold a borrow
        // of `self` across the `&mut self` call below.
        let client_id = self.client_id.clone();
        let scope = self.scope.clone();
        let form = [
            ("client_id", client_id.as_str()),
            ("response_type", "code"),
            ("scope", scope.as_str()),
            ("redirect_uri", redirect_uri),
            ("state", state),
            ("code_challenge", code_challenge),
            ("code_challenge_method", "S256"),
        ];
        let json = self.post_form_with_dpop(&url, &form).await?;
        serde_json::from_value(json).map_err(|e| OAuthClientError::Parse(e.to_string()))
    }

    /// Build the authorization URL the browser is redirected to. atproto OAuth
    /// makes PAR mandatory, so this carries only `client_id` + `request_uri`.
    pub fn build_authorize_url(&self, request_uri: &str) -> String {
        format!(
            "{}/oauth/atproto/authorize?client_id={}&request_uri={}",
            self.as_base_url,
            urlencoding::encode(&self.client_id),
            urlencoding::encode(request_uri),
        )
    }

    /// Exchange an authorization code for a DPoP-bound token pair.
    pub async fn exchange_code_for_tokens(
        &mut self,
        code: &str,
        code_verifier: &str,
        redirect_uri: &str,
    ) -> Result<TokenResponse, OAuthClientError> {
        let url = format!("{}/oauth/atproto/token", self.as_base_url);
        let client_id = self.client_id.clone();
        let form = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", code_verifier),
            ("client_id", client_id.as_str()),
            ("redirect_uri", redirect_uri),
        ];
        let json = self.post_form_with_dpop(&url, &form).await?;
        serde_json::from_value(json).map_err(|e| OAuthClientError::Parse(e.to_string()))
    }

    /// Rotate the token pair with the refresh grant. The proof is signed by the
    /// same DPoP key as issuance, so the AS's proof-of-possession check passes.
    ///
    /// Part of the complete, tested OAuth-client surface, but not exercised by
    /// the admin login flow: that flow uses the OAuth token only to resolve the
    /// operator's DID and then mints a separate Aurora account session, so it
    /// never refreshes the OAuth token. Retained for reuse by any future
    /// consumer that keeps the OAuth session live.
    #[allow(dead_code)]
    pub async fn refresh_tokens(
        &mut self,
        refresh_token: &str,
    ) -> Result<TokenResponse, OAuthClientError> {
        let url = format!("{}/oauth/atproto/token", self.as_base_url);
        let client_id = self.client_id.clone();
        let form = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id.as_str()),
        ];
        let json = self.post_form_with_dpop(&url, &form).await?;
        serde_json::from_value(json).map_err(|e| OAuthClientError::Parse(e.to_string()))
    }

    /// POST a form to the AS with a fresh DPoP proof, handling the one-shot
    /// `use_dpop_nonce` retry. Returns the parsed JSON body on 2xx, or an
    /// [`OAuthClientError::Server`] carrying the AS's OAuth error otherwise.
    async fn post_form_with_dpop(
        &mut self,
        url: &str,
        form: &[(&str, &str)],
    ) -> Result<serde_json::Value, OAuthClientError> {
        let (status, nonce, body) = self.one_shot(url, form).await?;
        // Absorb any server nonce for this and subsequent requests.
        if let Some(ref n) = nonce {
            self.dpop.with_nonce(n.clone());
        }
        // If the AS demanded a nonce and actually supplied one, retry once with
        // a proof that now carries it. Without a supplied nonce a retry would be
        // identical, so we fall through to surface the error instead.
        if status == reqwest::StatusCode::UNAUTHORIZED && is_use_dpop_nonce(&body) && nonce.is_some()
        {
            let (status2, nonce2, body2) = self.one_shot(url, form).await?;
            if let Some(n) = nonce2 {
                self.dpop.with_nonce(n);
            }
            return interpret(status2, &body2);
        }
        interpret(status, &body)
    }

    /// Issue a single DPoP-authenticated POST and return
    /// `(status, dpop_nonce_header, body_bytes)`.
    async fn one_shot(
        &self,
        url: &str,
        form: &[(&str, &str)],
    ) -> Result<(reqwest::StatusCode, Option<String>, Vec<u8>), OAuthClientError> {
        // PAR and token are POST; the proof's htu is this exact URL (the builder
        // strips the query, matching the AS-side validator).
        let proof = self.dpop.build_proof("POST", url, None)?;
        let resp = self
            .http
            .post(url)
            .header("DPoP", proof)
            .form(form)
            .send()
            .await
            .map_err(|e| OAuthClientError::Http(e.to_string()))?;
        let status = resp.status();
        let nonce = resp
            .headers()
            .get("DPoP-Nonce")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body = resp
            .bytes()
            .await
            .map_err(|e| OAuthClientError::Http(e.to_string()))?
            .to_vec();
        Ok((status, nonce, body))
    }
}

/// Interpret an AS response: parse the JSON body on 2xx, or map a non-2xx to an
/// [`OAuthClientError::Server`] carrying its OAuth `error`/`error_description`.
fn interpret(
    status: reqwest::StatusCode,
    body: &[u8],
) -> Result<serde_json::Value, OAuthClientError> {
    if status.is_success() {
        serde_json::from_slice(body).map_err(|e| OAuthClientError::Parse(e.to_string()))
    } else {
        let json = serde_json::from_slice::<serde_json::Value>(body).unwrap_or(serde_json::Value::Null);
        Err(OAuthClientError::Server {
            status: status.as_u16(),
            error: json
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error")
                .to_string(),
            description: json
                .get("error_description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        })
    }
}

/// Whether an AS error body is the RFC 9449 `use_dpop_nonce` challenge.
fn is_use_dpop_nonce(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.as_str())
                .map(|s| s == "use_dpop_nonce")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{header, HeaderMap, HeaderName, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::Router;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::sync::oneshot;

    const CLIENT_ID: &str = "https://pds.example.com/oauth/client-metadata.json";
    const REDIRECT_URI: &str = "https://pds.example.com/admin-oauth/callback";

    /// A request the mock AS captured, for post-hoc assertions.
    #[derive(Clone, Default)]
    struct Captured {
        dpop: Option<String>,
        form: String,
    }

    /// Spawn a mock AS on an ephemeral port; returns (base_url, shutdown).
    async fn spawn(app: Router) -> (String, oneshot::Sender<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await
                .ok();
        });
        (format!("http://{addr}"), tx)
    }

    /// Base64url-decode a JWT segment into JSON.
    fn decode_segment(seg: &str) -> serde_json::Value {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        let bytes = URL_SAFE_NO_PAD.decode(seg).expect("valid base64url");
        serde_json::from_slice(&bytes).expect("segment JSON")
    }

    /// (header, claims) of a compact JWT.
    fn split_jwt(jwt: &str) -> (serde_json::Value, serde_json::Value) {
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3, "DPoP proof must be a compact JWT");
        (decode_segment(parts[0]), decode_segment(parts[1]))
    }

    fn parse_form(body: &str) -> HashMap<String, String> {
        url::form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect()
    }

    fn json_response(status: StatusCode, body: &'static str) -> axum::response::Response {
        (
            status,
            [(header::CONTENT_TYPE, "application/json")],
            body,
        )
            .into_response()
    }

    #[tokio::test]
    async fn par_sends_all_params_and_a_dpop_proof() {
        let cap = Arc::new(Mutex::new(Vec::<Captured>::new()));
        let cap_h = cap.clone();
        let app = Router::new().route(
            "/oauth/atproto/par",
            post(move |headers: HeaderMap, body: String| {
                let cap = cap_h.clone();
                async move {
                    cap.lock().unwrap().push(Captured {
                        dpop: headers
                            .get("DPoP")
                            .and_then(|v| v.to_str().ok())
                            .map(String::from),
                        form: body,
                    });
                    json_response(
                        StatusCode::CREATED,
                        r#"{"request_uri":"urn:ietf:params:oauth:request_uri:xyz","expires_in":60}"#,
                    )
                }
            }),
        );
        let (base, _shutdown) = spawn(app).await;

        let mut client = AdminOAuthClient::new(CLIENT_ID.to_string(), base).unwrap();
        let par = client
            .pushed_authorization_request("state-123", "challenge-abc", REDIRECT_URI)
            .await
            .expect("PAR succeeds");
        assert_eq!(par.request_uri, "urn:ietf:params:oauth:request_uri:xyz");
        assert_eq!(par.expires_in, 60);

        let captured = cap.lock().unwrap().clone();
        assert_eq!(captured.len(), 1);
        let c = &captured[0];

        // DPoP proof present, well-formed, for POST {…}/par.
        let proof = c.dpop.as_deref().expect("PAR carries a DPoP header");
        let (header, claims) = split_jwt(proof);
        assert_eq!(header["typ"], "dpop+jwt");
        assert_eq!(claims["htm"], "POST");
        assert!(claims["htu"].as_str().unwrap().ends_with("/oauth/atproto/par"));

        // All PAR params present + correct.
        let f = parse_form(&c.form);
        assert_eq!(f["client_id"], CLIENT_ID);
        assert_eq!(f["response_type"], "code");
        assert_eq!(f["scope"], ADMIN_OAUTH_SCOPE);
        assert_eq!(f["redirect_uri"], REDIRECT_URI);
        assert_eq!(f["state"], "state-123");
        assert_eq!(f["code_challenge"], "challenge-abc");
        assert_eq!(f["code_challenge_method"], "S256");
    }

    #[test]
    fn authorize_url_carries_encoded_client_id_and_request_uri() {
        let client =
            AdminOAuthClient::new(CLIENT_ID.to_string(), "https://pds.example.com/".to_string())
                .unwrap();
        let url = client.build_authorize_url("urn:ietf:params:oauth:request_uri:xyz");
        // Trailing slash trimmed at construction; single authorize path.
        assert!(url.starts_with("https://pds.example.com/oauth/atproto/authorize?"));
        // Both values percent-encoded (':' and '/' escaped).
        assert!(url.contains("client_id=https%3A%2F%2Fpds.example.com"));
        assert!(url.contains("request_uri=urn%3Aietf%3Aparams%3Aoauth%3Arequest_uri%3Axyz"));
    }

    #[tokio::test]
    async fn code_exchange_sends_grant_params_and_parses_tokens() {
        let cap = Arc::new(Mutex::new(Vec::<Captured>::new()));
        let cap_h = cap.clone();
        let app = Router::new().route(
            "/oauth/atproto/token",
            post(move |headers: HeaderMap, body: String| {
                let cap = cap_h.clone();
                async move {
                    cap.lock().unwrap().push(Captured {
                        dpop: headers
                            .get("DPoP")
                            .and_then(|v| v.to_str().ok())
                            .map(String::from),
                        form: body,
                    });
                    json_response(
                        StatusCode::OK,
                        r#"{"access_token":"at_1","refresh_token":"rt_1","token_type":"DPoP","expires_in":3600,"scope":"atproto transition:generic"}"#,
                    )
                }
            }),
        );
        let (base, _shutdown) = spawn(app).await;

        let mut client = AdminOAuthClient::new(CLIENT_ID.to_string(), base).unwrap();
        let tokens = client
            .exchange_code_for_tokens("the-code", "the-verifier", REDIRECT_URI)
            .await
            .expect("exchange succeeds");
        assert_eq!(tokens.access_token, "at_1");
        assert_eq!(tokens.refresh_token, "rt_1");
        assert_eq!(tokens.token_type, "DPoP");
        assert_eq!(tokens.expires_in, 3600);

        let captured = cap.lock().unwrap().clone();
        assert_eq!(captured.len(), 1);
        assert!(captured[0].dpop.is_some());
        let f = parse_form(&captured[0].form);
        assert_eq!(f["grant_type"], "authorization_code");
        assert_eq!(f["code"], "the-code");
        assert_eq!(f["code_verifier"], "the-verifier");
        assert_eq!(f["client_id"], CLIENT_ID);
        assert_eq!(f["redirect_uri"], REDIRECT_URI);
    }

    #[tokio::test]
    async fn refresh_sends_refresh_grant_params() {
        let cap = Arc::new(Mutex::new(Vec::<Captured>::new()));
        let cap_h = cap.clone();
        let app = Router::new().route(
            "/oauth/atproto/token",
            post(move |body: String| {
                let cap = cap_h.clone();
                async move {
                    cap.lock().unwrap().push(Captured {
                        dpop: None,
                        form: body,
                    });
                    json_response(
                        StatusCode::OK,
                        r#"{"access_token":"at_2","refresh_token":"rt_2","token_type":"DPoP","expires_in":3600,"scope":"atproto transition:generic"}"#,
                    )
                }
            }),
        );
        let (base, _shutdown) = spawn(app).await;

        let mut client = AdminOAuthClient::new(CLIENT_ID.to_string(), base).unwrap();
        let tokens = client.refresh_tokens("rt_1").await.expect("refresh succeeds");
        assert_eq!(tokens.access_token, "at_2");
        assert_eq!(tokens.refresh_token, "rt_2");

        let f = parse_form(&cap.lock().unwrap()[0].form);
        assert_eq!(f["grant_type"], "refresh_token");
        assert_eq!(f["refresh_token"], "rt_1");
        assert_eq!(f["client_id"], CLIENT_ID);
    }

    #[tokio::test]
    async fn use_dpop_nonce_challenge_triggers_one_retry_with_the_nonce() {
        let calls = Arc::new(AtomicUsize::new(0));
        let cap = Arc::new(Mutex::new(Vec::<Captured>::new()));
        let calls_h = calls.clone();
        let cap_h = cap.clone();
        let app = Router::new().route(
            "/oauth/atproto/token",
            post(move |headers: HeaderMap, body: String| {
                let calls = calls_h.clone();
                let cap = cap_h.clone();
                async move {
                    let n = calls.fetch_add(1, Ordering::SeqCst);
                    cap.lock().unwrap().push(Captured {
                        dpop: headers
                            .get("DPoP")
                            .and_then(|v| v.to_str().ok())
                            .map(String::from),
                        form: body,
                    });
                    if n == 0 {
                        // First call: demand a nonce.
                        (
                            StatusCode::UNAUTHORIZED,
                            [
                                (header::CONTENT_TYPE, "application/json"),
                                (HeaderName::from_static("dpop-nonce"), "srv-nonce-1"),
                            ],
                            r#"{"error":"use_dpop_nonce","error_description":"nonce required"}"#,
                        )
                            .into_response()
                    } else {
                        json_response(
                            StatusCode::OK,
                            r#"{"access_token":"at_n","refresh_token":"rt_n","token_type":"DPoP","expires_in":3600,"scope":"atproto"}"#,
                        )
                    }
                }
            }),
        );
        let (base, _shutdown) = spawn(app).await;

        let mut client = AdminOAuthClient::new(CLIENT_ID.to_string(), base).unwrap();
        let tokens = client
            .exchange_code_for_tokens("c", "v", REDIRECT_URI)
            .await
            .expect("retry after nonce challenge succeeds");
        assert_eq!(tokens.access_token, "at_n");

        // Exactly one retry (two calls total).
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let captured = cap.lock().unwrap().clone();
        assert_eq!(captured.len(), 2);
        // First proof had no nonce; the retry proof carries the server nonce.
        let (_h0, c0) = split_jwt(captured[0].dpop.as_deref().unwrap());
        assert!(c0.get("nonce").is_none(), "first proof must not carry a nonce");
        let (_h1, c1) = split_jwt(captured[1].dpop.as_deref().unwrap());
        assert_eq!(c1["nonce"], "srv-nonce-1", "retry proof must carry the nonce");
    }

    #[tokio::test]
    async fn as_error_response_surfaces_as_server_error() {
        let app = Router::new().route(
            "/oauth/atproto/token",
            post(|| async {
                json_response(
                    StatusCode::BAD_REQUEST,
                    r#"{"error":"invalid_grant","error_description":"code expired"}"#,
                )
            }),
        );
        let (base, _shutdown) = spawn(app).await;

        let mut client = AdminOAuthClient::new(CLIENT_ID.to_string(), base).unwrap();
        let err = client
            .exchange_code_for_tokens("bad", "v", REDIRECT_URI)
            .await
            .expect_err("invalid_grant must surface as an error");
        match err {
            OAuthClientError::Server {
                status,
                error,
                description,
            } => {
                assert_eq!(status, 400);
                assert_eq!(error, "invalid_grant");
                assert_eq!(description, "code expired");
            }
            other => panic!("expected Server error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transport_failure_surfaces_as_http_error() {
        // Nothing is listening on this port after shutdown fires.
        let app = Router::new().route("/oauth/atproto/token", post(|| async { StatusCode::OK }));
        let (base, shutdown) = spawn(app).await;
        let _ = shutdown.send(()); // stop the server
        // Give the listener a moment to drop by making the request; connect fails.
        let mut client = AdminOAuthClient::new(CLIENT_ID.to_string(), base).unwrap();
        let err = client
            .refresh_tokens("rt")
            .await
            .expect_err("connect to a dead server must fail");
        assert!(
            matches!(err, OAuthClientError::Http(_)),
            "expected Http error, got {err:?}"
        );
    }
}
