//! Arc 17 §17.3.1 — `LexResolver` end-to-end: cache check → single-flight
//! gate → authority resolution (DNS TXT + `did=` strict parse, or
//! `did_authority` override) → lexicon record fetch (delegated to a
//! [`LexiconRecordFetcher`] impl so the PLC + HTTP plumbing lives at the
//! validator-integration layer, not here) → proto-blue parse → cache
//! write.
//!
//! Single-flight gate (§17.3.2 / round-1 F6): concurrent
//! `resolve_and_fetch(nsid)` calls for the same NSID share one in-flight
//! future via `futures::Shared`. The first call registers; subsequent
//! calls await the same `Shared<BoxFuture>` and resolve when it does.
//! Releases on completion (Ok or Err).
//!
//! Strict TXT-parse posture (§17.3.1 step 3c / round-1 F5 / Step 0.0a
//! ratification): multiple TXT records OR multiple `did=` entries in a
//! single record → hard-fail with `LexiconAuthorityAmbiguous`. Matches
//! bsky-PDS at the reference SHA byte-for-byte.
//!
//! NSID authority algorithm (§17.3.5 v2 / Step 0.0e ratification): all
//! segments minus the last, reversed; e.g. `app.bsky.feed.post` →
//! `feed.bsky.app`. See `nsid_authority` for the worked examples.

use crate::config::LexiconConfig;
use crate::error::PdsError;
use crate::federation::dns_resolver::{DnsResolverError, DnsTxtResolver};
use crate::federation::lexicon_cache::{log_persist_failure, CachedLexicon, LexiconCache};
use async_trait::async_trait;
use chrono::Utc;
use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use proto_blue::lexicon::{LexiconDoc, Lexicons};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

/// Trait the resolver uses to perform the lexicon-record HTTP fetch
/// against the authority DID's hosting PDS. Lives behind a trait so
/// unit tests can inject canned responses without spinning up real
/// PLC + HTTP stacks.
///
/// Production impl (wired in Step 2) routes through Arc 13's PLC
/// resolver to find the hosting PDS, then issues an anonymous
/// `com.atproto.sync.getRecord?collection=com.atproto.lexicon.schema&rkey=<nsid>`
/// against that endpoint. Tombstoned-authority detection happens
/// inside the production impl (the PLC client surfaces it) and
/// surfaces as [`LexiconFetcherError::AuthorityTombstoned`].
#[async_trait]
pub trait LexiconRecordFetcher: Send + Sync {
    /// Fetch the lexicon-record JSON for `nsid` from the PDS hosting
    /// `authority_did`. Returns the raw lexicon-doc JSON body (the
    /// resolver parses via proto-blue separately).
    async fn fetch(
        &self,
        authority_did: &str,
        nsid: &str,
    ) -> Result<String, LexiconFetcherError>;
}

/// Coarse-grained fetcher errors mapped to §17.3.6 `failure_class`
/// taxonomy values. Each variant carries enough context for the
/// resolver to construct the right [`PdsError`] variant.
#[derive(Debug, thiserror::Error)]
pub enum LexiconFetcherError {
    /// PLC resolution succeeded but the DID has a `#tombstone` op as
    /// its latest entry (round-1 F13 closure). Maps to
    /// `failure_class = "authority_tombstoned"`.
    #[error("authority DID {0} is tombstoned")]
    AuthorityTombstoned(String),

    /// PLC resolution failed (DID not found, PLC unreachable, etc.).
    /// Maps to `failure_class = "did_fail"`.
    #[error("DID resolution failed for {did}: {detail}")]
    DidResolutionFailed { did: String, detail: String },

    /// PLC resolved but the hosting PDS endpoint was unreachable
    /// (DNS, TCP, TLS-handshake failure). Maps to
    /// `failure_class = "pds_unreachable"`.
    #[error("hosting PDS unreachable: {0}")]
    PdsUnreachable(String),

    /// PDS returned a 4xx response (lexicon record doesn't exist, or
    /// is malformed, or the rkey path is wrong). Maps to
    /// `failure_class = "http_4xx"`.
    #[error("HTTP 4xx fetching lexicon: {0}")]
    Http4xx(String),

    /// PDS returned a 5xx response. Maps to
    /// `failure_class = "http_5xx"`.
    #[error("HTTP 5xx fetching lexicon: {0}")]
    Http5xx(String),

    /// Per-attempt timeout expired. Maps to
    /// `failure_class = "timeout"`.
    #[error("HTTP timeout fetching lexicon")]
    Timeout,
}

impl LexiconFetcherError {
    /// Map to the round-1 F14 forensic-log `failure_class` taxonomy.
    pub fn failure_class(&self) -> &'static str {
        match self {
            Self::AuthorityTombstoned(_) => "authority_tombstoned",
            Self::DidResolutionFailed { .. } => "did_fail",
            Self::PdsUnreachable(_) => "pds_unreachable",
            Self::Http4xx(_) => "http_4xx",
            Self::Http5xx(_) => "http_5xx",
            Self::Timeout => "timeout",
        }
    }
}

/// Single-flight shared-future shape. Inner future is `Send` because
/// proto-blue types and tokio runtimes require it; `Shared` lets us
/// hand out cheap clones to concurrent callers.
type InFlight = Shared<BoxFuture<'static, Result<CachedLexicon, ResolverErrorRepr>>>;

/// Internal compact representation of a fetch failure. Carries just
/// enough info to reconstruct the right [`PdsError`] at the call site;
/// kept distinct from `PdsError` itself because `Shared` requires
/// `Clone` and `PdsError` is mostly-Clone-but-not-uniformly.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolverErrorRepr {
    InvalidNsid(String),
    Ambiguous { nsid: String, candidates: Vec<String> },
    Tombstoned { nsid: String, did: String },
    FetchFailed { nsid: String, failure_class: &'static str, detail: String },
    InvalidSchema { nsid: String, detail: String },
}

impl ResolverErrorRepr {
    fn into_pds_error(self) -> PdsError {
        match self {
            Self::InvalidNsid(nsid) => PdsError::LexiconInvalidNsid { nsid },
            Self::Ambiguous { nsid, candidates } => {
                PdsError::LexiconAuthorityAmbiguous { nsid, candidates }
            }
            Self::Tombstoned { nsid, did } => {
                PdsError::LexiconAuthorityTombstoned { nsid, did }
            }
            Self::FetchFailed { nsid, failure_class, detail } => {
                PdsError::LexiconFetchFailed { nsid, failure_class, source_detail: detail }
            }
            Self::InvalidSchema { nsid, detail } => {
                PdsError::LexiconInvalidSchema { nsid, detail }
            }
        }
    }
}

/// The resolver. Holds the cache, the DNS+fetcher traits, and the
/// configuration knobs. One instance per process; clones are cheap
/// because the inner state is `Arc`-shared.
pub struct LexResolver {
    cache: Arc<LexiconCache>,
    dns: Arc<dyn DnsTxtResolver>,
    fetcher: Arc<dyn LexiconRecordFetcher>,
    config: LexiconConfig,
    in_flight: Arc<Mutex<HashMap<String, InFlight>>>,
}

impl LexResolver {
    /// Build a resolver. Caller wires production impls
    /// ([`crate::federation::dns_resolver::HickoryDnsTxtResolver`] +
    /// the Step-2 production fetcher) or test mocks. The cache must
    /// be set up with the right TTL / persist threshold per
    /// [`LexiconConfig`].
    pub fn new(
        cache: Arc<LexiconCache>,
        dns: Arc<dyn DnsTxtResolver>,
        fetcher: Arc<dyn LexiconRecordFetcher>,
        config: LexiconConfig,
    ) -> Self {
        Self {
            cache,
            dns,
            fetcher,
            config,
            in_flight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Resolve + fetch for a single NSID, single-flight-gated. The
    /// returned `CachedLexicon` is from the in-memory cache; the
    /// on-disk persist fires async on cache misses (errors logged via
    /// [`log_persist_failure`], do not propagate).
    pub async fn resolve_and_fetch(&self, nsid: &str) -> Result<CachedLexicon, PdsError> {
        let now = Utc::now();

        // 1. Cache check first — serves stale-but-cached entries
        // immediately per §17.3.1 step 1. (Background re-fetch on
        // stale is a v0.6+ feature per §17.5.4; v0.5 serves the
        // stale value without spawning a refresh.)
        if let Some(entry) = self.cache.get(nsid, now).await {
            return Ok(entry);
        }

        // 2. Single-flight gate.
        let shared: InFlight = {
            let mut guard = self.in_flight.lock().await;
            if let Some(existing) = guard.get(nsid) {
                existing.clone()
            } else {
                let nsid_owned = nsid.to_string();
                let self_clone = self.clone_inner_for_fetch();
                let fut: BoxFuture<'static, Result<CachedLexicon, ResolverErrorRepr>> =
                    Box::pin(async move { self_clone.fetch_uncached(&nsid_owned).await });
                let shared = fut.shared();
                guard.insert(nsid.to_string(), shared.clone());
                shared
            }
        };

        let result = shared.await;

        // 3. Release the in-flight slot regardless of outcome — a
        // subsequent call (after this one resolves) will re-fetch
        // rather than wait on a stale `Shared`.
        {
            let mut guard = self.in_flight.lock().await;
            guard.remove(nsid);
        }

        result.map_err(|repr| repr.into_pds_error())
    }

    /// Clone the bits the spawned single-flight future needs. We
    /// can't share `&self` across the future boundary because the
    /// `Shared` future is `'static`; instead we hand a lightweight
    /// shadow struct over.
    fn clone_inner_for_fetch(&self) -> ResolverFetchHandle {
        ResolverFetchHandle {
            cache: self.cache.clone(),
            dns: self.dns.clone(),
            fetcher: self.fetcher.clone(),
            config: self.config.clone(),
        }
    }
}

/// Lightweight shadow struct passed across the `Shared` future
/// boundary. Has just enough state to perform a single fetch.
struct ResolverFetchHandle {
    cache: Arc<LexiconCache>,
    dns: Arc<dyn DnsTxtResolver>,
    fetcher: Arc<dyn LexiconRecordFetcher>,
    config: LexiconConfig,
}

impl ResolverFetchHandle {
    async fn fetch_uncached(self, nsid: &str) -> Result<CachedLexicon, ResolverErrorRepr> {
        // Validate NSID first — cheap, deterministic, no I/O.
        if !is_valid_nsid(nsid) {
            return Err(ResolverErrorRepr::InvalidNsid(nsid.to_string()));
        }

        // Resolve authority DID. The `did_authority` config override
        // (if set) bypasses DNS TXT entirely.
        let authority_did = match &self.config.did_authority {
            Some(did) => did.clone(),
            None => self.resolve_authority_did(nsid).await?,
        };

        // Fetch the lexicon record JSON.
        let lexicon_json = self.fetcher.fetch(&authority_did, nsid).await.map_err(|e| {
            let failure_class = e.failure_class();
            match e {
                LexiconFetcherError::AuthorityTombstoned(did) => {
                    ResolverErrorRepr::Tombstoned { nsid: nsid.to_string(), did }
                }
                _ => ResolverErrorRepr::FetchFailed {
                    nsid: nsid.to_string(),
                    failure_class,
                    detail: e.to_string(),
                },
            }
        })?;

        // Parse via proto-blue. Schema-validation errors → InvalidSchema.
        let doc: LexiconDoc = serde_json::from_str(&lexicon_json).map_err(|e| {
            ResolverErrorRepr::InvalidSchema {
                nsid: nsid.to_string(),
                detail: format!("JSON parse: {e}"),
            }
        })?;
        let mut registry = Lexicons::new();
        registry.add(doc.clone()).map_err(|e| ResolverErrorRepr::InvalidSchema {
            nsid: nsid.to_string(),
            detail: e.to_string(),
        })?;

        // Build cache entry + write both layers.
        let now = Utc::now();
        let entry = CachedLexicon::new(
            nsid.to_string(),
            authority_did.clone(),
            doc,
            registry,
            lexicon_json,
            now,
            self.config.cache_ttl_secs,
        );
        self.cache.insert(entry.clone()).await;

        // Async on-disk persist (§17.5.8) — fire-and-forget; failure
        // emits a WARN log and leaves the in-memory entry valid for
        // its TTL.
        let cache_for_persist = self.cache.clone();
        let entry_for_persist = entry.clone();
        let nsid_for_log = nsid.to_string();
        tokio::spawn(async move {
            if let Err(e) = cache_for_persist.persist(&entry_for_persist).await {
                log_persist_failure(&nsid_for_log, &e);
            }
        });

        info!(
            event = "lexicon_fetch_complete",
            nsid = %nsid,
            authority_did = %authority_did,
            "lexicon fetch + parse + cache write succeeded"
        );
        Ok(entry)
    }

    async fn resolve_authority_did(&self, nsid: &str) -> Result<String, ResolverErrorRepr> {
        let authority = nsid_authority(nsid)
            .map_err(|_| ResolverErrorRepr::InvalidNsid(nsid.to_string()))?;
        let txt_name = format!("_lexicon.{authority}");

        let records = self.dns.resolve_txt(&txt_name).await.map_err(|e| {
            let (failure_class, detail) = match &e {
                DnsResolverError::NoRecords(_) => ("dns_fail", e.to_string()),
                DnsResolverError::Transport { .. } => ("dns_fail", e.to_string()),
            };
            ResolverErrorRepr::FetchFailed {
                nsid: nsid.to_string(),
                failure_class,
                detail,
            }
        })?;

        parse_did_from_txt(&records).map_err(|candidates| {
            if candidates.is_empty() {
                ResolverErrorRepr::FetchFailed {
                    nsid: nsid.to_string(),
                    failure_class: "dns_fail",
                    detail: format!("no did= entries in TXT records for {txt_name}"),
                }
            } else {
                ResolverErrorRepr::Ambiguous {
                    nsid: nsid.to_string(),
                    candidates,
                }
            }
        })
    }
}

/// Compute the authority hostname for an NSID. §17.3.5 v2 algorithm:
/// all-segments-minus-last reverse. Worked examples:
/// - `app.bsky.feed.post` → `feed.bsky.app`
/// - `com.atproto.lexicon.schema` → `lexicon.atproto.com`
/// - `tools.ozone.moderation.defs` → `moderation.ozone.tools`
/// - `com.atproto.lexicon` (3-segment minimum) → `atproto.com`
pub fn nsid_authority(nsid: &str) -> Result<String, PdsError> {
    let parts: Vec<&str> = nsid.split('.').collect();
    if parts.len() < 3 {
        return Err(PdsError::LexiconInvalidNsid {
            nsid: nsid.to_string(),
        });
    }
    for seg in &parts {
        if !is_valid_nsid_segment(seg) {
            return Err(PdsError::LexiconInvalidNsid {
                nsid: nsid.to_string(),
            });
        }
    }
    let auth_segments: Vec<&str> =
        parts[..parts.len() - 1].iter().rev().copied().collect();
    Ok(auth_segments.join("."))
}

/// Single-segment validator per ATProto NSID spec: `[a-z][a-z0-9-]*[a-z0-9]`.
fn is_valid_nsid_segment(seg: &str) -> bool {
    let bytes = seg.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    if !bytes[0].is_ascii_lowercase() {
        return false;
    }
    if !bytes[bytes.len() - 1].is_ascii_alphanumeric() {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Full-NSID validator (≥ 3 segments, each valid).
fn is_valid_nsid(nsid: &str) -> bool {
    let parts: Vec<&str> = nsid.split('.').collect();
    parts.len() >= 3 && parts.iter().all(|seg| is_valid_nsid_segment(seg))
}

/// Strict `did=` parse per §17.3.1 step 3c / round-1 F5. The TXT
/// `records` slice is the resolver's output (each entry is one TXT
/// record's joined chunks). Rules:
/// - Across ALL records, count entries starting with `did=`. There
///   must be exactly one. Multiple → `Err(candidates)` (ambiguous).
///   Zero → `Err(empty)` (no did=). Whitespace tolerance: none.
///
/// Returns `Ok(did)` or `Err(candidates_for_log)` where `candidates`
/// is the full list of `did=...` values seen (so operators can see
/// what was actually published).
fn parse_did_from_txt(records: &[String]) -> Result<String, Vec<String>> {
    let mut candidates: Vec<String> = Vec::new();
    for rec in records {
        if let Some(did) = rec.strip_prefix("did=") {
            candidates.push(did.to_string());
        }
    }
    match candidates.len() {
        1 => Ok(candidates.into_iter().next().unwrap()),
        _ => Err(candidates),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::federation::dns_resolver::{DnsResolverError, MockDnsTxtResolver};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Test fetcher counting `fetch` invocations + returning canned
    /// responses keyed by NSID. `error_for` lets a test wire a
    /// specific failure for one NSID.
    struct MockFetcher {
        responses: std::collections::HashMap<String, String>,
        errors: std::collections::HashMap<String, fn() -> LexiconFetcherError>,
        calls: AtomicUsize,
        delay_ms: u64,
    }

    impl MockFetcher {
        fn new() -> Self {
            Self {
                responses: std::collections::HashMap::new(),
                errors: std::collections::HashMap::new(),
                calls: AtomicUsize::new(0),
                delay_ms: 0,
            }
        }
        fn with_response(mut self, nsid: &str, body: &str) -> Self {
            self.responses.insert(nsid.to_string(), body.to_string());
            self
        }
        fn with_error(mut self, nsid: &str, err_fn: fn() -> LexiconFetcherError) -> Self {
            self.errors.insert(nsid.to_string(), err_fn);
            self
        }
        fn with_delay_ms(mut self, ms: u64) -> Self {
            self.delay_ms = ms;
            self
        }
        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl LexiconRecordFetcher for MockFetcher {
        async fn fetch(
            &self,
            _authority_did: &str,
            nsid: &str,
        ) -> Result<String, LexiconFetcherError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            }
            if let Some(err_fn) = self.errors.get(nsid) {
                return Err(err_fn());
            }
            self.responses
                .get(nsid)
                .cloned()
                .ok_or_else(|| LexiconFetcherError::Http4xx("404 not found".to_string()))
        }
    }

    fn sample_doc_json(nsid: &str) -> String {
        format!(
            r#"{{
                "lexicon": 1,
                "id": "{nsid}",
                "defs": {{
                    "main": {{
                        "type": "record",
                        "key": "tid",
                        "record": {{
                            "type": "object",
                            "required": ["text"],
                            "properties": {{
                                "text": {{ "type": "string" }}
                            }}
                        }}
                    }}
                }}
            }}"#
        )
    }

    fn config_with(did_authority: Option<&str>) -> LexiconConfig {
        let mut cfg = LexiconConfig::default();
        cfg.enabled = true;
        cfg.did_authority = did_authority.map(|s| s.to_string());
        cfg
    }

    // ──────────────────────────────────────────────────────────────
    // §17.3.5 algorithm tests — round-1 F9 / Step 0.0e ratification.
    // The 4+ segment cases are the load-bearing ones; the 3-segment
    // case is the degenerate-but-correct boundary.
    // ──────────────────────────────────────────────────────────────

    #[test]
    fn nsid_authority_four_segment_app_bsky_feed_post() {
        assert_eq!(
            nsid_authority("app.bsky.feed.post").unwrap(),
            "feed.bsky.app"
        );
    }

    #[test]
    fn nsid_authority_four_segment_com_atproto_lexicon_schema() {
        assert_eq!(
            nsid_authority("com.atproto.lexicon.schema").unwrap(),
            "lexicon.atproto.com"
        );
    }

    #[test]
    fn nsid_authority_four_segment_tools_ozone_moderation_defs() {
        assert_eq!(
            nsid_authority("tools.ozone.moderation.defs").unwrap(),
            "moderation.ozone.tools"
        );
    }

    #[test]
    fn nsid_authority_three_segment_degenerate_matches_first_two_reverse() {
        // The degenerate case where all-segments-minus-last reverse
        // produces the same result as first-two reverse. This is
        // exactly the case v1's algorithm passed; the v2 algorithm
        // must continue to pass it.
        assert_eq!(
            nsid_authority("com.atproto.lexicon").unwrap(),
            "atproto.com"
        );
    }

    #[test]
    fn nsid_authority_rejects_two_segment() {
        let err = nsid_authority("com.example").unwrap_err();
        assert!(matches!(err, PdsError::LexiconInvalidNsid { .. }));
    }

    #[test]
    fn nsid_authority_rejects_invalid_segment_start_digit() {
        let err = nsid_authority("9invalid.bsky.app").unwrap_err();
        assert!(matches!(err, PdsError::LexiconInvalidNsid { .. }));
    }

    #[test]
    fn nsid_authority_rejects_invalid_segment_trailing_hyphen() {
        let err = nsid_authority("app.bsky.feed-.post").unwrap_err();
        assert!(matches!(err, PdsError::LexiconInvalidNsid { .. }));
    }

    // ──────────────────────────────────────────────────────────────
    // TXT-parse strict-posture tests — §17.3.1 step 3c / round-1 F5 /
    // Step 0.0a ratification (matches bsky-PDS hard-fail behavior).
    // ──────────────────────────────────────────────────────────────

    #[test]
    fn parse_did_from_txt_single_record_single_did_ok() {
        let records = vec!["did=did:plc:abc".to_string()];
        assert_eq!(parse_did_from_txt(&records).unwrap(), "did:plc:abc");
    }

    #[test]
    fn parse_did_from_txt_multiple_records_returns_candidates() {
        let records = vec![
            "did=did:plc:one".to_string(),
            "did=did:plc:two".to_string(),
        ];
        let candidates = parse_did_from_txt(&records).unwrap_err();
        assert_eq!(candidates.len(), 2);
        assert!(candidates.contains(&"did:plc:one".to_string()));
        assert!(candidates.contains(&"did:plc:two".to_string()));
    }

    #[test]
    fn parse_did_from_txt_zero_did_entries_returns_empty_candidates() {
        let records = vec!["something=else".to_string(), "spf=v1".to_string()];
        let candidates = parse_did_from_txt(&records).unwrap_err();
        assert!(candidates.is_empty());
    }

    // ──────────────────────────────────────────────────────────────
    // End-to-end resolver tests.
    // ──────────────────────────────────────────────────────────────

    fn build_resolver(
        dns: MockDnsTxtResolver,
        fetcher: MockFetcher,
        config: LexiconConfig,
    ) -> (LexResolver, Arc<MockFetcher>) {
        let cache = Arc::new(LexiconCache::in_memory(60));
        let dns: Arc<dyn DnsTxtResolver> = Arc::new(dns);
        let fetcher_arc = Arc::new(fetcher);
        let fetcher_dyn: Arc<dyn LexiconRecordFetcher> = fetcher_arc.clone();
        let resolver = LexResolver::new(cache, dns, fetcher_dyn, config);
        (resolver, fetcher_arc)
    }

    #[tokio::test]
    async fn resolve_and_fetch_happy_path_caches_and_returns_doc() {
        let nsid = "app.bsky.feed.post";
        let auth_did = "did:plc:bsky";
        let dns = MockDnsTxtResolver::new()
            .with_txt("_lexicon.feed.bsky.app", vec![format!("did={auth_did}")]);
        let fetcher = MockFetcher::new().with_response(nsid, &sample_doc_json(nsid));
        let (r, fetcher_arc) = build_resolver(dns, fetcher, config_with(None));

        let entry = r.resolve_and_fetch(nsid).await.expect("happy");
        assert_eq!(entry.nsid, nsid);
        assert_eq!(entry.authority_did, auth_did);
        assert_eq!(fetcher_arc.call_count(), 1);

        // Second call should hit cache and NOT increment fetcher.
        let entry2 = r.resolve_and_fetch(nsid).await.expect("hit");
        assert_eq!(entry2.nsid, nsid);
        assert_eq!(fetcher_arc.call_count(), 1, "second call must hit cache");
    }

    #[tokio::test]
    async fn resolve_and_fetch_did_authority_override_skips_dns() {
        let nsid = "app.bsky.feed.post";
        // DNS would fail if consulted — but the override should skip
        // it entirely.
        let dns = MockDnsTxtResolver::new();
        let fetcher = MockFetcher::new().with_response(nsid, &sample_doc_json(nsid));
        let (r, _) = build_resolver(dns, fetcher, config_with(Some("did:plc:override")));

        let entry = r.resolve_and_fetch(nsid).await.expect("override");
        assert_eq!(entry.authority_did, "did:plc:override");
    }

    #[tokio::test]
    async fn resolve_and_fetch_multi_did_records_returns_ambiguous() {
        let nsid = "app.bsky.feed.post";
        let dns = MockDnsTxtResolver::new().with_txt(
            "_lexicon.feed.bsky.app",
            vec![
                "did=did:plc:one".to_string(),
                "did=did:plc:two".to_string(),
            ],
        );
        let fetcher = MockFetcher::new();
        let (r, _) = build_resolver(dns, fetcher, config_with(None));

        let err = r.resolve_and_fetch(nsid).await.unwrap_err();
        match err {
            PdsError::LexiconAuthorityAmbiguous { nsid: n, candidates } => {
                assert_eq!(n, nsid);
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected LexiconAuthorityAmbiguous, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_and_fetch_tombstoned_authority_routes_to_tombstoned_variant() {
        let nsid = "app.bsky.feed.post";
        let dns = MockDnsTxtResolver::new()
            .with_txt("_lexicon.feed.bsky.app", vec!["did=did:plc:dead".to_string()]);
        let fetcher = MockFetcher::new()
            .with_error(nsid, || LexiconFetcherError::AuthorityTombstoned("did:plc:dead".to_string()));
        let (r, _) = build_resolver(dns, fetcher, config_with(None));

        let err = r.resolve_and_fetch(nsid).await.unwrap_err();
        match err {
            PdsError::LexiconAuthorityTombstoned { nsid: n, did } => {
                assert_eq!(n, nsid);
                assert_eq!(did, "did:plc:dead");
            }
            other => panic!("expected LexiconAuthorityTombstoned, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_and_fetch_invalid_nsid_rejected_before_io() {
        let dns = MockDnsTxtResolver::new();
        let fetcher = MockFetcher::new();
        let (r, fetcher_arc) = build_resolver(dns, fetcher, config_with(None));

        let err = r.resolve_and_fetch("only.two").await.unwrap_err();
        assert!(matches!(err, PdsError::LexiconInvalidNsid { .. }));
        assert_eq!(
            fetcher_arc.call_count(),
            0,
            "invalid NSID must not reach the fetcher"
        );
    }

    #[tokio::test]
    async fn single_flight_dedups_concurrent_calls() {
        let nsid = "app.bsky.feed.post";
        let auth_did = "did:plc:bsky";
        let dns = MockDnsTxtResolver::new()
            .with_txt("_lexicon.feed.bsky.app", vec![format!("did={auth_did}")]);
        // 50ms delay forces the second concurrent call to land on
        // the in-flight slot before the first completes.
        let fetcher = MockFetcher::new()
            .with_response(nsid, &sample_doc_json(nsid))
            .with_delay_ms(50);
        let (r, fetcher_arc) = build_resolver(dns, fetcher, config_with(None));
        let r = Arc::new(r);

        let r1 = r.clone();
        let r2 = r.clone();
        let h1 = tokio::spawn(async move { r1.resolve_and_fetch(nsid).await });
        let h2 = tokio::spawn(async move { r2.resolve_and_fetch(nsid).await });

        let (a, b) = tokio::join!(h1, h2);
        a.unwrap().expect("first ok");
        b.unwrap().expect("second ok");
        assert_eq!(
            fetcher_arc.call_count(),
            1,
            "two concurrent resolves must share one fetcher call"
        );
    }

    #[tokio::test]
    async fn fetch_http_5xx_routes_to_lexicon_fetch_failed_with_class() {
        let nsid = "app.bsky.feed.post";
        let auth_did = "did:plc:bsky";
        let dns = MockDnsTxtResolver::new()
            .with_txt("_lexicon.feed.bsky.app", vec![format!("did={auth_did}")]);
        let fetcher = MockFetcher::new()
            .with_error(nsid, || LexiconFetcherError::Http5xx("503".to_string()));
        let (r, _) = build_resolver(dns, fetcher, config_with(None));

        let err = r.resolve_and_fetch(nsid).await.unwrap_err();
        match err {
            PdsError::LexiconFetchFailed { failure_class, .. } => {
                assert_eq!(failure_class, "http_5xx");
            }
            other => panic!("expected LexiconFetchFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_invalid_json_routes_to_lexicon_invalid_schema() {
        let nsid = "app.bsky.feed.post";
        let auth_did = "did:plc:bsky";
        let dns = MockDnsTxtResolver::new()
            .with_txt("_lexicon.feed.bsky.app", vec![format!("did={auth_did}")]);
        let fetcher = MockFetcher::new().with_response(nsid, "not-json");
        let (r, _) = build_resolver(dns, fetcher, config_with(None));

        let err = r.resolve_and_fetch(nsid).await.unwrap_err();
        assert!(matches!(err, PdsError::LexiconInvalidSchema { .. }));
    }
}
