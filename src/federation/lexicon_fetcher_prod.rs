//! Arc 17 production [`LexiconRecordFetcher`] implementation.
//!
//! Replaces the trait seam Steps 1-3 built around with the real
//! end-to-end pipeline per V05_DESIGN_arc17.md §17.3.1 steps 4-7:
//! PLC-resolve authority DID → extract PDS endpoint → anonymous HTTP
//! GET `com.atproto.sync.getRecord` → parse CAR → walk MST →
//! `LexValue` → JSON.
//!
//! ## Standard door (Phase 1 reuse verdict)
//!
//! `sync.getRecord` returns CAR bytes (`application/vnd.ipld.car`).
//! The parse path is six lines of proto-blue glue, exercised by
//! Aurora's own [`crate::actor_store::car`] tests on the producer
//! side:
//!
//! ```ignore
//! let (root_cid, blocks) = pb_car::read_car_with_root(&car_bytes)?;
//! let storage = Arc::new(MemoryBlockstore::from_blocks(blocks, Some(root_cid)));
//! let repo = Repo::load(storage)?;
//! let lex_value = repo.get_record(LEXICON_COLLECTION, nsid)?.ok_or(...)?;
//! let json_value = lex_to_json(&lex_value);
//! let json_string = serde_json::to_string(&json_value)?;
//! ```
//!
//! ## Signature verification deferral
//!
//! `Repo::load` parses the signed commit via
//! `SignedCommit::from_lex_value` but does NOT call
//! `verify_commit_sig` — exactly the §17.7 / §17.3.9 v0.6+ deferral.
//! Adding the verify call is a one-liner against
//! `get_signing_key(authority_did)` once a future cycle ships it.
//!
//! ## One-root CAR assumption
//!
//! [`pb_car::read_car_with_root`] errors when `roots.len() != 1`.
//! Aurora's [`crate::actor_store::car::export_record_to_car`] emits
//! exactly one root (the commit CID); bsky-PDS at the Step 0.0a
//! reference SHA emits the same shape (verified). A hypothetical
//! future bsky-PDS revision that ships multi-root CARs would need a
//! fallback to [`pb_car::read_car`] (returns `Vec<Cid>`) plus
//! commit-shape inspection to pick the right root. Chainlink covers
//! the assumption for forensic-trail discoverability.

use crate::error::PdsError;
use crate::federation::authentication::extract_pds_endpoint;
use crate::federation::lexicon_resolver::{LexiconFetcherError, LexiconRecordFetcher};
use crate::identity::IdentityResolverApi;
use async_trait::async_trait;
use proto_blue::repo::{
    car as pb_car,
    storage::{MemoryBlockstore, RepoStorage},
    Repo,
};
use std::sync::Arc;

/// Collection NSID for ATProto lexicon records per §17.3.1 step 6.
pub const LEXICON_COLLECTION: &str = "com.atproto.lexicon.schema";

/// Production fetcher backed by Aurora's [`IdentityResolverApi`] +
/// a pooled [`reqwest::Client`]. Constructed once at [`AppContext`]
/// startup when `config.lexicon.enabled` is true (see
/// [`crate::context::AppContext::new`] wiring) and shared via `Arc`
/// across every lexicon fetch.
///
/// The HTTP client's per-request timeout is set at construction time
/// from `config.lexicon.fetch_timeout_secs`; the resolver's
/// retry-budget knob `config.lexicon.fetch_max_retries` is honored
/// at the resolver layer (§17.3.1 step 6 bounded retry), not here.
pub struct ProductionLexiconFetcher {
    identity_resolver: Arc<dyn IdentityResolverApi>,
    http_client: reqwest::Client,
}

impl ProductionLexiconFetcher {
    pub fn new(
        identity_resolver: Arc<dyn IdentityResolverApi>,
        http_client: reqwest::Client,
    ) -> Self {
        Self {
            identity_resolver,
            http_client,
        }
    }
}

#[async_trait]
impl LexiconRecordFetcher for ProductionLexiconFetcher {
    async fn fetch(
        &self,
        authority_did: &str,
        nsid: &str,
    ) -> Result<String, LexiconFetcherError> {
        // §17.3.1 step 4 — PLC resolution. Arc 13 v4.2's typed
        // `DidTombstoned` variant routes structurally into the
        // `authority_tombstoned` failure_class; other resolution
        // failures collapse into `did_fail`. The match-on-PdsError
        // here is the consumer side of the v4.2 contract.
        let did_doc = self
            .identity_resolver
            .resolve_did(authority_did)
            .await
            .map_err(|e| match e {
                PdsError::DidTombstoned(d) => LexiconFetcherError::AuthorityTombstoned(d),
                other => LexiconFetcherError::DidResolutionFailed {
                    did: authority_did.to_string(),
                    detail: other.to_string(),
                },
            })?;

        // §17.3.1 step 5 — extract PDS endpoint. Reuses Arc 16f's
        // already-shared free function (federation/authentication.rs).
        // No service entry → `pds_unreachable` (the DID document
        // doesn't tell us where to fetch from).
        let pds_endpoint = extract_pds_endpoint(&did_doc).ok_or_else(|| {
            LexiconFetcherError::PdsUnreachable(format!(
                "no AtprotoPersonalDataServer service entry in DID document for {authority_did}"
            ))
        })?;

        // §17.3.1 step 6 — anonymous HTTP GET. URL encoding is
        // load-bearing: NSIDs are `[a-z0-9-.]+` (no URL-unsafe chars
        // today), but encoding via urlencoding::encode keeps the
        // construction safe under any future NSID-spec relaxation.
        let url = format!(
            "{}/xrpc/com.atproto.sync.getRecord?did={}&collection={}&rkey={}",
            pds_endpoint.trim_end_matches('/'),
            urlencoding::encode(authority_did),
            urlencoding::encode(LEXICON_COLLECTION),
            urlencoding::encode(nsid),
        );

        let response = self.http_client.get(&url).send().await.map_err(|e| {
            // Order: timeout-check first (subset of is_connect on some
            // platforms otherwise), connect-check second, generic
            // transport last.
            if e.is_timeout() {
                LexiconFetcherError::Timeout
            } else if e.is_connect() {
                LexiconFetcherError::PdsUnreachable(format!("connect to {pds_endpoint} failed: {e}"))
            } else {
                LexiconFetcherError::PdsUnreachable(format!("HTTP request to {pds_endpoint} failed: {e}"))
            }
        })?;

        let status = response.status();
        if status.is_client_error() {
            return Err(LexiconFetcherError::Http4xx(format!(
                "{status} fetching {LEXICON_COLLECTION}/{nsid} from {pds_endpoint}"
            )));
        }
        if status.is_server_error() {
            return Err(LexiconFetcherError::Http5xx(format!(
                "{status} fetching {LEXICON_COLLECTION}/{nsid} from {pds_endpoint}"
            )));
        }
        if !status.is_success() {
            // 1xx / 3xx surface uncommonly; reqwest follows redirects
            // by default so 3xx shouldn't reach us, but be defensive.
            return Err(LexiconFetcherError::Http4xx(format!(
                "unexpected status {status} fetching {LEXICON_COLLECTION}/{nsid}"
            )));
        }

        let car_bytes = response.bytes().await.map_err(|e| {
            if e.is_timeout() {
                LexiconFetcherError::Timeout
            } else {
                LexiconFetcherError::PdsUnreachable(format!(
                    "body read from {pds_endpoint} failed: {e}"
                ))
            }
        })?;

        // §17.3.1 step 7 — CAR parse path (Phase 1 pinned).
        //
        // `read_car_with_root` enforces `roots.len() == 1`. Both
        // Aurora's emitter (`actor_store::car::export_record_to_car`)
        // and bsky-PDS at the Step 0.0a reference SHA emit exactly
        // one root (the commit CID). A future bsky-PDS revision that
        // ships multi-root CARs would need a fallback to
        // `pb_car::read_car` (returns `Vec<Cid>`) and commit-shape
        // inspection to pick the right root. The single-root
        // assumption is intentional and tracked.
        let (root_cid, blocks) = pb_car::read_car_with_root(&car_bytes).map_err(|e| {
            LexiconFetcherError::InvalidResponseStructure(format!(
                "CAR parse failed (single-root expected, see #46 thread): {e}"
            ))
        })?;

        let storage: Arc<dyn RepoStorage> =
            Arc::new(MemoryBlockstore::from_blocks(blocks, Some(root_cid)));

        // `Repo::load` walks the signed commit + MST. It calls
        // `SignedCommit::from_lex_value` (parse only) but NOT
        // `verify_commit_sig` — signature verification is deferred
        // per §17.7 / §17.3.9. v0.6+ adds the verify call here.
        let repo = Repo::load(storage).map_err(|e| {
            LexiconFetcherError::InvalidResponseStructure(format!(
                "Repo::load failed (commit/MST structure invalid): {e}"
            ))
        })?;

        // MST lookup. `Ok(None)` means the CAR was structurally
        // valid but didn't contain a record at `(collection, rkey)` —
        // the HTTP fetch succeeded (200) so classifying as http_4xx
        // would misrepresent the transport status. Map to
        // `InvalidResponseStructure` → `failure_class = "invalid_schema"`
        // so log readers see "content problem, not transport".
        let lex_value = repo
            .get_record(LEXICON_COLLECTION, nsid)
            .map_err(|e| {
                LexiconFetcherError::InvalidResponseStructure(format!(
                    "MST lookup failed: {e}"
                ))
            })?
            .ok_or_else(|| {
                LexiconFetcherError::InvalidResponseStructure(format!(
                    "CAR fetched OK but contains no record at {LEXICON_COLLECTION}/{nsid}"
                ))
            })?;

        // Bridge LexValue → serde_json::Value → JSON string. The
        // resolver's step 7 (Lexicons::add) expects a JSON-shaped
        // string and parses via `serde_json::from_str`.
        let json_value = proto_blue::lex_json::lex_to_json(&lex_value);
        serde_json::to_string(&json_value).map_err(|e| {
            LexiconFetcherError::InvalidResponseStructure(format!(
                "lex_to_json roundtrip serialization failed: {e}"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::did_document::DidDocument;
    use axum::body::Body;
    use axum::http::{header, Response, StatusCode};
    use axum::routing::any;
    use axum::Router;
    use std::sync::Mutex;

    /// Minimal mock identity resolver that maps a single
    /// `authority_did` to a configured response. Tests script the
    /// response (Ok DidDocument / Err PdsError) to exercise the
    /// fetcher's PLC-failure classifications.
    struct MockIdentityResolver {
        response: Mutex<Option<PdsResultBox>>,
    }

    enum PdsResultBox {
        Doc(DidDocument),
        Err(PdsError),
    }

    impl MockIdentityResolver {
        fn with_doc(doc: DidDocument) -> Self {
            Self {
                response: Mutex::new(Some(PdsResultBox::Doc(doc))),
            }
        }
        fn with_err(err: PdsError) -> Self {
            Self {
                response: Mutex::new(Some(PdsResultBox::Err(err))),
            }
        }
    }

    #[async_trait]
    impl IdentityResolverApi for MockIdentityResolver {
        async fn resolve_did(&self, _did: &str) -> crate::error::PdsResult<DidDocument> {
            let taken = self.response.lock().unwrap().take();
            match taken {
                Some(PdsResultBox::Doc(d)) => Ok(d),
                Some(PdsResultBox::Err(e)) => Err(e),
                None => Err(PdsError::IdentityResolution("mock exhausted".into())),
            }
        }
        async fn resolve_handle(&self, _handle: &str) -> crate::error::PdsResult<String> {
            unimplemented!("not used by ProductionLexiconFetcher tests")
        }
        async fn get_signing_key(&self, _did: &str) -> crate::error::PdsResult<Vec<u8>> {
            unimplemented!()
        }
        async fn get_handle_for_did(
            &self,
            _did: &str,
        ) -> crate::error::PdsResult<Option<String>> {
            unimplemented!()
        }
        async fn update_handle(&self, _did: &str, _handle: &str) -> crate::error::PdsResult<()> {
            unimplemented!()
        }
        async fn invalidate_handle(&self, _handle: &str) -> crate::error::PdsResult<()> {
            unimplemented!()
        }
        async fn invalidate_did(&self, _did: &str) -> crate::error::PdsResult<()> {
            unimplemented!()
        }
        async fn cleanup_cache(&self) -> crate::error::PdsResult<()> {
            unimplemented!()
        }
    }

    fn doc_pointing_at(endpoint: &str) -> DidDocument {
        // Construct a minimal DID document with the AtprotoPersonalDataServer
        // service entry. Field set matches what extract_pds_endpoint walks.
        let json = serde_json::json!({
            "id": "did:plc:authority",
            "alsoKnownAs": [],
            "verificationMethod": [],
            "service": [
                {
                    "id": "#atproto_pds",
                    "type": "AtprotoPersonalDataServer",
                    "serviceEndpoint": endpoint,
                }
            ],
        });
        serde_json::from_value(json).expect("doc parse")
    }

    /// One-shot stub PDS at 127.0.0.1:0 returning the configured
    /// `Response` for any GET. Mirrors `src/federation/blob_fetch.rs`'s
    /// TestOrigin pattern. Drop the shutdown_tx to terminate.
    struct StubPds {
        url: String,
        _shutdown: tokio::sync::oneshot::Sender<()>,
    }

    impl StubPds {
        async fn start(
            handler: impl Fn() -> Response<Body> + Send + Sync + Clone + 'static,
        ) -> Self {
            let app = Router::new().route(
                "/*path",
                any(move || {
                    let handler = handler.clone();
                    async move { handler() }
                }),
            );

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (tx, rx) = tokio::sync::oneshot::channel::<()>();
            tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = rx.await;
                    })
                    .await
                    .ok();
            });
            Self {
                url: format!("http://{addr}"),
                _shutdown: tx,
            }
        }
    }

    fn test_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap()
    }

    // ──────────────────────────────────────────────────────────────
    // PLC-failure classifications (resolution + tombstone)
    // ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn fetch_classifies_did_tombstoned_into_authority_tombstoned() {
        let mock = Arc::new(MockIdentityResolver::with_err(PdsError::DidTombstoned(
            "did:plc:dead".into(),
        )));
        let fetcher = ProductionLexiconFetcher::new(mock, test_http_client());
        let err = fetcher
            .fetch("did:plc:dead", "com.example.foo.bar")
            .await
            .unwrap_err();
        match err {
            LexiconFetcherError::AuthorityTombstoned(d) => assert_eq!(d, "did:plc:dead"),
            other => panic!("expected AuthorityTombstoned, got {other:?}"),
        }
        assert_eq!(
            LexiconFetcherError::AuthorityTombstoned("x".into()).failure_class(),
            "authority_tombstoned"
        );
    }

    #[tokio::test]
    async fn fetch_classifies_other_plc_errors_into_did_fail() {
        let mock = Arc::new(MockIdentityResolver::with_err(PdsError::IdentityResolution(
            "PLC directory returned error: 500 Internal Server Error".into(),
        )));
        let fetcher = ProductionLexiconFetcher::new(mock, test_http_client());
        let err = fetcher
            .fetch("did:plc:fail", "com.example.foo.bar")
            .await
            .unwrap_err();
        match err {
            LexiconFetcherError::DidResolutionFailed { did, .. } => {
                assert_eq!(did, "did:plc:fail");
            }
            other => panic!("expected DidResolutionFailed, got {other:?}"),
        }
        assert_eq!(
            LexiconFetcherError::DidResolutionFailed {
                did: "x".into(),
                detail: "x".into(),
            }
            .failure_class(),
            "did_fail"
        );
    }

    #[tokio::test]
    async fn fetch_classifies_missing_pds_endpoint_into_pds_unreachable() {
        // DID document with no AtprotoPersonalDataServer service entry.
        let doc: DidDocument = serde_json::from_value(serde_json::json!({
            "id": "did:plc:noendpoint",
            "alsoKnownAs": [],
            "verificationMethod": [],
            "service": [],
        }))
        .unwrap();
        let mock = Arc::new(MockIdentityResolver::with_doc(doc));
        let fetcher = ProductionLexiconFetcher::new(mock, test_http_client());
        let err = fetcher
            .fetch("did:plc:noendpoint", "com.example.foo.bar")
            .await
            .unwrap_err();
        assert!(
            matches!(err, LexiconFetcherError::PdsUnreachable(_)),
            "expected PdsUnreachable, got {err:?}"
        );
    }

    // ──────────────────────────────────────────────────────────────
    // HTTP-response classifications (4xx / 5xx)
    // ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn fetch_classifies_4xx_response_into_http_4xx() {
        let stub = StubPds::start(|| {
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from("not found"))
                .unwrap()
        })
        .await;
        let doc = doc_pointing_at(&stub.url);
        let mock = Arc::new(MockIdentityResolver::with_doc(doc));
        let fetcher = ProductionLexiconFetcher::new(mock, test_http_client());
        let err = fetcher
            .fetch("did:plc:foo", "com.example.foo.bar")
            .await
            .unwrap_err();
        match &err {
            LexiconFetcherError::Http4xx(msg) => assert!(msg.contains("404")),
            other => panic!("expected Http4xx, got {other:?}"),
        }
        assert_eq!(err.failure_class(), "http_4xx");
    }

    #[tokio::test]
    async fn fetch_classifies_5xx_response_into_http_5xx() {
        let stub = StubPds::start(|| {
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("boom"))
                .unwrap()
        })
        .await;
        let doc = doc_pointing_at(&stub.url);
        let mock = Arc::new(MockIdentityResolver::with_doc(doc));
        let fetcher = ProductionLexiconFetcher::new(mock, test_http_client());
        let err = fetcher
            .fetch("did:plc:foo", "com.example.foo.bar")
            .await
            .unwrap_err();
        match &err {
            LexiconFetcherError::Http5xx(msg) => assert!(msg.contains("500")),
            other => panic!("expected Http5xx, got {other:?}"),
        }
        assert_eq!(err.failure_class(), "http_5xx");
    }

    #[tokio::test]
    async fn fetch_classifies_unroutable_endpoint_into_pds_unreachable() {
        // 127.0.0.1:1 — a port the test runner is virtually certain
        // not to have anything listening on. reqwest will surface
        // is_connect()==true.
        let doc = doc_pointing_at("http://127.0.0.1:1");
        let mock = Arc::new(MockIdentityResolver::with_doc(doc));
        let fetcher = ProductionLexiconFetcher::new(mock, test_http_client());
        let err = fetcher
            .fetch("did:plc:foo", "com.example.foo.bar")
            .await
            .unwrap_err();
        assert!(
            matches!(err, LexiconFetcherError::PdsUnreachable(_)),
            "expected PdsUnreachable, got {err:?}"
        );
        assert_eq!(err.failure_class(), "pds_unreachable");
    }

    // ──────────────────────────────────────────────────────────────
    // CAR-parse classifications (malformed + record-absent)
    // ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn fetch_classifies_malformed_car_into_invalid_schema() {
        let stub = StubPds::start(|| {
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/vnd.ipld.car")
                .body(Body::from(b"not a real car".to_vec()))
                .unwrap()
        })
        .await;
        let doc = doc_pointing_at(&stub.url);
        let mock = Arc::new(MockIdentityResolver::with_doc(doc));
        let fetcher = ProductionLexiconFetcher::new(mock, test_http_client());
        let err = fetcher
            .fetch("did:plc:foo", "com.example.foo.bar")
            .await
            .unwrap_err();
        match &err {
            LexiconFetcherError::InvalidResponseStructure(msg) => {
                assert!(msg.contains("CAR parse failed"));
            }
            other => panic!("expected InvalidResponseStructure, got {other:?}"),
        }
        // Fold-2 invariant — malformed CAR must NOT classify as
        // http_4xx (the HTTP fetch returned 200).
        assert_eq!(err.failure_class(), "invalid_schema");
        assert_ne!(err.failure_class(), "http_4xx");
    }

    /// Build a valid CAR containing one bogus block CID'd via for_raw.
    /// `Repo::load` will fail to interpret it as a signed commit, so
    /// this exercises the "valid CAR + invalid commit structure"
    /// classification.
    fn malformed_repo_car() -> Vec<u8> {
        use proto_blue::lex_data::Cid;
        use proto_blue::repo::block_map::BlockMap;
        let payload = b"not a signed commit".to_vec();
        let cid = Cid::for_raw(&payload);
        let mut map = BlockMap::new();
        map.set(cid.clone(), payload);
        pb_car::blocks_to_car(Some(&cid), &map).unwrap()
    }

    #[tokio::test]
    async fn fetch_classifies_invalid_commit_structure_into_invalid_schema() {
        let car_bytes = malformed_repo_car();
        let body_bytes = car_bytes.clone();
        let stub = StubPds::start(move || {
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/vnd.ipld.car")
                .body(Body::from(body_bytes.clone()))
                .unwrap()
        })
        .await;
        let doc = doc_pointing_at(&stub.url);
        let mock = Arc::new(MockIdentityResolver::with_doc(doc));
        let fetcher = ProductionLexiconFetcher::new(mock, test_http_client());
        let err = fetcher
            .fetch("did:plc:foo", "com.example.foo.bar")
            .await
            .unwrap_err();
        assert!(
            matches!(err, LexiconFetcherError::InvalidResponseStructure(_)),
            "expected InvalidResponseStructure, got {err:?}"
        );
        // Fold-2 invariant: invalid commit structure on a 200 OK
        // response must NOT classify as http_4xx.
        assert_eq!(err.failure_class(), "invalid_schema");
    }
}
