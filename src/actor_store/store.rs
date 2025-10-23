/// Actor Store Manager - Handles per-user repository databases
use crate::{
    actor_store::{get_actor_location, models::*, ActorLocation},
    error::{PdsError, PdsResult},
};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Configuration for the actor store
#[derive(Debug, Clone)]
pub struct ActorStoreConfig {
    pub base_directory: PathBuf,
    pub cache_size: usize,
}

impl Default for ActorStoreConfig {
    fn default() -> Self {
        Self {
            base_directory: PathBuf::from("./data/actors"),
            cache_size: 100,
        }
    }
}

/// Actor Store - Manages per-user repositories
#[derive(Clone)]
pub struct ActorStore {
    config: ActorStoreConfig,
    // Cache of open database connections (LRU-style)
    db_cache: Arc<RwLock<HashMap<String, SqlitePool>>>,
}

impl ActorStore {
    /// Create a new actor store
    pub fn new(config: ActorStoreConfig) -> Self {
        Self {
            config,
            db_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get the location information for a DID
    pub fn get_location(&self, did: &str) -> ActorLocation {
        get_actor_location(&self.config.base_directory, did)
    }

    /// Check if an actor's repository exists
    pub async fn exists(&self, did: &str) -> bool {
        let location = self.get_location(did);
        location.db_location.exists()
    }

    /// Create a new actor repository
    pub async fn create(&self, did: &str) -> PdsResult<()> {
        let location = self.get_location(did);

        // Create directory structure
        tokio::fs::create_dir_all(&location.directory).await?;

        // Create database
        let pool = self.open_db(did).await?;

        // Run migrations to create schema
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|e| PdsError::Internal(format!("Actor DB migration failed: {}", e)))?;

        // Initialize empty repository root
        sqlx::query(
            "INSERT INTO repo_root (did, cid, rev, indexed_at)
             VALUES (?1, ?2, ?3, ?4)"
        )
        .bind(did)
        .bind("bafyreihk5ztsfapt6g2cnxbxgbxb7dltipq5pufb4jtwmqrxrxqaygceyq") // Empty MST root
        .bind("3jzfcijpj2z2a") // Initial TID
        .bind(chrono::Utc::now())
        .execute(&pool)
        .await?;

        Ok(())
    }

    /// Open a connection to an actor's database
    pub async fn open_db(&self, did: &str) -> PdsResult<SqlitePool> {
        // Check cache first
        {
            let cache = self.db_cache.read().await;
            if let Some(pool) = cache.get(did) {
                return Ok(pool.clone());
            }
        }

        // Not in cache, open connection
        let location = self.get_location(did);

        if !location.db_location.exists() {
            return Err(PdsError::NotFound(format!("Actor repository not found for {}", did)));
        }

        let pool = SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&location.db_location)
                .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                .foreign_keys(true)
                .busy_timeout(std::time::Duration::from_secs(5)),
        )
        .await
        .map_err(|e| PdsError::Database(e))?;

        // Add to cache
        {
            let mut cache = self.db_cache.write().await;

            // Simple cache eviction: if cache is full, remove oldest entry
            if cache.len() >= self.config.cache_size {
                if let Some(key) = cache.keys().next().cloned() {
                    cache.remove(&key);
                }
            }

            cache.insert(did.to_string(), pool.clone());
        }

        Ok(pool)
    }

    /// Get the current repository root
    pub async fn get_repo_root(&self, did: &str) -> PdsResult<RepoRoot> {
        let pool = self.open_db(did).await?;

        let root = sqlx::query(
            "SELECT did, cid, rev, indexed_at FROM repo_root WHERE did = ?1"
        )
        .bind(did)
        .fetch_optional(&pool)
        .await?
        .ok_or_else(|| PdsError::NotFound("Repository root not found".to_string()))?;

        Ok(RepoRoot {
            did: root.get("did"),
            cid: root.get("cid"),
            rev: root.get("rev"),
            indexed_at: root.get("indexed_at"),
        })
    }

    /// Update repository root
    pub async fn update_repo_root(&self, did: &str, cid: &str, rev: &str) -> PdsResult<()> {
        let pool = self.open_db(did).await?;

        sqlx::query(
            "UPDATE repo_root SET cid = ?1, rev = ?2, indexed_at = ?3 WHERE did = ?4"
        )
        .bind(cid)
        .bind(rev)
        .bind(chrono::Utc::now())
        .bind(did)
        .execute(&pool)
        .await?;

        Ok(())
    }

    /// Get a record by URI
    pub async fn get_record(&self, did: &str, uri: &str) -> PdsResult<Option<Record>> {
        let pool = self.open_db(did).await?;

        let record = sqlx::query(
            "SELECT uri, cid, collection, rkey, repo_rev, indexed_at, takedown_ref
             FROM record
             WHERE uri = ?1"
        )
        .bind(uri)
        .fetch_optional(&pool)
        .await?;

        if let Some(row) = record {
            Ok(Some(Record {
                uri: row.get("uri"),
                cid: row.get("cid"),
                collection: row.get("collection"),
                rkey: row.get("rkey"),
                repo_rev: row.get("repo_rev"),
                indexed_at: row.get("indexed_at"),
                takedown_ref: row.get("takedown_ref"),
            }))
        } else {
            Ok(None)
        }
    }

    /// List records in a collection
    pub async fn list_records(
        &self,
        did: &str,
        collection: &str,
        limit: i64,
        cursor: Option<&str>,
    ) -> PdsResult<Vec<Record>> {
        let pool = self.open_db(did).await?;

        let query = if let Some(cursor) = cursor {
            sqlx::query(
                "SELECT uri, cid, collection, rkey, repo_rev, indexed_at, takedown_ref
                 FROM record
                 WHERE collection = ?1 AND rkey > ?2
                 ORDER BY rkey ASC
                 LIMIT ?3"
            )
            .bind(collection)
            .bind(cursor)
            .bind(limit)
        } else {
            sqlx::query(
                "SELECT uri, cid, collection, rkey, repo_rev, indexed_at, takedown_ref
                 FROM record
                 WHERE collection = ?1
                 ORDER BY rkey ASC
                 LIMIT ?2"
            )
            .bind(collection)
            .bind(limit)
        };

        let rows = query.fetch_all(&pool).await?;

        let records = rows
            .into_iter()
            .map(|row| Record {
                uri: row.get("uri"),
                cid: row.get("cid"),
                collection: row.get("collection"),
                rkey: row.get("rkey"),
                repo_rev: row.get("repo_rev"),
                indexed_at: row.get("indexed_at"),
                takedown_ref: row.get("takedown_ref"),
            })
            .collect();

        Ok(records)
    }

    /// Create or update a record
    pub async fn put_record(
        &self,
        did: &str,
        uri: &str,
        cid: &str,
        collection: &str,
        rkey: &str,
        repo_rev: &str,
    ) -> PdsResult<()> {
        let pool = self.open_db(did).await?;

        sqlx::query(
            "INSERT INTO record (uri, cid, collection, rkey, repo_rev, indexed_at, takedown_ref)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)
             ON CONFLICT(uri) DO UPDATE SET
                cid = excluded.cid,
                repo_rev = excluded.repo_rev,
                indexed_at = excluded.indexed_at"
        )
        .bind(uri)
        .bind(cid)
        .bind(collection)
        .bind(rkey)
        .bind(repo_rev)
        .bind(chrono::Utc::now())
        .execute(&pool)
        .await?;

        Ok(())
    }

    /// Delete a record
    pub async fn delete_record(&self, did: &str, uri: &str) -> PdsResult<()> {
        let pool = self.open_db(did).await?;

        sqlx::query("DELETE FROM record WHERE uri = ?1")
            .bind(uri)
            .execute(&pool)
            .await?;

        Ok(())
    }

    /// Count records in a collection
    pub async fn count_records(&self, did: &str, collection: &str) -> PdsResult<i64> {
        let pool = self.open_db(did).await?;

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM record WHERE collection = ?1"
        )
        .bind(collection)
        .fetch_one(&pool)
        .await?;

        Ok(count)
    }

    /// Destroy an actor's repository (delete all data)
    pub async fn destroy(&self, did: &str) -> PdsResult<()> {
        let location = self.get_location(did);

        // Remove from cache
        {
            let mut cache = self.db_cache.write().await;
            cache.remove(did);
        }

        // Delete directory
        if location.directory.exists() {
            tokio::fs::remove_dir_all(&location.directory).await?;
        }

        Ok(())
    }
}
