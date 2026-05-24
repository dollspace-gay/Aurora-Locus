//! Arc 17 §17.3.2 — two-layer lexicon cache (in-memory hot + on-disk
//! persist). The resolver writes through both; reads are in-memory only.
//!
//! On-disk persist is async / fire-and-forget — a failed INSERT does NOT
//! block the validate-phase return per §17.5.8. On restart, in-memory
//! state is lost and the cache rehydrates lazily via cold-on-demand
//! fetches; operators seeing repeated `lexicon_fetch_starting` events
//! for the same NSID should check for `lexicon_cache_persist_failed`
//! WARN logs.
//!
//! `last_used_at` write throttling (round-1 F11): in-memory updates
//! immediately on every hit, but on-disk writes fire only when the
//! in-memory value advances by ≥ `last_used_persist_threshold_secs`
//! (default 60s from [`crate::config::LexiconConfig`]). Keeps hot-NSID
//! reads from hammering the `lexicon_cache` table.
//!
//! Schema (both backends, pinned by chainlink #132 / migration 0012
//! SQLite + 0013 Postgres): all timestamp columns are TEXT (ISO-8601
//! UTC). #130 invariant — sqlx::Any deliberately excludes
//! `chrono::DateTime<Utc>` from its type-compat set.

use chrono::{DateTime, Utc};
use proto_blue::lexicon::{LexiconDoc, Lexicons};
use sqlx::AnyPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;

/// A lexicon document plus the bookkeeping needed to drive cache
/// eviction and re-fetch. `lexicons` is a single-doc registry that
/// proto-blue's `validate_record` can consume directly; the original
/// JSON is preserved for on-disk persistence and re-hydration.
///
/// Manual `Debug` impl elides `lexicons` (proto-blue's `Lexicons`
/// does not implement `Debug`) and the verbose `lexicon_json` — the
/// useful fields for debug output are `nsid` + `authority_did` +
/// timestamps. Test `unwrap_err()` / `expect()` patterns require
/// `Debug` on the success type, so the manual impl is load-bearing.
#[derive(Clone)]
pub struct CachedLexicon {
    /// The NSID this document defines (matches `doc.id`).
    pub nsid: String,
    /// The authority DID resolved from `_lexicon.<host>` (or the
    /// `did_authority` config override).
    pub authority_did: String,
    /// Proto-blue's `Lexicons` registry holding just this doc plus
    /// any nested definitions. Wrapped in `Arc` so cache reads clone
    /// the pointer, not the registry.
    pub lexicons: Arc<Lexicons>,
    /// The parsed lexicon doc itself. Wrapped in `Arc` for cheap
    /// cloning across validate-phase calls.
    pub doc: Arc<LexiconDoc>,
    /// Original JSON text — preserved verbatim for on-disk persist
    /// and rehydration after restart (proto-blue's `LexiconDoc` is
    /// deserialize-only; re-emitting it from the in-memory struct
    /// would require a custom serializer that doesn't exist yet).
    pub lexicon_json: String,
    /// When this entry was fetched (ISO-8601 UTC). Drives TTL
    /// expiry via `expires_at = fetched_at + cache_ttl_secs`.
    pub fetched_at: DateTime<Utc>,
    /// Last validate-phase hit. In-memory updates immediately; on-disk
    /// updates are throttled by
    /// [`crate::config::LexiconConfig::last_used_persist_threshold_secs`].
    pub last_used_at: DateTime<Utc>,
    /// TTL boundary — entries with `expires_at < now()` are stale and
    /// trigger a background re-fetch on next read (the cached value is
    /// served in the interim; see §17.3.1 step 1).
    pub expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for CachedLexicon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedLexicon")
            .field("nsid", &self.nsid)
            .field("authority_did", &self.authority_did)
            .field("fetched_at", &self.fetched_at)
            .field("last_used_at", &self.last_used_at)
            .field("expires_at", &self.expires_at)
            .field("lexicon_json_len", &self.lexicon_json.len())
            .finish_non_exhaustive()
    }
}

impl CachedLexicon {
    /// Construct from a freshly-parsed lexicon doc plus its authority
    /// and the source JSON. `now` and `cache_ttl_secs` set
    /// `fetched_at = last_used_at = now` and
    /// `expires_at = now + cache_ttl_secs`.
    pub fn new(
        nsid: String,
        authority_did: String,
        doc: LexiconDoc,
        lexicons: Lexicons,
        lexicon_json: String,
        now: DateTime<Utc>,
        cache_ttl_secs: u64,
    ) -> Self {
        let expires_at = now + chrono::Duration::seconds(cache_ttl_secs as i64);
        Self {
            nsid,
            authority_did,
            lexicons: Arc::new(lexicons),
            doc: Arc::new(doc),
            lexicon_json,
            fetched_at: now,
            last_used_at: now,
            expires_at,
        }
    }

    /// True when `now` is past `expires_at`. A stale entry is still
    /// served (caller decides whether to trigger background re-fetch);
    /// callers MUST NOT reject solely on staleness.
    pub fn is_stale(&self, now: DateTime<Utc>) -> bool {
        now >= self.expires_at
    }
}

/// Two-layer cache: `Arc<RwLock<HashMap<nsid, CachedLexicon>>>` in-memory,
/// optional `AnyPool` for on-disk persist.
///
/// The pool is optional so unit tests can exercise the in-memory paths
/// without a backing DB. Production wiring at startup passes the real
/// pool; the on-disk persist methods short-circuit when the pool is
/// `None`.
pub struct LexiconCache {
    entries: Arc<RwLock<HashMap<String, CachedLexicon>>>,
    pool: Option<AnyPool>,
    /// Throttle window for on-disk `last_used_at` persists (round-1 F11).
    /// Consumed by `persist_last_used_if_due` below; that path itself is
    /// tests-only today, so the field reads as dead under `--lib`.
    #[allow(dead_code)]
    last_used_persist_threshold_secs: u64,
}

impl LexiconCache {
    /// Construct an in-memory-only cache (no on-disk persist). Useful
    /// for tests; production should use [`Self::with_pool`].
    /// Tests-only consumer today; `--lib` doesn't see tests/.
    #[allow(dead_code)]
    pub fn in_memory(last_used_persist_threshold_secs: u64) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            pool: None,
            last_used_persist_threshold_secs,
        }
    }

    /// Construct a cache wired to a database pool. The pool is shared
    /// (typically the same `account_db` pool used elsewhere); the
    /// `lexicon_cache` table must exist (migrations 0012 SQLite / 0013
    /// Postgres provide it).
    pub fn with_pool(pool: AnyPool, last_used_persist_threshold_secs: u64) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            pool: Some(pool),
            last_used_persist_threshold_secs,
        }
    }

    /// In-memory read. Returns `None` if the NSID isn't cached; returns
    /// the entry regardless of staleness (caller decides whether to
    /// trigger background re-fetch — see §17.3.1 step 1).
    ///
    /// On a hit, updates the in-memory `last_used_at` to `now`. On-disk
    /// `last_used_at` is updated separately via
    /// [`Self::persist_last_used_if_due`] (round-1 F11 throttling).
    pub async fn get(&self, nsid: &str, now: DateTime<Utc>) -> Option<CachedLexicon> {
        let mut guard = self.entries.write().await;
        if let Some(entry) = guard.get_mut(nsid) {
            entry.last_used_at = now;
            Some(entry.clone())
        } else {
            None
        }
    }

    /// In-memory write — replaces any existing entry for the NSID.
    /// Always succeeds; the resolver pairs this with an async on-disk
    /// persist via [`Self::persist`].
    pub async fn insert(&self, entry: CachedLexicon) {
        let mut guard = self.entries.write().await;
        guard.insert(entry.nsid.clone(), entry);
    }

    /// Arc 17 §17.3.7 admin surface — snapshot the in-memory cache
    /// as a Vec, sorted by NSID for deterministic pagination. Cheap
    /// because `CachedLexicon::clone` is `Arc`-pointer cloning, not
    /// deep copying of the lexicon doc.
    ///
    /// Returns `(entries, total_count)`. `cursor` is the last NSID
    /// from the previous page (None for the first page); `limit`
    /// caps the response size. Backwards-compatible with future
    /// pagination on top of the same shape.
    pub async fn snapshot(
        &self,
        cursor: Option<&str>,
        limit: usize,
        nsid_filter: Option<&str>,
    ) -> (Vec<CachedLexicon>, usize) {
        let guard = self.entries.read().await;

        if let Some(nsid) = nsid_filter {
            let total = guard.contains_key(nsid) as usize;
            let entries = guard.get(nsid).cloned().into_iter().collect();
            return (entries, total);
        }

        let mut all: Vec<&CachedLexicon> = guard.values().collect();
        all.sort_by(|a, b| a.nsid.cmp(&b.nsid));
        let total = all.len();

        let start_idx = match cursor {
            Some(c) => all
                .iter()
                .position(|e| e.nsid.as_str() > c)
                .unwrap_or(all.len()),
            None => 0,
        };
        let end_idx = (start_idx + limit).min(all.len());
        let page = all[start_idx..end_idx]
            .iter()
            .map(|e| (*e).clone())
            .collect();
        (page, total)
    }

    /// Arc 17 §17.3.7 admin surface — evict a single NSID from the
    /// in-memory cache. Returns `true` if the entry was present.
    /// Does NOT touch the on-disk row; the next restart will pick
    /// up the on-disk value and re-warm. Operators wanting both
    /// layers cleared should follow with the on-disk DELETE
    /// directly (or just rely on TTL).
    pub async fn evict(&self, nsid: &str) -> bool {
        let mut guard = self.entries.write().await;
        guard.remove(nsid).is_some()
    }

    /// Arc 17 §17.3.7 admin surface — evict every in-memory entry.
    /// Returns the count of entries removed. Same on-disk caveat
    /// as `evict`: the on-disk rows survive and will rehydrate on
    /// next fetch.
    pub async fn evict_all(&self) -> usize {
        let mut guard = self.entries.write().await;
        let count = guard.len();
        guard.clear();
        count
    }

    /// On-disk INSERT/UPDATE for a fresh entry. The resolver spawns this
    /// onto its own task so the validate-phase return isn't blocked
    /// (§17.5.8 async-write-failure-consistency).
    ///
    /// Dual-backend conflict handling (round-1 F6 closure):
    ///   PG:     INSERT ... ON CONFLICT (nsid) DO UPDATE SET ...
    ///   SQLite: INSERT OR REPLACE INTO ...
    ///
    /// Backend detected at runtime via the `pg_backend_pid()` probe
    /// established for the blob_store path. If the pool isn't set
    /// (in-memory-only cache), this is a no-op returning `Ok(())`.
    pub async fn persist(&self, entry: &CachedLexicon) -> Result<(), sqlx::Error> {
        let Some(pool) = &self.pool else {
            return Ok(());
        };
        let is_postgres = detect_postgres(pool).await;
        let sql = if is_postgres {
            "INSERT INTO lexicon_cache \
             (nsid, authority_did, lexicon_json, fetched_at, last_used_at, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (nsid) DO UPDATE SET \
                authority_did = EXCLUDED.authority_did, \
                lexicon_json = EXCLUDED.lexicon_json, \
                fetched_at = EXCLUDED.fetched_at, \
                last_used_at = EXCLUDED.last_used_at, \
                expires_at = EXCLUDED.expires_at"
        } else {
            "INSERT OR REPLACE INTO lexicon_cache \
             (nsid, authority_did, lexicon_json, fetched_at, last_used_at, expires_at) \
             VALUES (?, ?, ?, ?, ?, ?)"
        };

        sqlx::query(sql)
            .bind(&entry.nsid)
            .bind(&entry.authority_did)
            .bind(&entry.lexicon_json)
            .bind(entry.fetched_at.to_rfc3339())
            .bind(entry.last_used_at.to_rfc3339())
            .bind(entry.expires_at.to_rfc3339())
            .execute(pool)
            .await?;
        Ok(())
    }

    /// Throttled on-disk `last_used_at` write (round-1 F11 closure).
    /// Updates the DB row's `last_used_at` ONLY if the new value
    /// exceeds the existing on-disk value by ≥
    /// `last_used_persist_threshold_secs`. Returns `Ok(true)` when an
    /// update fired, `Ok(false)` when throttled (no DB write).
    ///
    /// Forward-substrate — consumer is the resolver's read path,
    /// which short-circuits this in v0.5 to avoid per-fetch DB writes.
    /// Wires up with v0.6 cache-warming work.
    #[allow(dead_code)]
    pub async fn persist_last_used_if_due(
        &self,
        nsid: &str,
        last_used_at: DateTime<Utc>,
    ) -> Result<bool, sqlx::Error> {
        let Some(pool) = &self.pool else {
            return Ok(false);
        };

        let existing: Option<(String,)> =
            sqlx::query_as("SELECT last_used_at FROM lexicon_cache WHERE nsid = ?")
                .bind(nsid)
                .fetch_optional(pool)
                .await?;

        let Some((existing_iso,)) = existing else {
            return Ok(false);
        };

        let existing_dt = match DateTime::parse_from_rfc3339(&existing_iso) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(_) => {
                // Corrupt timestamp on disk — let the next persist()
                // call overwrite it. Don't fire a stale-throttled
                // update that might widen the corruption.
                return Ok(false);
            }
        };
        let delta = (last_used_at - existing_dt).num_seconds();
        if delta < self.last_used_persist_threshold_secs as i64 {
            return Ok(false);
        }

        sqlx::query("UPDATE lexicon_cache SET last_used_at = ? WHERE nsid = ?")
            .bind(last_used_at.to_rfc3339())
            .bind(nsid)
            .execute(pool)
            .await?;
        Ok(true)
    }
}

/// Probe the pool to determine the backend. Mirrors the `is_postgres`
/// detection used in `blob_store/store.rs`. SQLite returns an error on
/// `pg_backend_pid()`; PG returns a row.
async fn detect_postgres(pool: &AnyPool) -> bool {
    sqlx::query("SELECT pg_backend_pid()")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .is_some()
}

/// Helper: emit a structured WARN log when an async on-disk persist
/// fails. §17.5.8 self-healing-via-TTL rationale — the cache stays
/// correct in-memory; the DB repopulates on next fetch. Operators
/// seeing repeated `lexicon_cache_persist_failed` entries for the same
/// NSID should investigate.
pub fn log_persist_failure(nsid: &str, err: &sqlx::Error) {
    warn!(
        event = "lexicon_cache_persist_failed",
        nsid = %nsid,
        error = %err,
        "lexicon cache on-disk persist failed; in-memory entry remains valid for its TTL"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto_blue::lexicon::Lexicons;

    fn fixed_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-23T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn sample_doc_json() -> String {
        r#"{
            "lexicon": 1,
            "id": "com.example.minimal.thing",
            "defs": {
                "main": {
                    "type": "record",
                    "key": "tid",
                    "record": {
                        "type": "object",
                        "required": ["text"],
                        "properties": {
                            "text": { "type": "string" }
                        }
                    }
                }
            }
        }"#
        .to_string()
    }

    fn sample_cached_entry(now: DateTime<Utc>, ttl_secs: u64) -> CachedLexicon {
        let json = sample_doc_json();
        let doc: LexiconDoc = serde_json::from_str(&json).expect("doc parse");
        let mut lex = Lexicons::new();
        lex.add(doc.clone()).expect("lex add");
        CachedLexicon::new(
            doc.id.clone(),
            "did:plc:example".to_string(),
            doc,
            lex,
            json,
            now,
            ttl_secs,
        )
    }

    #[tokio::test]
    async fn in_memory_insert_get_returns_entry() {
        let cache = LexiconCache::in_memory(60);
        let now = fixed_now();
        let entry = sample_cached_entry(now, 3600);
        cache.insert(entry.clone()).await;
        let got = cache.get(&entry.nsid, now).await.expect("hit");
        assert_eq!(got.nsid, entry.nsid);
        assert_eq!(got.authority_did, "did:plc:example");
    }

    #[tokio::test]
    async fn miss_returns_none() {
        let cache = LexiconCache::in_memory(60);
        let got = cache.get("com.example.missing", fixed_now()).await;
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn get_updates_last_used_at_in_memory() {
        let cache = LexiconCache::in_memory(60);
        let t0 = fixed_now();
        let entry = sample_cached_entry(t0, 3600);
        cache.insert(entry.clone()).await;
        let t1 = t0 + chrono::Duration::minutes(5);
        let got = cache.get(&entry.nsid, t1).await.expect("hit");
        assert_eq!(got.last_used_at, t1);
    }

    #[tokio::test]
    async fn is_stale_fires_after_ttl() {
        let t0 = fixed_now();
        let entry = sample_cached_entry(t0, 3600);
        assert!(!entry.is_stale(t0 + chrono::Duration::minutes(30)));
        assert!(entry.is_stale(t0 + chrono::Duration::hours(2)));
    }

    #[tokio::test]
    async fn persist_no_pool_is_noop() {
        let cache = LexiconCache::in_memory(60);
        let entry = sample_cached_entry(fixed_now(), 3600);
        assert!(cache.persist(&entry).await.is_ok());
    }

    #[tokio::test]
    async fn persist_last_used_no_pool_returns_false() {
        let cache = LexiconCache::in_memory(60);
        let fired = cache
            .persist_last_used_if_due("com.example.x", fixed_now())
            .await
            .expect("ok");
        assert!(!fired);
    }
}
