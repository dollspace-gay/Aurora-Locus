//! Database layer for Aurora Locus
//!
//! Manages database connections, migrations, and provides typed access
//! to the account, sequencer, and DID cache databases.

pub mod account;
pub mod postgres;

use crate::error::{PdsError, PdsResult};
use sqlx::sqlite::SqlitePool;
use std::path::Path;

/// Database connection options
#[derive(Debug, Clone)]
pub struct DatabaseOptions {
    #[allow(dead_code)] // Future connection pool configuration
    pub max_connections: u32,
    pub enable_wal: bool,
}

impl Default for DatabaseOptions {
    fn default() -> Self {
        Self {
            max_connections: 10,
            enable_wal: true,
        }
    }
}

/// Create a SQLite connection pool
pub async fn create_pool(path: &Path, options: DatabaseOptions) -> PdsResult<SqlitePool> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let pool = SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(if options.enable_wal {
                sqlx::sqlite::SqliteJournalMode::Wal
            } else {
                sqlx::sqlite::SqliteJournalMode::Delete
            })
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(5)),
    )
    .await
    .map_err(PdsError::Database)?;

    Ok(pool)
}

/// Run migrations for a database
/// Migrations are embedded at compile time from ./migrations directory
pub async fn run_migrations(pool: &SqlitePool) -> PdsResult<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(|e| PdsError::Internal(format!("Migration failed: {}", e)))?;

    Ok(())
}

/// Test database connection
pub async fn test_connection(pool: &SqlitePool) -> PdsResult<()> {
    sqlx::query("SELECT 1")
        .execute(pool)
        .await
        .map_err(PdsError::Database)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_wal_mode_enabled() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        // Create pool with WAL enabled
        let pool = create_pool(
            &db_path,
            DatabaseOptions {
                max_connections: 5,
                enable_wal: true,
            },
        )
        .await
        .unwrap();

        // Query the journal mode to verify WAL is enabled
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(
            journal_mode.to_lowercase(),
            "wal",
            "WAL mode should be enabled"
        );

        // Verify foreign keys are enabled
        let foreign_keys: i32 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(foreign_keys, 1, "Foreign keys should be enabled");
    }

    #[tokio::test]
    async fn test_wal_mode_disabled() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_no_wal.db");

        // Create pool with WAL disabled
        let pool = create_pool(
            &db_path,
            DatabaseOptions {
                max_connections: 5,
                enable_wal: false,
            },
        )
        .await
        .unwrap();

        // Query the journal mode to verify WAL is NOT enabled
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(
            journal_mode.to_lowercase(),
            "delete",
            "DELETE mode should be used when WAL is disabled"
        );
    }

    #[tokio::test]
    async fn test_wal_checkpoint_configuration() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_checkpoint.db");

        // Create pool with WAL enabled
        let pool = create_pool(
            &db_path,
            DatabaseOptions {
                max_connections: 5,
                enable_wal: true,
            },
        )
        .await
        .unwrap();

        // Set checkpoint configuration
        sqlx::query("PRAGMA wal_autocheckpoint = 1000")
            .execute(&pool)
            .await
            .unwrap();

        // Verify checkpoint is set
        let checkpoint: i32 = sqlx::query_scalar("PRAGMA wal_autocheckpoint")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(
            checkpoint, 1000,
            "WAL autocheckpoint should be set to 1000 pages"
        );

        // `PRAGMA synchronous` is a per-connection setting in SQLite —
        // setting it on the pool affects only the connection that ran
        // the query, and the read-back may come from a different one.
        // Acquire and reuse a single connection so the assertion is
        // actually meaningful.
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("PRAGMA synchronous = NORMAL")
            .execute(&mut *conn)
            .await
            .unwrap();

        let sync_mode: i32 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&mut *conn)
            .await
            .unwrap();

        // NORMAL = 1, FULL = 2
        assert_eq!(sync_mode, 1, "Synchronous should be set to NORMAL (1)");
    }

    #[tokio::test]
    async fn test_concurrent_reads_with_wal() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_concurrent.db");

        // Create pool with WAL enabled
        let pool = create_pool(
            &db_path,
            DatabaseOptions {
                max_connections: 10,
                enable_wal: true,
            },
        )
        .await
        .unwrap();

        // Create a test table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS test_cache (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        // Insert test data
        sqlx::query("INSERT INTO test_cache (key, value) VALUES ('test', 'data')")
            .execute(&pool)
            .await
            .unwrap();

        // Spawn multiple concurrent reads
        let mut handles = vec![];
        for i in 0..10 {
            let pool_clone = pool.clone();
            let handle = tokio::spawn(async move {
                let result: String =
                    sqlx::query_scalar("SELECT value FROM test_cache WHERE key = 'test'")
                        .fetch_one(&pool_clone)
                        .await
                        .unwrap();
                assert_eq!(result, "data");
                i
            });
            handles.push(handle);
        }

        // All reads should succeed concurrently
        for handle in handles {
            handle.await.unwrap();
        }
    }
}
