/// Cache for local records (read-after-write consistency)
///
/// This cache stores LocalRecords in memory with TTL to avoid repeated database queries.
/// Records are invalidated after TTL expires or when explicitly invalidated on updates.

use super::types::LocalRecords;
use moka::future::Cache;
use std::sync::Arc;
use std::time::Duration;

/// Cache key combining DID and revision
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct CacheKey {
    pub did: String,
    pub since_rev: String,
}

/// LocalRecordsCache manages in-memory caching of read-after-write records
///
/// Uses Moka for LRU cache with TTL. Default TTL is 5 seconds, matching Bluesky PDS.
/// Records are invalidated on any write to that user's repository.
pub struct LocalRecordsCache {
    /// In-memory LRU cache with TTL
    cache: Cache<CacheKey, Arc<LocalRecords>>,

    /// Default TTL for cached records
    ttl: Duration,
}

impl LocalRecordsCache {
    /// Create a new LocalRecordsCache with default settings
    ///
    /// - Max 10,000 entries (covering ~1,000 concurrent users with 10 revisions each)
    /// - 5 second TTL (matching Bluesky PDS)
    /// - LRU eviction policy
    pub fn new() -> Self {
        Self::with_config(10_000, Duration::from_secs(5))
    }

    /// Create a new LocalRecordsCache with custom settings
    pub fn with_config(max_capacity: u64, ttl: Duration) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_capacity)
            .time_to_live(ttl)
            .time_to_idle(ttl) // Also evict if not accessed
            .build();

        Self { cache, ttl }
    }

    /// Get cached records for a DID and revision
    pub async fn get(&self, did: &str, since_rev: &str) -> Option<Arc<LocalRecords>> {
        let key = CacheKey {
            did: did.to_string(),
            since_rev: since_rev.to_string(),
        };

        self.cache.get(&key).await
    }

    /// Cache records for a DID and revision
    pub async fn set(&self, did: &str, since_rev: &str, records: LocalRecords) {
        let key = CacheKey {
            did: did.to_string(),
            since_rev: since_rev.to_string(),
        };

        self.cache.insert(key, Arc::new(records)).await;
    }

    /// Invalidate all cached records for a specific DID
    ///
    /// Called when any record is created/updated/deleted for this user.
    /// This ensures subsequent reads will fetch fresh data from the database.
    pub async fn invalidate_did(&self, did: &str) {
        // Moka doesn't have prefix-based invalidation, so we need to track keys
        // For now, we use invalidate_all which is simple but less efficient
        // Future: maintain a secondary index of DID -> keys for targeted invalidation
        self.cache.invalidate_all();

        tracing::debug!(
            "Invalidated all cached records (DID: {} caused cache clear)",
            did
        );
    }

    /// Invalidate all cached records (nuclear option)
    pub async fn invalidate_all(&self) {
        self.cache.invalidate_all();
        tracing::info!("Invalidated all cached local records");
    }

    /// Get cache statistics
    pub async fn stats(&self) -> CacheStats {
        CacheStats {
            entry_count: self.cache.entry_count(),
            weighted_size: self.cache.weighted_size(),
            ttl_secs: self.ttl.as_secs(),
        }
    }
}

impl Default for LocalRecordsCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub entry_count: u64,
    pub weighted_size: u64,
    pub ttl_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read_after_write::types::LocalRecords;

    #[tokio::test]
    async fn test_cache_set_get() {
        let cache = LocalRecordsCache::new();
        let records = LocalRecords::empty();

        cache.set("did:plc:alice", "rev-123", records.clone()).await;

        let cached = cache.get("did:plc:alice", "rev-123").await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().count, 0);
    }

    #[tokio::test]
    async fn test_cache_invalidate_did() {
        let cache = LocalRecordsCache::new();
        let records = LocalRecords::empty();

        cache.set("did:plc:alice", "rev-123", records).await;

        // Invalidate Alice's records
        cache.invalidate_did("did:plc:alice").await;

        // Should be gone
        let cached = cache.get("did:plc:alice", "rev-123").await;
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn test_cache_ttl() {
        let cache = LocalRecordsCache::with_config(1000, Duration::from_millis(100));
        let records = LocalRecords::empty();

        cache.set("did:plc:alice", "rev-123", records).await;

        // Immediately available
        assert!(cache.get("did:plc:alice", "rev-123").await.is_some());

        // Wait for TTL to expire
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Should be evicted
        assert!(cache.get("did:plc:alice", "rev-123").await.is_none());
    }
}
