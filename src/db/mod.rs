//! Database layer for Aurora Locus
//!
//! Manages database connections, migrations, and provides typed access
//! to the account, sequencer, and DID cache databases.

pub mod account;
pub mod postgres;

use crate::config::{DatabaseBackend, DatabaseConfig};
use crate::error::{PdsError, PdsResult};
use sqlx::any::AnyPoolOptions;
use sqlx::sqlite::SqlitePool;
use sqlx::AnyPool;
use sqlx::Row;
use std::path::Path;
use std::time::Duration;

/// Portable boolean column read across SQLite (INTEGER 0/1) and
/// Postgres (BOOLEAN) backends via `sqlx::Any` (chainlink #96).
///
/// Phase 3 (b851678) standardized boolean reads on the
/// `row.get::<i64, _>(col) != 0` pattern because SQLite migration
/// columns are INTEGER-typed and `sqlx::Any` couldn't decode INTEGER
/// directly into `bool` when the column type was declared INTEGER.
/// That pattern fails on Postgres, where the same logical columns
/// are real `BOOLEAN` and `sqlx::Any` rejects `i64` decoding from
/// `BOOLEAN` with `ColumnDecode`.
///
/// `read_bool` papers over the difference: try `bool` first
/// (Postgres BOOLEAN succeeds; SQLite INTEGER may also succeed via
/// the Any driver's loose typing), then fall back to `i64 != 0`
/// (legacy SQLite INTEGER columns where the bool decode refuses).
/// Single decision point for all current and future bool reads.
pub fn read_bool(row: &sqlx::any::AnyRow, col: &str) -> Result<bool, sqlx::Error> {
    match row.try_get::<bool, _>(col) {
        Ok(b) => Ok(b),
        Err(_) => row.try_get::<i64, _>(col).map(|i| i != 0),
    }
}

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

// ============================================================================
// Backend-dispatching pool / migration entry points (Phase 2 / chainlink #75).
//
// The functions below return / operate on `AnyPool`, sqlx's driver-agnostic
// pool. Phase 3 (#76) will switch the 16 Group A+B managers from
// `SqlitePool` to `AnyPool` per file. Until then these factories exist
// alongside the legacy SQLite-only paths above; AppContext continues to
// construct the legacy `SqlitePool` for now.
// ============================================================================

/// Resolve the connection URL for an `AnyPool` from a `DatabaseConfig`.
/// SQLite gets a `sqlite://` prefix and `?mode=rwc` so the file is
/// created if missing, matching the legacy `create_pool` behaviour.
#[allow(dead_code)] // Phase 3 (#76) wires this into AppContext.
fn any_url_for(config: &DatabaseConfig, fallback_sqlite_path: &Path) -> String {
    match config.backend {
        DatabaseBackend::Sqlite => {
            let path = config.url.as_deref().unwrap_or_else(|| {
                fallback_sqlite_path
                    .to_str()
                    .expect("SQLite path must be valid UTF-8")
            });
            format!("sqlite://{}?mode=rwc", path)
        }
        DatabaseBackend::Postgres => config
            .url
            .clone()
            .expect("postgres backend requires a URL; validated at config load"),
    }
}

/// Create a backend-dispatched `AnyPool` from a `DatabaseConfig`.
///
/// Calls `sqlx::any::install_default_drivers()` once on first use so the
/// `Any` driver knows how to dispatch `sqlite://` and `postgres://` URLs
/// to the correct concrete driver.
#[allow(dead_code)] // Phase 3 (#76) calls this from AppContext::new.
pub async fn create_any_pool(
    config: &DatabaseConfig,
    fallback_sqlite_path: &Path,
) -> PdsResult<AnyPool> {
    use std::sync::Once;
    static INSTALL: Once = Once::new();
    INSTALL.call_once(sqlx::any::install_default_drivers);

    if let DatabaseBackend::Sqlite = config.backend {
        if let Some(parent) = fallback_sqlite_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }

    let mut opts = AnyPoolOptions::new()
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .acquire_timeout(Duration::from_secs(config.acquire_timeout_secs));
    if let Some(t) = config.idle_timeout_secs {
        opts = opts.idle_timeout(Some(Duration::from_secs(t)));
    }
    if let Some(t) = config.max_lifetime_secs {
        opts = opts.max_lifetime(Some(Duration::from_secs(t)));
    }

    let url = any_url_for(config, fallback_sqlite_path);
    opts.connect(&url).await.map_err(PdsError::Database)
}

/// Run backend-appropriate migrations against an `AnyPool`. Dispatches
/// to `migrations/` for SQLite and `migrations/postgres/` for Postgres.
#[allow(dead_code)] // Phase 3 (#76) calls this from AppContext::new.
pub async fn run_any_migrations(pool: &AnyPool, config: &DatabaseConfig) -> PdsResult<()> {
    match config.backend {
        DatabaseBackend::Sqlite => sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .map_err(|e| PdsError::Internal(format!("SQLite migration failed: {}", e))),
        DatabaseBackend::Postgres => sqlx::migrate!("./migrations/postgres")
            .run(pool)
            .await
            .map_err(|e| PdsError::Internal(format!("Postgres migration failed: {}", e))),
    }
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

    /// Verify `read_bool` against SQLite-via-Any with INTEGER 0/1
    /// columns — the legacy decode path that Phase 3's pattern handled.
    /// Postgres BOOLEAN is exercised by the integration test layer
    /// (tests/postgres_smoke_test.rs) once the helper is wired in.
    #[tokio::test]
    async fn test_read_bool_sqlite_integer_columns() {
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);

        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        // Schema mirrors the production pattern: INTEGER 0/1 booleans.
        sqlx::query("CREATE TABLE flags (id INTEGER PRIMARY KEY, on_flag INTEGER NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO flags (id, on_flag) VALUES (1, 1), (2, 0)")
            .execute(&pool)
            .await
            .unwrap();

        let row1 = sqlx::query("SELECT on_flag FROM flags WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(read_bool(&row1, "on_flag").unwrap(), "1 → true");

        let row2 = sqlx::query("SELECT on_flag FROM flags WHERE id = 2")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(!read_bool(&row2, "on_flag").unwrap(), "0 → false");
    }

    #[tokio::test]
    async fn test_read_bool_unknown_column_errors() {
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);

        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE x (id INTEGER PRIMARY KEY, on_flag INTEGER NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO x (id, on_flag) VALUES (1, 1)")
            .execute(&pool)
            .await
            .unwrap();
        let row = sqlx::query("SELECT on_flag FROM x WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        // Unknown column name surfaces as a sqlx::Error from try_get.
        assert!(read_bool(&row, "no_such_column").is_err());
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
