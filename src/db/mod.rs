/// Database layer for Aurora Locus
///
/// Manages database connections, migrations, and provides typed access
/// to the account, sequencer, and DID cache databases.

pub mod account;

use crate::error::{PdsError, PdsResult};
use sqlx::sqlite::SqlitePool;
use std::path::Path;

/// Database connection options
#[derive(Debug, Clone)]
pub struct DatabaseOptions {
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

    // Build connection string with pragmas
    let connection_string = format!(
        "sqlite://{}?mode=rwc",
        path.to_string_lossy()
    );

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
    .map_err(|e| PdsError::Database(e))?;

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
        .map_err(|e| PdsError::Database(e))?;

    Ok(())
}
