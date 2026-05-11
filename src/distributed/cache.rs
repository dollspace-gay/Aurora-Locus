//! TTL-based per-instance cache for parse-result optimization
//! (V04_DESIGN.md §6.3.4 "What CAN be cached").
//!
//! **This cache is for parse-result optimization only.** It is
//! **not** a substitute for distributed-store consultation when
//! single-use semantics matter. A cache that mirrors the store
//! eventually-consistently is fundamentally incompatible with
//! single-use semantics across instances: instance B's cache
//! saying "valid" while instance A has already deleted the row
//! creates a replay window.
//!
//! Concretely, the legitimate use case in Arc 7 is caching the
//! result of expensive cryptographic work — for example, the
//! parsed `DPopClaims` struct after JWT signature verification.
//! The cache key is the proof bytes (or its SHA-256 prefix);
//! the cache value is the parsed claims. The single-use check
//! (consulting `dpop_jti_replay`) still happens unconditionally
//! per request; the cache only short-circuits the parse step
//! when the same proof is seen again before the TTL elapses.
//!
//! No consumer in Step 1 — this module exists for Step 3 to
//! decide whether parse-caching ships. Step 1 commits to the
//! shape; Step 3 wires it (or doesn't).
//!
//! Implementation: `dashmap::DashMap` keyed by `K`, with values
//! wrapped in a [`CacheEntry`] carrying the absolute expiry.
//! `dashmap` is already a Cargo dep (used elsewhere in the
//! rate-limit middleware); no new dependency is introduced.

use std::hash::Hash;
use std::sync::Arc;

use chrono::Duration;
use dashmap::DashMap;

/// A `(value, expires_at_epoch_ms)` pair. Lives in the cache's
/// internal map; not exposed.
#[derive(Clone)]
struct CacheEntry<V> {
    value: V,
    expires_at_epoch_ms: i64,
}

/// TTL-based per-instance cache.
///
/// Reads return `Some(value)` only when the entry exists AND is
/// not past its TTL. Expired entries are returned as `None` and
/// optionally evicted lazily on access (so a quiet cache doesn't
/// hold expired entries indefinitely).
///
/// `K` must be `Eq + Hash + Clone` (dashmap requirements + the
/// insert signature consumes the key by value). `V` must be
/// `Clone` because reads hand back owned values — locking a
/// shard while the caller does work with the borrowed value
/// would defeat dashmap's whole point.
pub struct TtlCache<K, V> {
    inner: Arc<DashMap<K, CacheEntry<V>>>,
    default_ttl: Duration,
}

impl<K, V> TtlCache<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    /// Construct a cache with the given default TTL. Per-insert
    /// overrides are available via [`Self::insert_with_ttl`].
    pub fn new(default_ttl: Duration) -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
            default_ttl,
        }
    }

    /// Fetch the value associated with `key`, returning `None`
    /// if absent or past its TTL. Past-TTL entries are evicted
    /// lazily to bound memory growth in caches whose hot keys
    /// shift over time.
    pub fn get(&self, key: &K) -> Option<V> {
        let now = chrono::Utc::now().timestamp_millis();
        // First check via a read guard — fast path for the
        // common case (entry present + not expired).
        if let Some(entry) = self.inner.get(key) {
            if entry.expires_at_epoch_ms > now {
                return Some(entry.value.clone());
            }
        }
        // Either absent or expired. Drop the read guard before
        // taking a write guard for eviction; dashmap shard
        // contention rules apply.
        self.inner.remove_if(key, |_, entry| {
            entry.expires_at_epoch_ms <= now
        });
        None
    }

    /// Insert with the default TTL.
    pub fn insert(&self, key: K, value: V) {
        self.insert_with_ttl(key, value, self.default_ttl);
    }

    /// Insert with an explicit TTL, overriding the cache's
    /// default for this entry only. Useful when the caller
    /// knows a tighter bound than the default (e.g., the DPoP
    /// proof's own `exp` claim is shorter than the cache's
    /// default TTL).
    pub fn insert_with_ttl(&self, key: K, value: V, ttl: Duration) {
        let now = chrono::Utc::now().timestamp_millis();
        let expires_at_epoch_ms = now.saturating_add(ttl.num_milliseconds());
        self.inner.insert(
            key,
            CacheEntry {
                value,
                expires_at_epoch_ms,
            },
        );
    }

    /// Number of entries currently in the cache, including any
    /// that are past their TTL but haven't been lazily evicted
    /// yet. Useful for observability (e.g., Prometheus gauge).
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// True iff [`Self::len`] is zero.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Evict all entries whose TTL has elapsed. Periodic call
    /// from a reaper task is optional — the cache evicts
    /// lazily on `get`. This method matters mainly for tests
    /// and for caches with imbalanced access patterns where
    /// some keys go cold without ever being looked up again.
    pub fn evict_expired(&self) -> usize {
        let now = chrono::Utc::now().timestamp_millis();
        let mut evicted = 0;
        self.inner.retain(|_, entry| {
            if entry.expires_at_epoch_ms <= now {
                evicted += 1;
                false
            } else {
                true
            }
        });
        evicted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration as StdDuration;

    #[test]
    fn insert_then_get_returns_value() {
        let cache: TtlCache<String, i32> = TtlCache::new(Duration::seconds(60));
        cache.insert("k".to_string(), 42);
        assert_eq!(cache.get(&"k".to_string()), Some(42));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn get_on_absent_key_is_none() {
        let cache: TtlCache<String, i32> = TtlCache::new(Duration::seconds(60));
        assert_eq!(cache.get(&"absent".to_string()), None);
    }

    #[test]
    fn get_on_expired_entry_returns_none_and_evicts() {
        let cache: TtlCache<String, i32> = TtlCache::new(Duration::milliseconds(50));
        cache.insert("k".to_string(), 9);
        assert_eq!(cache.get(&"k".to_string()), Some(9));
        // Sleep past the TTL.
        sleep(StdDuration::from_millis(80));
        assert_eq!(cache.get(&"k".to_string()), None);
        // Lazy eviction kicked in on the failing get.
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn insert_with_ttl_overrides_default() {
        // Cache default is generous (60s); per-entry override
        // is tight (50ms). The tight entry should expire while
        // a default-TTL entry is still valid.
        let cache: TtlCache<&'static str, &'static str> =
            TtlCache::new(Duration::seconds(60));
        cache.insert("default", "long-lived");
        cache.insert_with_ttl("short", "ephemeral", Duration::milliseconds(50));
        sleep(StdDuration::from_millis(80));
        assert_eq!(cache.get(&"default"), Some("long-lived"));
        assert_eq!(cache.get(&"short"), None);
    }

    #[test]
    fn evict_expired_removes_only_expired() {
        let cache: TtlCache<&'static str, &'static str> =
            TtlCache::new(Duration::seconds(60));
        cache.insert_with_ttl("a", "1", Duration::milliseconds(10));
        cache.insert_with_ttl("b", "2", Duration::seconds(60));
        sleep(StdDuration::from_millis(40));
        let evicted = cache.evict_expired();
        assert_eq!(evicted, 1);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&"b"), Some("2"));
    }

    #[test]
    fn reinsert_updates_value_and_resets_ttl() {
        let cache: TtlCache<&'static str, i32> = TtlCache::new(Duration::seconds(60));
        cache.insert("k", 1);
        cache.insert("k", 2);
        assert_eq!(cache.get(&"k"), Some(2));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn is_empty_reflects_state() {
        let cache: TtlCache<&'static str, ()> = TtlCache::new(Duration::seconds(60));
        assert!(cache.is_empty());
        cache.insert("k", ());
        assert!(!cache.is_empty());
        cache.evict_expired(); // not yet expired — no-op
        assert!(!cache.is_empty());
    }

    #[test]
    fn concurrent_inserts_dont_corrupt() {
        // Smoke-test against the dashmap concurrency guarantees.
        // The cache is shared across N tasks each inserting a
        // distinct key; the post-condition is N entries with no
        // panics or duplicate-replacement weirdness.
        use std::sync::Arc as StdArc;
        let cache: StdArc<TtlCache<i32, i32>> =
            StdArc::new(TtlCache::new(Duration::seconds(60)));
        let mut handles = Vec::new();
        for i in 0..32 {
            let c = StdArc::clone(&cache);
            handles.push(std::thread::spawn(move || {
                c.insert(i, i * 10);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(cache.len(), 32);
        for i in 0..32 {
            assert_eq!(cache.get(&i), Some(i * 10));
        }
    }
}
