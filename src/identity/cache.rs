/// DID Cache - Database layer for caching DID documents and handle mappings
use crate::{
    error::{PdsError, PdsResult},
    identity::{CachedDidDoc, CachedHandle},
};
use chrono::{DateTime, Duration, Utc};
use sqlx::{Row, SqlitePool};

/// DID cache manager
#[derive(Clone)]
pub struct DidCache {
    db: SqlitePool,
    /// TTL for DID document cache (default: 1 hour)
    did_doc_ttl: Duration,
    /// TTL for handle cache (default: 5 minutes)
    handle_ttl: Duration,
}

impl DidCache {
    /// Create a new DID cache
    pub fn new(db: SqlitePool) -> Self {
        Self {
            db,
            did_doc_ttl: Duration::hours(1),
            handle_ttl: Duration::minutes(5),
        }
    }

    /// Set custom TTLs
    pub fn with_ttls(mut self, did_doc_ttl: Duration, handle_ttl: Duration) -> Self {
        self.did_doc_ttl = did_doc_ttl;
        self.handle_ttl = handle_ttl;
        self
    }

    /// Get cached DID document
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
        .map_err(|e| PdsError::Database(e))?;

        if let Some(row) = result {
            let cached_doc = CachedDidDoc {
                did: row.try_get("did")?,
                doc: row.try_get("doc")?,
                updated_at: parse_timestamp(&row.try_get::<String, _>("updated_at")?)?,
                cached_at: parse_timestamp(&row.try_get::<String, _>("cached_at")?)?,
            };

            // Check if cache is still valid
            if Utc::now() - cached_doc.cached_at < self.did_doc_ttl {
                return Ok(Some(cached_doc));
            } else {
                // Cache expired, delete it
                self.delete_did_doc(did).await?;
                return Ok(None);
            }
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
        .map_err(|e| PdsError::Database(e))?;

        Ok(())
    }

    /// Delete DID document from cache
    pub async fn delete_did_doc(&self, did: &str) -> PdsResult<()> {
        sqlx::query("DELETE FROM did_doc WHERE did = ?1")
            .bind(did)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        Ok(())
    }

    /// Get cached handle mapping
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
        .map_err(|e| PdsError::Database(e))?;

        if let Some(row) = result {
            let cached_handle = CachedHandle {
                handle: row.try_get("handle")?,
                did: row.try_get("did")?,
                declared_at: row
                    .try_get::<Option<String>, _>("declared_at")?
                    .and_then(|s| parse_timestamp(&s).ok()),
                updated_at: parse_timestamp(&row.try_get::<String, _>("updated_at")?)?,
            };

            // Check if cache is still valid
            if Utc::now() - cached_handle.updated_at < self.handle_ttl {
                return Ok(Some(cached_handle));
            } else {
                // Cache expired, delete it
                self.delete_handle(handle).await?;
                return Ok(None);
            }
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
        .map_err(|e| PdsError::Database(e))?;

        Ok(())
    }

    /// Delete handle from cache
    pub async fn delete_handle(&self, handle: &str) -> PdsResult<()> {
        let normalized = handle.to_lowercase();

        sqlx::query("DELETE FROM did_handle WHERE handle = ?1")
            .bind(&normalized)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

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
        .map_err(|e| PdsError::Database(e))?;

        if let Some(row) = result {
            Ok(Some(row.try_get("handle")?))
        } else {
            Ok(None)
        }
    }

    /// Clean up expired cache entries
    pub async fn cleanup_expired(&self) -> PdsResult<()> {
        let did_doc_cutoff = (Utc::now() - self.did_doc_ttl).to_rfc3339();
        let handle_cutoff = (Utc::now() - self.handle_ttl).to_rfc3339();

        // Delete expired DID documents
        sqlx::query("DELETE FROM did_doc WHERE cached_at < ?1")
            .bind(&did_doc_cutoff)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        // Delete expired handles
        sqlx::query("DELETE FROM did_handle WHERE updated_at < ?1")
            .bind(&handle_cutoff)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

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
        assert_eq!(cached.unwrap().doc, doc);
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
        assert_eq!(cached.as_ref().unwrap().did, did);

        // Test case-insensitive lookup
        let cached_upper = cache.get_handle("ALICE.TEST").await.unwrap();
        assert!(cached_upper.is_some());
        assert_eq!(cached_upper.unwrap().did, did);
    }

    #[tokio::test]
    async fn test_reverse_handle_lookup() {
        let cache = create_test_cache().await;

        cache.cache_handle("bob.test", "did:plc:bob").await.unwrap();

        let handle = cache.get_did_handle("did:plc:bob").await.unwrap();
        assert_eq!(handle, Some("bob.test".to_string()));
    }
}
