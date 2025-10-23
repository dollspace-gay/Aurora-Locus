/// PostgreSQL database support for Aurora Locus PDS
///
/// This module provides PostgreSQL database backend as an alternative to SQLite
/// for production deployments requiring better scalability and concurrency.

use crate::error::{PdsError, PdsResult};
use sqlx::postgres::{PgPool, PgPoolOptions};
use tracing::{info, error};

/// PostgreSQL database configuration
#[derive(Debug, Clone)]
pub struct PostgresConfig {
    /// PostgreSQL connection URL
    /// Example: "postgresql://user:password@localhost:5432/aurora_locus"
    pub database_url: String,

    /// Maximum number of connections in the pool
    pub max_connections: u32,

    /// Minimum number of connections in the pool
    pub min_connections: u32,

    /// Connection timeout in seconds
    pub connect_timeout: u64,

    /// Maximum lifetime of a connection in seconds
    pub max_lifetime: u64,

    /// Idle timeout for connections in seconds
    pub idle_timeout: u64,
}

impl Default for PostgresConfig {
    fn default() -> Self {
        Self {
            database_url: "postgresql://localhost:5432/aurora_locus".to_string(),
            max_connections: 100,
            min_connections: 10,
            connect_timeout: 30,
            max_lifetime: 1800, // 30 minutes
            idle_timeout: 600,   // 10 minutes
        }
    }
}

impl PostgresConfig {
    /// Load from environment variables
    pub fn from_env() -> PdsResult<Self> {
        let database_url = std::env::var("DATABASE_URL")
            .or_else(|_| std::env::var("POSTGRES_URL"))
            .map_err(|_| {
                PdsError::Internal("DATABASE_URL or POSTGRES_URL must be set for PostgreSQL".to_string())
            })?;

        Ok(Self {
            database_url,
            max_connections: std::env::var("POSTGRES_MAX_CONNECTIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100),
            min_connections: std::env::var("POSTGRES_MIN_CONNECTIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10),
            connect_timeout: std::env::var("POSTGRES_CONNECT_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            max_lifetime: std::env::var("POSTGRES_MAX_LIFETIME")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1800),
            idle_timeout: std::env::var("POSTGRES_IDLE_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(600),
        })
    }
}

/// Create a PostgreSQL connection pool
pub async fn create_pool(config: PostgresConfig) -> PdsResult<PgPool> {
    info!("Connecting to PostgreSQL database...");
    info!("  Max connections: {}", config.max_connections);
    info!("  Min connections: {}", config.min_connections);

    let pool = PgPoolOptions::new()
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .acquire_timeout(std::time::Duration::from_secs(config.connect_timeout))
        .max_lifetime(std::time::Duration::from_secs(config.max_lifetime))
        .idle_timeout(std::time::Duration::from_secs(config.idle_timeout))
        .connect(&config.database_url)
        .await
        .map_err(|e| {
            error!("Failed to connect to PostgreSQL: {}", e);
            PdsError::Database(e)
        })?;

    info!("✓ PostgreSQL connection established");

    Ok(pool)
}

/// Run PostgreSQL migrations
pub async fn run_migrations(pool: &PgPool) -> PdsResult<()> {
    info!("Running PostgreSQL migrations...");

    sqlx::migrate!("./migrations/postgres")
        .run(pool)
        .await
        .map_err(|e| {
            error!("Failed to run migrations: {}", e);
            PdsError::Internal(format!("Migration failed: {}", e))
        })?;

    info!("✓ Migrations completed");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_postgres_config_default() {
        let config = PostgresConfig::default();
        assert_eq!(config.max_connections, 100);
        assert_eq!(config.min_connections, 10);
        assert_eq!(config.connect_timeout, 30);
    }

    #[test]
    fn test_postgres_config_from_env_missing_url() {
        // Clear environment
        std::env::remove_var("DATABASE_URL");
        std::env::remove_var("POSTGRES_URL");

        let result = PostgresConfig::from_env();
        assert!(result.is_err());
    }
}
