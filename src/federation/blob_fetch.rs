//! Origin-PDS blob fetch primitive (Arc 16f §9.6.3.3).
//!
//! Used by the Arc 16f importRepo handler's fetch-and-retry loop
//! (§9.6.3.5, landing in Step 3) to retrieve a blob that the local
//! Phase B signalled as `NeedsFetch`. The primitive owns the
//! per-CID inner retry budget (5xx / network / timeout with
//! exponential backoff); the caller loop in Step 3 owns the outer
//! round budget that drains multiple `NeedsFetch` CIDs.
//!
//! ## CF5 anonymity invariant
//!
//! This primitive sends **no `Authorization` header**. CF5 recon
//! against bsky-PDS reference SHA `5af5deff55d7f0027f5fddc0f2b53e330d13b43a`
//! (skydeval, 2026-05-21) established firsthand that
//! `com.atproto.sync.getBlob` is anonymous-by-design — the verifier
//! uses `authorizationOrAdminTokenOptional` with an unconditional
//! `authorize` callback and falls through to `unauthenticated(ctx)`
//! when no bearer/basic auth is present
//! ([`auth-verifier.ts:275-292`]). Service-auth JWTs are actively
//! REJECTED on the bearer path
//! ([`auth-verifier.ts:486-493`]: `Malformed token` is thrown for
//! any payload carrying an `lxm` claim — i.e. every ATProto
//! service-auth JWT). Sending a JWT would 400 the request, not
//! silently no-op.
//!
//! If a future protocol revision adds required auth to getBlob,
//! this primitive grows a JWT branch — see chainlink #113 CF5
//! recon at `docs/internal/v05-recon/V05_ARC16F_CF5_RECON.md`.
//!
//! [`auth-verifier.ts:275-292`]: https://github.com/bluesky-social/atproto/blob/5af5deff55d7f0027f5fddc0f2b53e330d13b43a/packages/pds/src/auth-verifier.ts#L275
//! [`auth-verifier.ts:486-493`]: https://github.com/bluesky-social/atproto/blob/5af5deff55d7f0027f5fddc0f2b53e330d13b43a/packages/pds/src/auth-verifier.ts#L486
//!
//! ## Test seam
//!
//! [`OriginBlobFetcher`] is the §9.6.4 Step 2.4 trait shim. The
//! production impl [`HttpOriginBlobFetcher`] delegates to
//! [`fetch_blob_from_origin`]; Step 3's caller loop and Phase B can
//! substitute a mock implementation to inject 4xx / 5xx / timeout
//! / oversize outcomes without standing up a live origin PDS.

#![allow(dead_code)] // Wired into the importRepo handler at Arc 16f Step 3.

use crate::context::AppContext;
use crate::error::{PdsError, PdsResult};
use crate::federation::authentication::extract_pds_endpoint;
use crate::identity::IdentityResolverApi;
use async_trait::async_trait;
use proto_blue::lex_data::Cid;
use std::time::Duration;
use tracing::{debug, warn};

/// Default backoff base for the inner per-CID retry budget. Per
/// §9.6.3.3 step 6: `1s / 2s / 4s` for attempts 1/2/3. The base
/// is `2 ** attempt_index * backoff_base`, starting at index 0
/// for the first retry.
pub const DEFAULT_BACKOFF_BASE: Duration = Duration::from_secs(1);

/// Fetch a single blob from the importing repo's origin PDS.
///
/// Composition per Arc 16f §9.6.3.3 (CF5-amended — step 3 JWT
/// minting struck; sends no `Authorization` header):
///
/// 1. Resolve `importing_did` → DID document via
///    [`IdentityResolverApi::resolve_did`].
/// 2. Extract origin PDS endpoint via
///    [`extract_pds_endpoint`].
/// 3. ~~Service-auth JWT minting~~ — STRUCK per CF5. Anonymous GET.
/// 4. HEAD `/xrpc/com.atproto.sync.getBlob?did=…&cid=…` to read
///    `Content-Length`; reject with [`PdsError::BlobTooLarge`] if
///    the announced size exceeds
///    `service.max_blob_fetch_size`. If the origin returns no
///    `Content-Length` on HEAD (some S3-fronted origins omit it,
///    or 405 the HEAD entirely), fall through to enforcing the
///    cap during streaming GET body read.
/// 5. GET the same URL with no auth header.
/// 6. Response classification:
///    - `2xx` → return the body bytes (capped at `max_blob_fetch_size`).
///    - `4xx` → [`PdsError::OriginFetchClientError`]. **No retry**
///      (durable failure — 404 means origin doesn't have it).
///    - `5xx` / network / timeout → retry with exponential
///      backoff (`backoff_base * 2^attempt_index`), up to
///      `service.blob_fetch_max_retries` retries. Exhaustion
///      surfaces as [`PdsError::OriginFetchClientError`] with a
///      reason describing the terminal-attempt failure (callers
///      in §9.6.3.5 aggregate cross-CID into
///      [`PdsError::OriginFetchExhausted`]).
///    - Body-cap exceeded mid-stream → [`PdsError::BlobTooLarge`].
///
/// The `client` is constructed by the Step 3 caller per round-1
/// F11 — single pooled [`reqwest::Client`] per handler invocation,
/// configured with `service.blob_fetch_timeout_seconds` as the
/// per-attempt timeout.
pub async fn fetch_blob_from_origin(
    ctx: &AppContext,
    client: &reqwest::Client,
    importing_did: &str,
    cid: &Cid,
) -> PdsResult<Vec<u8>> {
    fetch_blob_inner(
        ctx.identity_resolver.as_ref(),
        client,
        importing_did,
        cid,
        ctx.config.service.max_blob_fetch_size,
        ctx.config.service.blob_fetch_max_retries,
        DEFAULT_BACKOFF_BASE,
    )
    .await
}

/// §9.6.4 Step 2.4 test seam. The production implementation
/// ([`HttpOriginBlobFetcher`]) delegates to
/// [`fetch_blob_from_origin`]; Step 3's caller loop and Phase B
/// can swap in a mock to script per-CID outcomes without a live
/// origin PDS.
#[async_trait]
pub trait OriginBlobFetcher: Send + Sync {
    async fn fetch(
        &self,
        ctx: &AppContext,
        client: &reqwest::Client,
        importing_did: &str,
        cid: &Cid,
    ) -> PdsResult<Vec<u8>>;
}

/// Production [`OriginBlobFetcher`] — thin wrapper over
/// [`fetch_blob_from_origin`].
pub struct HttpOriginBlobFetcher;

#[async_trait]
impl OriginBlobFetcher for HttpOriginBlobFetcher {
    async fn fetch(
        &self,
        ctx: &AppContext,
        client: &reqwest::Client,
        importing_did: &str,
        cid: &Cid,
    ) -> PdsResult<Vec<u8>> {
        fetch_blob_from_origin(ctx, client, importing_did, cid).await
    }
}

/// Testable inner implementation — takes the identity-resolver
/// surface and per-request knobs directly instead of going through
/// [`AppContext`], so unit tests can construct a
/// `MockIdentityResolver` and drive the primitive without a full
/// `AppContext` fixture.
///
/// `max_size` is `service.max_blob_fetch_size`; `max_retries` is
/// `service.blob_fetch_max_retries`; `backoff_base` is normally
/// [`DEFAULT_BACKOFF_BASE`] but tests pass a small value
/// (e.g. `Duration::from_millis(5)`) so the retry-path tests do
/// not pay seconds-of-wall-clock per case.
pub(crate) async fn fetch_blob_inner(
    identity_resolver: &dyn IdentityResolverApi,
    client: &reqwest::Client,
    importing_did: &str,
    cid: &Cid,
    max_size: u64,
    max_retries: u32,
    backoff_base: Duration,
) -> PdsResult<Vec<u8>> {
    let did_doc = identity_resolver
        .resolve_did(importing_did)
        .await
        .map_err(|e| PdsError::OriginFetchClientError {
            cid: cid.clone(),
            status_or_reason: format!("could not resolve origin DID: {}", e),
        })?;

    let endpoint = extract_pds_endpoint(&did_doc).ok_or_else(|| {
        PdsError::OriginFetchClientError {
            cid: cid.clone(),
            status_or_reason: "could not extract origin PDS endpoint from DID document"
                .to_string(),
        }
    })?;

    let url = format!(
        "{}/xrpc/com.atproto.sync.getBlob?did={}&cid={}",
        endpoint.trim_end_matches('/'),
        urlencoding::encode(importing_did),
        urlencoding::encode(&cid.to_string()),
    );

    head_size_check(client, &url, cid, max_size).await?;

    // Inner retry loop. Total attempts = 1 + max_retries. Only
    // transient failures (5xx / network / timeout) advance the
    // attempt counter; 4xx is durable and returns immediately.
    let mut attempt: u32 = 0;
    loop {
        match do_get_attempt(client, &url, cid, max_size).await {
            Ok(bytes) => {
                debug!(
                    cid = %cid,
                    bytes = bytes.len(),
                    "origin blob fetch ok"
                );
                return Ok(bytes);
            }
            Err(err @ PdsError::OriginFetchClientError { .. }) => {
                // 4xx — durable. No retry.
                return Err(err);
            }
            Err(err @ PdsError::BlobTooLarge { .. }) => {
                // Body cap tripped mid-stream — durable.
                return Err(err);
            }
            Err(transient) => {
                let reason = transient.to_string();
                if attempt >= max_retries {
                    warn!(
                            cid = %cid,
                        attempts = attempt + 1,
                        last = %reason,
                        "origin blob fetch exhausted retries"
                    );
                    return Err(PdsError::OriginFetchClientError {
                        cid: cid.clone(),
                        status_or_reason: format!(
                            "exhausted after {} attempts: {}",
                            attempt + 1,
                            reason
                        ),
                    });
                }
                let sleep = backoff_base * (1u32 << attempt);
                debug!(
                    cid = %cid,
                    attempt = attempt + 1,
                    sleep_ms = sleep.as_millis() as u64,
                    last = %reason,
                    "origin blob fetch transient, retrying"
                );
                tokio::time::sleep(sleep).await;
                attempt += 1;
            }
        }
    }
}

/// HEAD-then-cap pre-fetch size check (§9.6.3.3 step 4). HEAD
/// failure or absent `Content-Length` falls back to the
/// body-stream cap inside [`do_get_attempt`] — see the
/// `head_response_without_content_length_falls_back_to_body_cap`
/// test for the contract.
async fn head_size_check(
    client: &reqwest::Client,
    url: &str,
    cid: &Cid,
    max_size: u64,
) -> PdsResult<()> {
    let resp = match client.head(url).send().await {
        Ok(r) => r,
        Err(e) => {
            debug!(
                cid = %cid,
                err = %e,
                "HEAD request failed; deferring size check to body-cap path"
            );
            return Ok(());
        }
    };

    let status = resp.status();
    if !status.is_success() {
        debug!(
            cid = %cid,
            %status,
            "HEAD returned non-success; deferring size check to body-cap path"
        );
        return Ok(());
    }

    if let Some(len_hdr) = resp.headers().get(reqwest::header::CONTENT_LENGTH) {
        if let Ok(len_str) = len_hdr.to_str() {
            if let Ok(len) = len_str.parse::<u64>() {
                if len > max_size {
                    return Err(PdsError::BlobTooLarge {
                        cid: cid.clone(),
                        size: len,
                    });
                }
            }
        }
    }
    Ok(())
}

/// One GET attempt with streaming body-cap enforcement.
///
/// Returns:
/// - `Ok(bytes)` on 2xx with body within `max_size`.
/// - `Err(OriginFetchClientError)` on 4xx (durable — no retry).
/// - `Err(BlobTooLarge)` if the body stream exceeds `max_size`
///   (durable — no retry; the origin lied about size or omitted
///   Content-Length).
/// - `Err(OriginFetchClientError)` (used as a transient marker
///   when carrying a 5xx/network/timeout reason) for the retry
///   path to consume. The variant choice is a pragmatic reuse
///   per the Step 1 error vocabulary; the retry loop branches
///   on whether the inner status was 4xx (durable) vs 5xx
///   (transient) before deciding whether to retry.
async fn do_get_attempt(
    client: &reqwest::Client,
    url: &str,
    cid: &Cid,
    max_size: u64,
) -> PdsResult<Vec<u8>> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| transient_error(cid, format!("GET send failed: {}", e)))?;

    let status = resp.status();
    if status.is_client_error() {
        return Err(PdsError::OriginFetchClientError {
            cid: cid.clone(),
            status_or_reason: format!("origin returned {}", status),
        });
    }
    if status.is_server_error() {
        return Err(transient_error(cid, format!("origin returned {}", status)));
    }
    if !status.is_success() {
        return Err(transient_error(
            cid,
            format!("origin returned unexpected status {}", status),
        ));
    }

    let mut resp = resp;
    let mut acc: Vec<u8> = Vec::new();
    loop {
        let chunk = resp
            .chunk()
            .await
            .map_err(|e| transient_error(cid, format!("body read failed: {}", e)))?;
        let Some(chunk) = chunk else { break };
        let next_len = acc.len() as u64 + chunk.len() as u64;
        if next_len > max_size {
            return Err(PdsError::BlobTooLarge {
                cid: cid.clone(),
                size: next_len,
            });
        }
        acc.extend_from_slice(&chunk);
    }
    Ok(acc)
}

/// Build a transient-class error carrying a reason. The outer
/// retry loop in [`fetch_blob_inner`] re-classifies these as
/// durable-after-exhaustion if `max_retries` is exceeded.
///
/// Implementation note for skydeval: Step 1's error vocabulary
/// doesn't include a separate "transient" variant; we reuse
/// `OriginFetchClientError`'s `status_or_reason` string for
/// in-band signalling between [`do_get_attempt`] and the retry
/// loop. The loop only treats `OriginFetchClientError` as
/// durable when [`do_get_attempt`] tagged it from a 4xx
/// response. If a refactor introduces a distinct transient
/// variant later, narrow this seam — search for
/// `transient_error` to find all call sites.
fn transient_error(cid: &Cid, reason: String) -> PdsError {
    PdsError::Internal(format!("origin-fetch transient ({}): {}", cid, reason))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::resolver::test_doubles::MockIdentityResolver;
    use crate::identity::did_document::DidDocument;
    use axum::{
        body::Body,
        extract::{Query, State},
        http::{HeaderMap, Method, StatusCode},
        response::Response,
        routing::any,
        Router,
    };
    use serde::Deserialize;
    use std::collections::VecDeque;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::sync::Mutex;

    fn test_cid() -> Cid {
        Cid::from_str("bafkreigh2akiscaildcqabsyg3dfr6chu3fgpregiymsck7e7aqa4s52zy")
            .expect("valid CIDv1 raw multibase")
    }

    fn did_doc_pointing_at(url: &str) -> DidDocument {
        let raw = serde_json::json!({
            "id": "did:plc:alice",
            "service": [
                {
                    "id": "#atproto_pds",
                    "type": "AtprotoPersonalDataServer",
                    "serviceEndpoint": url,
                }
            ]
        });
        serde_json::from_value(raw).expect("synthetic DID doc deserialises")
    }

    /// One scripted response from the test origin server.
    #[derive(Clone)]
    enum ScriptedResponse {
        Ok {
            body: Vec<u8>,
            include_content_length: bool,
        },
        Status(u16),
        SleepThenOk {
            body: Vec<u8>,
            sleep: Duration,
        },
        NoSuchRoute, // 404 for unrouted requests; used implicitly
    }

    /// Stateful test origin. Each test pushes scripted responses
    /// (one per HEAD and one per GET, in order); the server pops
    /// per request and records every inbound request for the
    /// CF5 anonymity assertion.
    #[derive(Default)]
    struct OriginScript {
        head_responses: Mutex<VecDeque<ScriptedResponse>>,
        get_responses: Mutex<VecDeque<ScriptedResponse>>,
        observed_requests: Mutex<Vec<RequestObservation>>,
    }

    #[derive(Clone, Debug)]
    struct RequestObservation {
        method: String,
        path: String,
        query: String,
        authorization_header: Option<String>,
    }

    #[derive(Deserialize)]
    struct GetBlobQuery {
        did: String,
        cid: String,
    }

    async fn handle_get_blob(
        State(script): State<Arc<OriginScript>>,
        method: Method,
        headers: HeaderMap,
        Query(q): Query<GetBlobQuery>,
    ) -> Response {
        let auth = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok().map(String::from));
        script.observed_requests.lock().unwrap().push(RequestObservation {
            method: method.as_str().to_string(),
            path: "/xrpc/com.atproto.sync.getBlob".to_string(),
            query: format!("did={}&cid={}", q.did, q.cid),
            authorization_header: auth,
        });

        let resp = if method == Method::HEAD {
            script.head_responses.lock().unwrap().pop_front()
        } else {
            script.get_responses.lock().unwrap().pop_front()
        };
        let resp = resp.unwrap_or(ScriptedResponse::Status(500));

        match resp {
            ScriptedResponse::Ok { body, include_content_length } => {
                let content_len = body.len();
                let mut builder = Response::builder().status(StatusCode::OK);
                if include_content_length {
                    builder = builder.header("content-length", content_len);
                }
                if method == Method::HEAD {
                    builder.body(Body::empty()).unwrap()
                } else {
                    builder.body(Body::from(body)).unwrap()
                }
            }
            ScriptedResponse::Status(code) => {
                let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                Response::builder().status(status).body(Body::empty()).unwrap()
            }
            ScriptedResponse::SleepThenOk { body, sleep } => {
                tokio::time::sleep(sleep).await;
                let content_len = body.len();
                let mut builder = Response::builder()
                    .status(StatusCode::OK)
                    .header("content-length", content_len);
                if method == Method::HEAD {
                    builder = builder.header("content-length", content_len);
                    builder.body(Body::empty()).unwrap()
                } else {
                    builder.body(Body::from(body)).unwrap()
                }
            }
            ScriptedResponse::NoSuchRoute => {
                Response::builder().status(StatusCode::NOT_FOUND).body(Body::empty()).unwrap()
            }
        }
    }

    struct TestOrigin {
        url: String,
        script: Arc<OriginScript>,
        _shutdown: tokio::sync::oneshot::Sender<()>,
    }

    impl TestOrigin {
        async fn start() -> Self {
            let script = Arc::new(OriginScript::default());
            let app: Router = Router::new()
                .route(
                    "/xrpc/com.atproto.sync.getBlob",
                    any(handle_get_blob),
                )
                .with_state(script.clone());

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (tx, rx) = tokio::sync::oneshot::channel::<()>();
            let serve = async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = rx.await;
                    })
                    .await
                    .ok();
            };
            tokio::spawn(serve);

            TestOrigin {
                url: format!("http://{}", addr),
                script,
                _shutdown: tx,
            }
        }

        fn script_head(&self, r: ScriptedResponse) {
            self.script.head_responses.lock().unwrap().push_back(r);
        }
        fn script_get(&self, r: ScriptedResponse) {
            self.script.get_responses.lock().unwrap().push_back(r);
        }
        fn observed(&self) -> Vec<RequestObservation> {
            self.script.observed_requests.lock().unwrap().clone()
        }
    }

    fn resolver_for(server: &TestOrigin) -> MockIdentityResolver {
        let r = MockIdentityResolver::new();
        r.script_did("did:plc:alice", did_doc_pointing_at(&server.url));
        r
    }

    fn test_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap()
    }

    // ============================================================
    // Step 2.5 — success & response classification
    // ============================================================

    #[tokio::test]
    async fn success_returns_body_bytes() {
        let server = TestOrigin::start().await;
        server.script_head(ScriptedResponse::Ok {
            body: Vec::new(),
            include_content_length: true,
        });
        server.script_get(ScriptedResponse::Ok {
            body: b"hello blob".to_vec(),
            include_content_length: true,
        });

        let resolver = resolver_for(&server);
        let client = test_client();
        let bytes = fetch_blob_inner(
            &resolver,
            &client,
            "did:plc:alice",
            &test_cid(),
            50_000_000,
            3,
            Duration::from_millis(5),
        )
        .await
        .expect("success");

        assert_eq!(bytes, b"hello blob");
    }

    #[tokio::test]
    async fn cf5_invariant_outbound_request_has_no_authorization_header() {
        let server = TestOrigin::start().await;
        server.script_head(ScriptedResponse::Ok {
            body: Vec::new(),
            include_content_length: true,
        });
        server.script_get(ScriptedResponse::Ok {
            body: b"hello".to_vec(),
            include_content_length: true,
        });

        let resolver = resolver_for(&server);
        let client = test_client();
        let _ = fetch_blob_inner(
            &resolver,
            &client,
            "did:plc:alice",
            &test_cid(),
            50_000_000,
            3,
            Duration::from_millis(5),
        )
        .await
        .unwrap();

        let observed = server.observed();
        assert!(
            observed.iter().any(|r| r.method == "HEAD"),
            "HEAD request expected"
        );
        assert!(
            observed.iter().any(|r| r.method == "GET"),
            "GET request expected"
        );
        for req in &observed {
            assert!(
                req.authorization_header.is_none(),
                "CF5 invariant violated: {} request to {} carried Authorization header: {:?}",
                req.method,
                req.path,
                req.authorization_header,
            );
        }
    }

    #[tokio::test]
    async fn client_4xx_is_durable_no_retry() {
        let server = TestOrigin::start().await;
        server.script_head(ScriptedResponse::Status(404));
        server.script_get(ScriptedResponse::Status(404));

        let resolver = resolver_for(&server);
        let client = test_client();
        let err = fetch_blob_inner(
            &resolver,
            &client,
            "did:plc:alice",
            &test_cid(),
            50_000_000,
            3,
            Duration::from_millis(5),
        )
        .await
        .unwrap_err();

        match err {
            PdsError::OriginFetchClientError { status_or_reason, .. } => {
                assert!(
                    status_or_reason.contains("404"),
                    "reason should mention status 404: {}",
                    status_or_reason
                );
                assert!(
                    !status_or_reason.contains("exhausted"),
                    "4xx should not be classified as exhausted: {}",
                    status_or_reason
                );
            }
            other => panic!("expected OriginFetchClientError, got {:?}", other),
        }

        // HEAD (non-success → fall-through) + exactly ONE GET attempt.
        let gets = server
            .observed()
            .iter()
            .filter(|r| r.method == "GET")
            .count();
        assert_eq!(gets, 1, "4xx must not retry; got {} GET attempts", gets);
    }

    #[tokio::test]
    async fn server_5xx_retries_then_exhausts() {
        let server = TestOrigin::start().await;
        // HEAD non-success → defer to body-cap path.
        server.script_head(ScriptedResponse::Status(500));
        // Four GET attempts (1 initial + 3 retries), all 503.
        for _ in 0..4 {
            server.script_get(ScriptedResponse::Status(503));
        }

        let resolver = resolver_for(&server);
        let client = test_client();
        let err = fetch_blob_inner(
            &resolver,
            &client,
            "did:plc:alice",
            &test_cid(),
            50_000_000,
            3,
            Duration::from_millis(5),
        )
        .await
        .unwrap_err();

        match err {
            PdsError::OriginFetchClientError { status_or_reason, .. } => {
                assert!(
                    status_or_reason.contains("exhausted"),
                    "should be tagged as exhausted: {}",
                    status_or_reason
                );
                assert!(
                    status_or_reason.contains("4 attempts"),
                    "should report 4 total attempts: {}",
                    status_or_reason
                );
            }
            other => panic!("expected OriginFetchClientError exhausted, got {:?}", other),
        }

        let gets = server
            .observed()
            .iter()
            .filter(|r| r.method == "GET")
            .count();
        assert_eq!(gets, 4, "expected 4 GET attempts (1 + 3 retries); got {}", gets);
    }

    #[tokio::test]
    async fn timeout_classified_as_transient_retries() {
        let server = TestOrigin::start().await;
        server.script_head(ScriptedResponse::Status(405));
        // First GET sleeps 10s — well beyond the 200ms client timeout.
        server.script_get(ScriptedResponse::SleepThenOk {
            body: b"never delivered".to_vec(),
            sleep: Duration::from_secs(10),
        });
        // Second GET returns quickly.
        server.script_get(ScriptedResponse::Ok {
            body: b"recovered".to_vec(),
            include_content_length: true,
        });

        let resolver = resolver_for(&server);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap();
        let bytes = fetch_blob_inner(
            &resolver,
            &client,
            "did:plc:alice",
            &test_cid(),
            50_000_000,
            3,
            Duration::from_millis(5),
        )
        .await
        .expect("retry should recover");

        assert_eq!(bytes, b"recovered");

        let gets = server
            .observed()
            .iter()
            .filter(|r| r.method == "GET")
            .count();
        assert_eq!(gets, 2, "expected 1 timed-out GET + 1 recovery GET; got {}", gets);
    }

    #[tokio::test]
    async fn head_reveals_oversized_blob_pre_download() {
        let server = TestOrigin::start().await;
        // HEAD response: content-length 99 bytes against a 50-byte cap.
        server.script_head(ScriptedResponse::Ok {
            body: vec![0u8; 99],
            include_content_length: true,
        });
        // No GET response scripted — assertion will catch any GET attempt.

        let resolver = resolver_for(&server);
        let client = test_client();
        let err = fetch_blob_inner(
            &resolver,
            &client,
            "did:plc:alice",
            &test_cid(),
            50,
            3,
            Duration::from_millis(5),
        )
        .await
        .unwrap_err();

        match err {
            PdsError::BlobTooLarge { size, .. } => assert_eq!(size, 99),
            other => panic!("expected BlobTooLarge, got {:?}", other),
        }

        let gets = server
            .observed()
            .iter()
            .filter(|r| r.method == "GET")
            .count();
        assert_eq!(
            gets, 0,
            "HEAD pre-check must short-circuit before any GET; got {}",
            gets
        );
    }

    #[tokio::test]
    async fn head_response_without_content_length_falls_back_to_body_cap() {
        let server = TestOrigin::start().await;
        // HEAD returns 200 but with no content-length.
        server.script_head(ScriptedResponse::Ok {
            body: Vec::new(),
            include_content_length: false,
        });
        // GET returns 100 bytes (without Content-Length) against a 50-byte cap.
        server.script_get(ScriptedResponse::Ok {
            body: vec![0u8; 100],
            include_content_length: false,
        });

        let resolver = resolver_for(&server);
        let client = test_client();
        let err = fetch_blob_inner(
            &resolver,
            &client,
            "did:plc:alice",
            &test_cid(),
            50,
            3,
            Duration::from_millis(5),
        )
        .await
        .unwrap_err();

        match err {
            PdsError::BlobTooLarge { size, .. } => {
                assert!(size > 50, "size should exceed cap: {}", size);
            }
            other => panic!("expected BlobTooLarge from body-cap, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn did_resolution_failure_propagates() {
        // No scripted DID — MockIdentityResolver returns
        // IdentityResolution error.
        let server = TestOrigin::start().await;
        let resolver = MockIdentityResolver::new();
        let client = test_client();
        let err = fetch_blob_inner(
            &resolver,
            &client,
            "did:plc:unscripted",
            &test_cid(),
            50_000_000,
            3,
            Duration::from_millis(5),
        )
        .await
        .unwrap_err();

        match err {
            PdsError::OriginFetchClientError { status_or_reason, .. } => {
                assert!(
                    status_or_reason.contains("resolve origin DID"),
                    "reason should mention DID resolution: {}",
                    status_or_reason
                );
            }
            other => panic!("expected OriginFetchClientError, got {:?}", other),
        }

        // No HTTP requests should have been made.
        assert!(
            server.observed().is_empty(),
            "no requests should be issued when DID resolution fails"
        );
    }

    #[tokio::test]
    async fn pds_endpoint_extraction_failure_propagates() {
        let resolver = MockIdentityResolver::new();
        // DID doc with NO AtprotoPersonalDataServer service entry.
        let raw = serde_json::json!({
            "id": "did:plc:alice",
            "service": [
                { "id": "#x", "type": "OtherService", "serviceEndpoint": "https://other.example.com" }
            ]
        });
        let doc: DidDocument = serde_json::from_value(raw).unwrap();
        resolver.script_did("did:plc:alice", doc);

        let client = test_client();
        let err = fetch_blob_inner(
            &resolver,
            &client,
            "did:plc:alice",
            &test_cid(),
            50_000_000,
            3,
            Duration::from_millis(5),
        )
        .await
        .unwrap_err();

        match err {
            PdsError::OriginFetchClientError { status_or_reason, .. } => {
                assert!(
                    status_or_reason.contains("PDS endpoint"),
                    "reason should mention endpoint extraction: {}",
                    status_or_reason
                );
            }
            other => panic!("expected OriginFetchClientError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn http_origin_blob_fetcher_implements_origin_blob_fetcher_trait() {
        // Type-level check — confirms the trait shim compiles and
        // is dyn-compatible (HttpOriginBlobFetcher will be used
        // behind Arc<dyn OriginBlobFetcher> in Step 3 / Phase B).
        fn assert_dyn_compat<T: OriginBlobFetcher + 'static>() {}
        assert_dyn_compat::<HttpOriginBlobFetcher>();
        let _: Box<dyn OriginBlobFetcher> = Box::new(HttpOriginBlobFetcher);
    }
}
