// Allow dead_code - identity cache features for future use
#![allow(dead_code)]

/// DID Cache - Database layer for caching DID documents and handle mappings
use crate::{
    error::{PdsError, PdsResult},
    identity::{CachedDidDoc, CachedHandle},
};
use chrono::{DateTime, Duration, Utc};
use sqlx::{Row, SqlitePool};

/// DID cache manager with two-tier TTL support
///
/// Implements stale-while-revalidate caching for graceful degradation:
/// - **Stale TTL**: Data is fresh and served immediately
/// - **Max TTL**: Data is stale but usable as fallback during outages
///
/// When data is between stale_ttl and max_ttl:
/// - It's marked as `stale: true`
/// - Can still be used if fresh fetch fails (graceful degradation)
/// - Should trigger background refresh
#[derive(Clone)]
pub struct DidCache {
    db: SqlitePool,
    /// Fresh TTL for DID documents (default: 1 hour)
    /// Data is fresh and served immediately within this period
    did_doc_stale_ttl: Duration,
    /// Maximum TTL for DID documents (default: 24 hours)
    /// Data can still be used as fallback between stale_ttl and max_ttl
    did_doc_max_ttl: Duration,
    /// Fresh TTL for handle cache (default: 5 minutes)
    handle_stale_ttl: Duration,
    /// Maximum TTL for handle cache (default: 1 hour)
    handle_max_ttl: Duration,
}

impl DidCache {
    /// Create a new DID cache with default TTLs
    ///
    /// Defaults:
    /// - DID doc stale: 1 hour, max: 24 hours
    /// - Handle stale: 5 minutes, max: 1 hour
    pub fn new(db: SqlitePool) -> Self {
        Self {
            db,
            did_doc_stale_ttl: Duration::hours(1),
            did_doc_max_ttl: Duration::hours(24),
            handle_stale_ttl: Duration::minutes(5),
            handle_max_ttl: Duration::hours(1),
        }
    }

    /// Set custom TTLs for DID documents
    pub fn with_did_doc_ttls(mut self, stale_ttl: Duration, max_ttl: Duration) -> Self {
        self.did_doc_stale_ttl = stale_ttl;
        self.did_doc_max_ttl = max_ttl;
        self
    }

    /// Set custom TTLs for handles
    pub fn with_handle_ttls(mut self, stale_ttl: Duration, max_ttl: Duration) -> Self {
        self.handle_stale_ttl = stale_ttl;
        self.handle_max_ttl = max_ttl;
        self
    }

    /// Set custom TTLs for all cache types (backward compatibility)
    #[deprecated(note = "Use with_did_doc_ttls() and with_handle_ttls() for two-tier TTL")]
    pub fn with_ttls(mut self, did_doc_ttl: Duration, handle_ttl: Duration) -> Self {
        self.did_doc_stale_ttl = did_doc_ttl;
        self.did_doc_max_ttl = did_doc_ttl * 24; // Max TTL = 24x stale TTL
        self.handle_stale_ttl = handle_ttl;
        self.handle_max_ttl = handle_ttl * 12; // Max TTL = 12x stale TTL
        self
    }

    /// Get cached DID document with stale detection
    ///
    /// Returns:
    /// - `Some(doc)` with `stale=false` if within stale_ttl (fresh)
    /// - `Some(doc)` with `stale=true` if past stale_ttl but within max_ttl
    /// - `None` if past max_ttl (truly expired)
    ///
    /// Stale data can be used as fallback during PLC outages for graceful degradation.
    pub async fn get_did_doc(&self, did: &str) -> PdsResult<Option<CachedDidDoc>> {
        let result = sqlx::query(
            r#"
            SELECT did, doc, updated_at, cached_at
            FROM did_doc
            WHERE did = ?1
            "#,
        )
        .bind(did)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?;

        if let Some(row) = result {
            let did_str: String = row.try_get("did")?;
            let doc: String = row.try_get("doc")?;
            let updated_at = parse_timestamp(&row.try_get::<String, _>("updated_at")?)?;
            let cached_at = parse_timestamp(&row.try_get::<String, _>("cached_at")?)?;

            let age = Utc::now() - cached_at;

            // Check if past max TTL (truly expired)
            if age >= self.did_doc_max_ttl {
                // Delete expired entry
                self.delete_did_doc(did).await?;
                return Ok(None);
            }

            // Determine if stale (past stale_ttl but within max_ttl)
            let is_stale = age >= self.did_doc_stale_ttl;

            return Ok(Some(CachedDidDoc {
                did: did_str,
                doc,
                updated_at,
                cached_at,
                stale: is_stale,
            }));
        }

        Ok(None)
    }

    /// Cache DID document
    pub async fn cache_did_doc(&self, did: &str, doc: &str) -> PdsResult<()> {
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            r#"
            INSERT INTO did_doc (did, doc, updated_at, cached_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(did) DO UPDATE SET
                doc = excluded.doc,
                updated_at = excluded.updated_at,
                cached_at = excluded.cached_at
            "#,
        )
        .bind(did)
        .bind(doc)
        .bind(&now)
        .bind(&now)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;

        Ok(())
    }

    /// Delete DID document from cache
    pub async fn delete_did_doc(&self, did: &str) -> PdsResult<()> {
        sqlx::query("DELETE FROM did_doc WHERE did = ?1")
            .bind(did)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        Ok(())
    }

    /// Get cached handle mapping with stale detection
    ///
    /// Returns:
    /// - `Some(handle)` with `stale=false` if within stale_ttl (fresh)
    /// - `Some(handle)` with `stale=true` if past stale_ttl but within max_ttl
    /// - `None` if past max_ttl (truly expired)
    ///
    /// Stale data can be used as fallback during DNS/HTTP outages for graceful degradation.
    pub async fn get_handle(&self, handle: &str) -> PdsResult<Option<CachedHandle>> {
        let normalized = handle.to_lowercase();

        let result = sqlx::query(
            r#"
            SELECT handle, did, declared_at, updated_at
            FROM did_handle
            WHERE handle = ?1
            "#,
        )
        .bind(&normalized)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?;

        if let Some(row) = result {
            let handle_str: String = row.try_get("handle")?;
            let did: String = row.try_get("did")?;
            let declared_at = row
                .try_get::<Option<String>, _>("declared_at")?
                .and_then(|s| parse_timestamp(&s).ok());
            let updated_at = parse_timestamp(&row.try_get::<String, _>("updated_at")?)?;

            let age = Utc::now() - updated_at;

            // Check if past max TTL (truly expired)
            if age >= self.handle_max_ttl {
                // Delete expired entry
                self.delete_handle(handle).await?;
                return Ok(None);
            }

            // Determine if stale (past stale_ttl but within max_ttl)
            let is_stale = age >= self.handle_stale_ttl;

            return Ok(Some(CachedHandle {
                handle: handle_str,
                did,
                declared_at,
                updated_at,
                stale: is_stale,
            }));
        }

        Ok(None)
    }

    /// Cache handle mapping
    pub async fn cache_handle(&self, handle: &str, did: &str) -> PdsResult<()> {
        let normalized = handle.to_lowercase();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            r#"
            INSERT INTO did_handle (handle, did, updated_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(handle) DO UPDATE SET
                did = excluded.did,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&normalized)
        .bind(did)
        .bind(&now)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;

        Ok(())
    }

    /// Delete handle from cache
    pub async fn delete_handle(&self, handle: &str) -> PdsResult<()> {
        let normalized = handle.to_lowercase();

        sqlx::query("DELETE FROM did_handle WHERE handle = ?1")
            .bind(&normalized)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        Ok(())
    }

    /// Get handle for a DID (reverse lookup)
    pub async fn get_did_handle(&self, did: &str) -> PdsResult<Option<String>> {
        let result = sqlx::query(
            r#"
            SELECT handle
            FROM did_handle
            WHERE did = ?1
            ORDER BY updated_at DESC
            LIMIT 1
            "#,
        )
        .bind(did)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?;

        if let Some(row) = result {
            Ok(Some(row.try_get("handle")?))
        } else {
            Ok(None)
        }
    }

    /// Clean up expired cache entries (past max_ttl only)
    ///
    /// Only deletes entries that are past max_ttl.
    /// Stale entries (between stale_ttl and max_ttl) are kept for fallback.
    pub async fn cleanup_expired(&self) -> PdsResult<()> {
        let did_doc_cutoff = (Utc::now() - self.did_doc_max_ttl).to_rfc3339();
        let handle_cutoff = (Utc::now() - self.handle_max_ttl).to_rfc3339();

        // Delete DID documents past max_ttl
        let did_result = sqlx::query("DELETE FROM did_doc WHERE cached_at < ?1")
            .bind(&did_doc_cutoff)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        // Delete handles past max_ttl
        let handle_result = sqlx::query("DELETE FROM did_handle WHERE updated_at < ?1")
            .bind(&handle_cutoff)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        tracing::debug!(
            did_docs_deleted = did_result.rows_affected(),
            handles_deleted = handle_result.rows_affected(),
            "Cleaned up expired cache entries"
        );

        Ok(())
    }
}

/// Parse RFC3339 timestamp
fn parse_timestamp(s: &str) -> PdsResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_test_cache() -> DidCache {
        let db = SqlitePool::connect(":memory:").await.unwrap();

        // Create tables
        sqlx::query(
            r#"
            CREATE TABLE did_doc (
                did TEXT PRIMARY KEY,
                doc TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                cached_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE did_handle (
                handle TEXT PRIMARY KEY,
                did TEXT NOT NULL,
                declared_at TEXT,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        DidCache::new(db)
    }

    #[tokio::test]
    async fn test_cache_and_get_did_doc() {
        let cache = create_test_cache().await;

        let did = "did:plc:test123";
        let doc = r#"{"id":"did:plc:test123"}"#;

        // Cache document
        cache.cache_did_doc(did, doc).await.unwrap();

        // Retrieve it
        let cached = cache.get_did_doc(did).await.unwrap();
        assert!(cached.is_some());
        let cached_doc = cached.unwrap();
        assert_eq!(cached_doc.doc, doc);
        assert!(!cached_doc.stale); // Should be fresh
    }

    #[tokio::test]
    async fn test_cache_and_get_handle() {
        let cache = create_test_cache().await;

        let handle = "alice.test";
        let did = "did:plc:alice123";

        // Cache handle
        cache.cache_handle(handle, did).await.unwrap();

        // Retrieve it
        let cached = cache.get_handle(handle).await.unwrap();
        assert!(cached.is_some());
        let cached_handle = cached.unwrap();
        assert_eq!(cached_handle.did, did);
        assert!(!cached_handle.stale); // Should be fresh

        // Test case-insensitive lookup
        let cached_upper = cache.get_handle("ALICE.TEST").await.unwrap();
        assert!(cached_upper.is_some());
        assert!(!cached_upper.unwrap().stale); // Should be fresh
    }

    #[tokio::test]
    async fn test_reverse_handle_lookup() {
        let cache = create_test_cache().await;

        cache.cache_handle("bob.test", "did:plc:bob").await.unwrap();

        let handle = cache.get_did_handle("did:plc:bob").await.unwrap();
        assert_eq!(handle, Some("bob.test".to_string()));
    }

    // ========== Tests for Stale TTL Tier ==========

    #[tokio::test]
    async fn test_stale_did_doc_detection() {
        let db = SqlitePool::connect(":memory:").await.unwrap();

        // Create tables
        sqlx::query(
            r#"
            CREATE TABLE did_doc (
                did TEXT PRIMARY KEY,
                doc TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                cached_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        // Create cache with short TTLs: stale=1s, max=10s
        let cache = DidCache::new(db)
            .with_did_doc_ttls(Duration::seconds(1), Duration::seconds(10));

        let did = "did:plc:staletest";
        let doc = r#"{"id":"did:plc:staletest"}"#;

        // Cache document
        cache.cache_did_doc(did, doc).await.unwrap();

        // Immediately fetch - should be fresh
        let cached = cache.get_did_doc(did).await.unwrap();
        assert!(cached.is_some());
        assert!(!cached.unwrap().stale);

        // Wait for stale TTL (1 second)
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        // Fetch again - should be stale but present
        let cached_stale = cache.get_did_doc(did).await.unwrap();
        assert!(cached_stale.is_some());
        let doc_stale = cached_stale.unwrap();
        assert!(doc_stale.stale); // Should be marked as stale
        assert_eq!(doc_stale.doc, doc); // Data should still be available

        // Wait for max TTL (10 seconds total)
        tokio::time::sleep(tokio::time::Duration::from_secs(9)).await;

        // Fetch again - should be expired (None)
        let cached_expired = cache.get_did_doc(did).await.unwrap();
        assert!(cached_expired.is_none()); // Should be completely expired
    }

    #[tokio::test]
    async fn test_stale_handle_detection() {
        let db = SqlitePool::connect(":memory:").await.unwrap();

        // Create tables
        sqlx::query(
            r#"
            CREATE TABLE did_handle (
                handle TEXT PRIMARY KEY,
                did TEXT NOT NULL,
                declared_at TEXT,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        // Create cache with short TTLs: stale=1s, max=5s
        let cache = DidCache::new(db)
            .with_handle_ttls(Duration::seconds(1), Duration::seconds(5));

        let handle = "stale.test";
        let did = "did:plc:stalehandle";

        // Cache handle
        cache.cache_handle(handle, did).await.unwrap();

        // Immediately fetch - should be fresh
        let cached = cache.get_handle(handle).await.unwrap();
        assert!(cached.is_some());
        assert!(!cached.unwrap().stale);

        // Wait for stale TTL
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        // Fetch again - should be stale but present
        let cached_stale = cache.get_handle(handle).await.unwrap();
        assert!(cached_stale.is_some());
        let handle_stale = cached_stale.unwrap();
        assert!(handle_stale.stale); // Should be marked as stale
        assert_eq!(handle_stale.did, did); // Data should still be available

        // Wait for max TTL
        tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;

        // Fetch again - should be expired (None)
        let cached_expired = cache.get_handle(handle).await.unwrap();
        assert!(cached_expired.is_none()); // Should be completely expired
    }

    #[tokio::test]
    async fn test_cleanup_preserves_stale_entries() {
        let db = SqlitePool::connect(":memory:").await.unwrap();

        // Create tables
        sqlx::query(
            r#"
            CREATE TABLE did_doc (
                did TEXT PRIMARY KEY,
                doc TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                cached_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        // Create cache: stale=1s, max=10s
        let cache = DidCache::new(db)
            .with_did_doc_ttls(Duration::seconds(1), Duration::seconds(10));

        let did = "did:plc:cleanuptest";
        let doc = r#"{"id":"did:plc:cleanuptest"}"#;

        // Cache document
        cache.cache_did_doc(did, doc).await.unwrap();

        // Wait for stale TTL
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        // Run cleanup - should NOT delete stale entries
        cache.cleanup_expired().await.unwrap();

        // Stale entry should still be available
        let cached = cache.get_did_doc(did).await.unwrap();
        assert!(cached.is_some());
        assert!(cached.unwrap().stale);
    }
}
