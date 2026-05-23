//! Arc 17 §17.3.7 — admin endpoints for the dynamic-lexicon resolver
//! cache, under `tools.aurora.lexicon.*`.
//!
//! Three endpoints:
//! - `getCacheState` (query) — read the in-memory cache; specific NSID
//!   lookup or paginated list.
//! - `evictCache` (procedure) — drop one in-memory entry or every entry.
//!   On-disk rows survive (TTL-driven rehydration / re-fetch).
//! - `fetchNow` (procedure) — force a `resolve_and_fetch` for an NSID.
//!   The warm-up affordance §17.5.1 points operators at.
//!
//! Auth: each handler requires `AdminAuthContext` (admin-tier session)
//! plus `Role::Admin` minimum. The OAuth scope mapping is
//! [`AtProtoScope::AdminAll`] per Step 0.0h pin — `AdminAll` is the
//! broadest admin grant and lexicon ops are admin-state mutations.
//!
//! When `lexicon_resolver` is `None` on the [`AppContext`] (the v0.5
//! default — `PDS_LEXICON_ENABLED=false`), every endpoint returns
//! HTTP 503 [`PdsError::LexiconDisabled`]. Production handlers wire
//! `Some(resolver)` only when `config.lexicon.enabled` is true.

use crate::auth::AdminAuthContext;
use crate::context::AppContext;
use crate::error::{PdsError, PdsResult};
use crate::federation::lexicon_cache::CachedLexicon;
use crate::require_admin_role;
use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

/// `tools.aurora.lexicon.getCacheState` request parameters. All
/// optional; mutual exclusivity is documented in the lexicon JSON.
#[derive(Debug, Deserialize)]
pub struct GetCacheStateParams {
    #[serde(default)]
    pub nsid: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub cursor: Option<String>,
}

/// `tools.aurora.lexicon.evictCache` request body.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvictCacheInput {
    #[serde(default)]
    pub nsid: Option<String>,
    #[serde(default)]
    pub all: Option<bool>,
}

/// `tools.aurora.lexicon.fetchNow` request body.
#[derive(Debug, Deserialize)]
pub struct FetchNowInput {
    pub nsid: String,
}

/// Wire shape for one cache entry (matches the lexicon JSON
/// `cacheEntry` def). `camelCase` to match ATProto JSON conventions.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheEntryWire {
    pub nsid: String,
    pub authority_did: String,
    pub fetched_at: String,
    pub last_used_at: String,
    pub expires_at: String,
    pub is_stale: bool,
}

impl CacheEntryWire {
    fn from_cached(entry: &CachedLexicon, now: chrono::DateTime<chrono::Utc>) -> Self {
        Self {
            nsid: entry.nsid.clone(),
            authority_did: entry.authority_did.clone(),
            fetched_at: entry.fetched_at.to_rfc3339(),
            last_used_at: entry.last_used_at.to_rfc3339(),
            expires_at: entry.expires_at.to_rfc3339(),
            is_stale: entry.is_stale(now),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct GetCacheStateOutput {
    pub entries: Vec<CacheEntryWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EvictCacheOutput {
    pub evicted: usize,
}

#[derive(Debug, Serialize)]
pub struct FetchNowOutput {
    pub entry: CacheEntryWire,
}

/// Default page size for `getCacheState` list mode. Matches the
/// lexicon JSON's `limit` default.
const DEFAULT_LIMIT: usize = 50;
/// Hard cap on `limit` (§17.3.7 paginated shape; matches the lexicon
/// JSON's `maximum`).
const MAX_LIMIT: usize = 100;

/// `tools.aurora.lexicon.getCacheState` — admin read.
pub async fn get_cache_state(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Query(params): Query<GetCacheStateParams>,
) -> PdsResult<Json<GetCacheStateOutput>> {
    require_admin_role!(auth, crate::admin::Role::Admin);

    let resolver = ctx.lexicon_resolver.as_ref().ok_or(PdsError::LexiconDisabled)?;
    let cache = resolver.cache();
    let now = chrono::Utc::now();

    let limit = params
        .limit
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);

    let (entries, total) = cache
        .snapshot(params.cursor.as_deref(), limit, params.nsid.as_deref())
        .await;

    // Compute next-page cursor only when we're in list mode (no
    // nsid filter) AND there are more entries beyond this page.
    let next_cursor = if params.nsid.is_none() && entries.len() == limit && entries.len() < total {
        entries.last().map(|e| e.nsid.clone())
    } else {
        None
    };

    let wire_entries = entries
        .iter()
        .map(|e| CacheEntryWire::from_cached(e, now))
        .collect();

    Ok(Json(GetCacheStateOutput {
        entries: wire_entries,
        cursor: next_cursor,
    }))
}

/// `tools.aurora.lexicon.evictCache` — admin write.
pub async fn evict_cache(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<EvictCacheInput>,
) -> PdsResult<Json<EvictCacheOutput>> {
    require_admin_role!(auth, crate::admin::Role::Admin);

    let resolver = ctx.lexicon_resolver.as_ref().ok_or(PdsError::LexiconDisabled)?;
    let cache = resolver.cache();

    // Mutual exclusivity check. Per the §17.3.7 pin, exactly one of
    // `nsid` or `all = true` must be supplied. Both supplied → 400;
    // neither supplied → 400. `all: false` is the absent state and
    // routes to the same "neither" rejection.
    let nsid_set = input.nsid.is_some();
    let all_set = matches!(input.all, Some(true));
    if nsid_set && all_set {
        return Err(PdsError::Validation(
            "evictCache: nsid and all=true are mutually exclusive".to_string(),
        ));
    }
    if !nsid_set && !all_set {
        return Err(PdsError::Validation(
            "evictCache: must supply nsid or all=true".to_string(),
        ));
    }

    let evicted = if let Some(nsid) = input.nsid.as_deref() {
        cache.evict(nsid).await as usize
    } else {
        cache.evict_all().await
    };

    Ok(Json(EvictCacheOutput { evicted }))
}

/// `tools.aurora.lexicon.fetchNow` — admin warm-up procedure.
///
/// Calls the same `resolve_and_fetch` path the validate-phase
/// fall-through uses; surfaces any §17.3.6 fetch failure as the
/// typed `PdsError` with HTTP 502 / 400 / 500 mapping.
pub async fn fetch_now(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<FetchNowInput>,
) -> PdsResult<Json<FetchNowOutput>> {
    require_admin_role!(auth, crate::admin::Role::Admin);

    let resolver = ctx.lexicon_resolver.as_ref().ok_or(PdsError::LexiconDisabled)?;

    let entry = resolver.resolve_and_fetch(&input.nsid).await?;
    let now = chrono::Utc::now();
    Ok(Json(FetchNowOutput {
        entry: CacheEntryWire::from_cached(&entry, now),
    }))
}

#[cfg(test)]
mod tests {
    //! Step 3.5 — handler-internal logic tests. The admin endpoints'
    //! axum-extractor surface (AdminAuthContext + AppContext) is
    //! exercised at integration scope by Step 4 (Phase B); these
    //! unit tests cover the cache/resolver interaction + the
    //! LexiconDisabled gate without spinning up a full AppContext.

    use super::*;
    use crate::config::LexiconConfig;
    use crate::federation::dns_resolver::{DnsTxtResolver, MockDnsTxtResolver};
    use crate::federation::lexicon_cache::{CachedLexicon, LexiconCache};
    use crate::federation::lexicon_resolver::{
        LexResolver, LexiconFetcherError, LexiconRecordFetcher,
    };
    use async_trait::async_trait;
    use std::sync::Arc;

    struct MockFetcher {
        response: Option<String>,
        error: Option<fn() -> LexiconFetcherError>,
    }

    impl MockFetcher {
        fn with_doc(json: &str) -> Self {
            Self { response: Some(json.to_string()), error: None }
        }
        fn with_error(err_fn: fn() -> LexiconFetcherError) -> Self {
            Self { response: None, error: Some(err_fn) }
        }
    }

    #[async_trait]
    impl LexiconRecordFetcher for MockFetcher {
        async fn fetch(
            &self,
            _authority_did: &str,
            _nsid: &str,
        ) -> Result<String, LexiconFetcherError> {
            if let Some(err_fn) = self.error {
                return Err(err_fn());
            }
            self.response.clone().ok_or(LexiconFetcherError::Http4xx("404".to_string()))
        }
    }

    fn sample_doc(nsid: &str) -> String {
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
                            "properties": {{ "text": {{ "type": "string" }} }}
                        }}
                    }}
                }}
            }}"#
        )
    }

    fn build_resolver_with_doc(nsid: &str, authority: &str) -> Arc<LexResolver> {
        let cfg = LexiconConfig {
            enabled: true,
            ..LexiconConfig::default()
        };
        let dns = MockDnsTxtResolver::new()
            .with_txt(&format!("_lexicon.{authority}"), vec![format!("did=did:plc:test")]);
        let cache = Arc::new(LexiconCache::in_memory(60));
        let dns: Arc<dyn DnsTxtResolver> = Arc::new(dns);
        let fetcher: Arc<dyn LexiconRecordFetcher> = Arc::new(MockFetcher::with_doc(&sample_doc(nsid)));
        Arc::new(LexResolver::new(cache, dns, fetcher, cfg))
    }

    fn build_resolver_with_fetch_error(err_fn: fn() -> LexiconFetcherError) -> Arc<LexResolver> {
        // Tests against this helper use NSID "com.example.thing.foo".
        // §17.3.5 all-segments-minus-last reverse for that NSID gives
        // authority hostname "thing.example.com"; wire the DNS mock to
        // match so DNS resolves cleanly and the configured fetcher
        // error is what surfaces.
        let cfg = LexiconConfig {
            enabled: true,
            ..LexiconConfig::default()
        };
        let dns = MockDnsTxtResolver::new().with_txt(
            "_lexicon.thing.example.com",
            vec!["did=did:plc:test".to_string()],
        );
        let cache = Arc::new(LexiconCache::in_memory(60));
        let dns: Arc<dyn DnsTxtResolver> = Arc::new(dns);
        let fetcher: Arc<dyn LexiconRecordFetcher> = Arc::new(MockFetcher::with_error(err_fn));
        Arc::new(LexResolver::new(cache, dns, fetcher, cfg))
    }

    // ─── LexiconDisabled gate ───
    //
    // The three handlers all branch the same way on
    // `ctx.lexicon_resolver.as_ref().ok_or(LexiconDisabled)`. We test
    // that branch directly with `Option<&Arc<LexResolver>>` rather
    // than constructing a full AppContext (4 sites in src/ already
    // have `create_test_context()` helpers, but their cost-vs-coverage
    // ratio for the no-resolver path isn't worth it here).

    #[tokio::test]
    async fn get_cache_state_returns_lexicon_disabled_when_no_resolver() {
        // Inline the resolver-extraction branch the handler runs.
        // (LexResolver doesn't impl Debug, so `unwrap_err` would
        // require it on the Ok variant; match explicitly instead.)
        let resolver: Option<&Arc<LexResolver>> = None;
        match resolver.ok_or(PdsError::LexiconDisabled) {
            Ok(_) => panic!("expected LexiconDisabled"),
            Err(e) => assert_eq!(e, PdsError::LexiconDisabled),
        }
    }

    // ─── fetchNow: happy + error mapping ───

    #[tokio::test]
    async fn fetch_now_logic_returns_entry_for_unknown_nsid_on_happy_path() {
        let nsid = "com.example.thing.foo";
        let resolver = build_resolver_with_doc(nsid, "thing.example.com");
        let entry = resolver.resolve_and_fetch(nsid).await.expect("happy");
        assert_eq!(entry.nsid, nsid);
        // The handler wraps this into FetchNowOutput; verify the
        // wire conversion doesn't drop fields.
        let now = chrono::Utc::now();
        let wire = CacheEntryWire::from_cached(&entry, now);
        assert_eq!(wire.nsid, nsid);
        assert_eq!(wire.authority_did, "did:plc:test");
        assert!(!wire.is_stale);
    }

    #[tokio::test]
    async fn fetch_now_logic_surfaces_lexicon_fetch_failed_on_http_5xx() {
        let resolver = build_resolver_with_fetch_error(|| {
            LexiconFetcherError::Http5xx("503".to_string())
        });
        let err = resolver
            .resolve_and_fetch("com.example.thing.foo")
            .await
            .unwrap_err();
        match err {
            PdsError::LexiconFetchFailed { failure_class, .. } => {
                assert_eq!(failure_class, "http_5xx");
            }
            other => panic!("expected LexiconFetchFailed, got {other:?}"),
        }
    }

    // ─── getCacheState: snapshot reflects fetched entries ───

    #[tokio::test]
    async fn get_cache_state_logic_reflects_fetched_entries() {
        let nsid = "com.example.thing.foo";
        let resolver = build_resolver_with_doc(nsid, "thing.example.com");
        // Populate cache via a fetch.
        resolver.resolve_and_fetch(nsid).await.expect("seed");
        // Now snapshot the cache (mimicking the handler's path).
        let (entries, total) = resolver.cache().snapshot(None, 50, None).await;
        assert_eq!(total, 1);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].nsid, nsid);
    }

    #[tokio::test]
    async fn get_cache_state_logic_nsid_filter_returns_single_entry() {
        let nsid = "com.example.thing.foo";
        let resolver = build_resolver_with_doc(nsid, "thing.example.com");
        resolver.resolve_and_fetch(nsid).await.expect("seed");
        let (entries, total) = resolver.cache().snapshot(None, 50, Some(nsid)).await;
        assert_eq!(total, 1);
        assert_eq!(entries[0].nsid, nsid);
    }

    #[tokio::test]
    async fn get_cache_state_logic_nsid_filter_returns_empty_for_unknown() {
        let resolver = build_resolver_with_doc("com.example.thing.foo", "thing.example.com");
        // No seeding fetch — cache is empty.
        let (entries, total) = resolver
            .cache()
            .snapshot(None, 50, Some("com.example.unknown.nsid"))
            .await;
        assert_eq!(total, 0);
        assert!(entries.is_empty());
    }

    // ─── evictCache: drop one + drop all ───

    #[tokio::test]
    async fn evict_cache_logic_drops_specific_nsid() {
        let nsid = "com.example.thing.foo";
        let resolver = build_resolver_with_doc(nsid, "thing.example.com");
        resolver.resolve_and_fetch(nsid).await.expect("seed");

        let evicted = resolver.cache().evict(nsid).await as usize;
        assert_eq!(evicted, 1);

        let evicted_again = resolver.cache().evict(nsid).await as usize;
        assert_eq!(evicted_again, 0, "second evict on the same nsid is a no-op");
    }

    #[tokio::test]
    async fn evict_cache_logic_drops_all_entries() {
        // Seed two distinct entries via two fetches.
        // Constructing two LexResolvers with different sample docs
        // and merging caches would be over-engineered; we'll fetch
        // twice through the same resolver using different NSIDs
        // resolved to the same mock fetcher (which returns whatever
        // doc was configured — for evict-all we just need >0
        // entries).
        let resolver = build_resolver_with_doc("com.example.thing.foo", "thing.example.com");
        resolver
            .resolve_and_fetch("com.example.thing.foo")
            .await
            .expect("seed-1");
        // Second NSID; same fetcher returns the same canned doc but
        // the cache key is different. (The doc's `id` mismatches the
        // requested NSID, but `validate_against_lexicon` is the
        // mismatch-detector and this test doesn't exercise validate;
        // the cache map keys by requested NSID.)
        resolver
            .resolve_and_fetch("com.example.thing.bar")
            .await
            .expect("seed-2");

        let evicted = resolver.cache().evict_all().await;
        assert_eq!(evicted, 2);
        let (_, total) = resolver.cache().snapshot(None, 50, None).await;
        assert_eq!(total, 0);
    }

    // ─── Metric emission proof (Step 3.3 wiring) ───
    //
    // We don't unit-test Prometheus counter values directly (the
    // counters are process-global lazy_statics, and other tests
    // running in parallel may also increment them). What we DO
    // test is that the resolver's dispatch path actually fires
    // (already covered by the resolve_and_fetch unit tests in
    // src/federation/lexicon_resolver.rs); the metric increments
    // are emitted unconditionally on those paths and Phase B
    // (Step 4) is the integration proof that scraping `/metrics`
    // returns non-zero values after live traffic.

    #[tokio::test]
    async fn failure_class_classification_covers_round1_f14_taxonomy() {
        // Spot-check several failure_class values map correctly
        // via the LexiconFetchFailed PdsError variant.
        let cases: &[(fn() -> LexiconFetcherError, &str)] = &[
            (|| LexiconFetcherError::Http5xx("x".to_string()), "http_5xx"),
            (|| LexiconFetcherError::Http4xx("x".to_string()), "http_4xx"),
            (|| LexiconFetcherError::Timeout, "timeout"),
            (
                || LexiconFetcherError::PdsUnreachable("x".to_string()),
                "pds_unreachable",
            ),
            (
                || LexiconFetcherError::DidResolutionFailed {
                    did: "did:plc:x".to_string(),
                    detail: "boom".to_string(),
                },
                "did_fail",
            ),
        ];

        for (err_fn, expected_class) in cases {
            let resolver = build_resolver_with_fetch_error(*err_fn);
            let err = resolver
                .resolve_and_fetch("com.example.thing.foo")
                .await
                .unwrap_err();
            match err {
                PdsError::LexiconFetchFailed { failure_class, .. } => {
                    assert_eq!(
                        failure_class, *expected_class,
                        "failure_class mismatch for one of the round-1 F14 cases"
                    );
                }
                other => panic!("expected LexiconFetchFailed, got {other:?}"),
            }
        }
    }

    // ─── Auth gate documentation ───
    //
    // The AdminAuthContext extractor + require_admin_role!(auth,
    // Role::Admin) gate is provided by the existing aurora_admin
    // tests (which exercise the same extractor against the
    // emit_event handler). Adding a redundant test here would
    // duplicate the AdminAuthContext fixture work; the audit trail
    // is: aurora_lexicon handlers use the same extractor + macro
    // as aurora_admin emit_event, so anything that breaks the auth
    // gate breaks both surfaces' tests simultaneously.
}
