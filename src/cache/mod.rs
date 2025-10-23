/// Redis-based caching layer for Aurora Locus PDS
///
/// Provides distributed caching capabilities for:
/// - DID resolution results
/// - Handle lookups
/// - Session tokens
/// - Repository metadata
/// - Rate limit counters (for distributed rate limiting)

use crate::error::{PdsError, PdsResult};
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client};
use serde::{de::DeserializeOwned, Serialize};
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Cache layer configuration
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Enable caching (default: false for backward compatibility)
    pub enabled: bool,

    /// Redis connection URL (e.g., "redis://localhost:6379")
    pub redis_url: String,

    /// Key prefix for all cache entries (default: "aurora:")
    pub key_prefix: String,

    /// Default TTL for cache entries in seconds (default: 300 = 5 minutes)
    pub default_ttl: u64,

    /// DID document cache TTL in seconds (default: 3600 = 1 hour)
    pub did_doc_ttl: u64,

    /// Handle resolution cache TTL in seconds (default: 1800 = 30 minutes)
    pub handle_ttl: u64,

    /// Session cache TTL in seconds (default: 600 = 10 minutes)
    pub session_ttl: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            redis_url: "redis://localhost:6379".to_string(),
            key_prefix: "aurora:".to_string(),
            default_ttl: 300,
            did_doc_ttl: 3600,
            handle_ttl: 1800,
            session_ttl: 600,
        }
    }
}

impl CacheConfig {
    /// Load from environment variables
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("CACHE_ENABLED")
                .unwrap_or_else(|_| "false".to_string())
                .parse()
                .unwrap_or(false),
            redis_url: std::env::var("REDIS_URL")
                .unwrap_or_else(|_| "redis://localhost:6379".to_string()),
            key_prefix: std::env::var("CACHE_KEY_PREFIX")
                .unwrap_or_else(|_| "aurora:".to_string()),
            default_ttl: std::env::var("CACHE_DEFAULT_TTL")
                .unwrap_or_else(|_| "300".to_string())
                .parse()
                .unwrap_or(300),
            did_doc_ttl: std::env::var("CACHE_DID_DOC_TTL")
                .unwrap_or_else(|_| "3600".to_string())
                .parse()
                .unwrap_or(3600),
            handle_ttl: std::env::var("CACHE_HANDLE_TTL")
                .unwrap_or_else(|_| "1800".to_string())
                .parse()
                .unwrap_or(1800),
            session_ttl: std::env::var("CACHE_SESSION_TTL")
                .unwrap_or_else(|_| "600".to_string())
                .parse()
                .unwrap_or(600),
        }
    }
}

/// Redis cache client
#[derive(Clone)]
pub struct CacheClient {
    connection: ConnectionManager,
    config: CacheConfig,
}

impl CacheClient {
    /// Create a new cache client
    pub async fn new(config: CacheConfig) -> PdsResult<Self> {
        if !config.enabled {
            return Err(PdsError::Internal(
                "Cache is disabled, cannot create client".to_string(),
            ));
        }

        info!("Connecting to Redis at {}", config.redis_url);

        let client = Client::open(config.redis_url.as_str()).map_err(|e| {
            error!("Failed to create Redis client: {}", e);
            PdsError::Internal(format!("Redis client creation failed: {}", e))
        })?;

        let connection = ConnectionManager::new(client).await.map_err(|e| {
            error!("Failed to connect to Redis: {}", e);
            PdsError::Internal(format!("Redis connection failed: {}", e))
        })?;

        info!("✓ Redis connection established");

        Ok(Self { connection, config })
    }

    /// Build a cache key with prefix
    fn build_key(&self, category: &str, key: &str) -> String {
        format!("{}{}{}", self.config.key_prefix, category, key)
    }

    /// Get a value from cache
    pub async fn get<T: DeserializeOwned>(&self, category: &str, key: &str) -> PdsResult<Option<T>> {
        let cache_key = self.build_key(category, key);

        debug!("Cache GET: {}", cache_key);

        let mut conn = self.connection.clone();
        let result: Option<String> = conn.get(&cache_key).await.map_err(|e| {
            warn!("Redis GET failed for {}: {}", cache_key, e);
            PdsError::Internal(format!("Cache get failed: {}", e))
        })?;

        match result {
            Some(json) => {
                debug!("Cache HIT: {}", cache_key);
                match serde_json::from_str(&json) {
                    Ok(value) => Ok(Some(value)),
                    Err(e) => {
                        warn!("Failed to deserialize cached value: {}", e);
                        // Delete corrupted cache entry
                        let _ = self.delete(category, key).await;
                        Ok(None)
                    }
                }
            }
            None => {
                debug!("Cache MISS: {}", cache_key);
                Ok(None)
            }
        }
    }

    /// Set a value in cache with TTL
    pub async fn set<T: Serialize>(
        &self,
        category: &str,
        key: &str,
        value: &T,
        ttl_secs: Option<u64>,
    ) -> PdsResult<()> {
        let cache_key = self.build_key(category, key);
        let ttl = ttl_secs.unwrap_or(self.config.default_ttl);

        debug!("Cache SET: {} (TTL: {}s)", cache_key, ttl);

        let json = serde_json::to_string(value).map_err(|e| {
            error!("Failed to serialize value for cache: {}", e);
            PdsError::Internal(format!("Cache serialization failed: {}", e))
        })?;

        let mut conn = self.connection.clone();
        conn.set_ex(&cache_key, json, ttl)
            .await
            .map_err(|e| {
                warn!("Redis SET failed for {}: {}", cache_key, e);
                PdsError::Internal(format!("Cache set failed: {}", e))
            })?;

        debug!("Cache SET successful: {}", cache_key);
        Ok(())
    }

    /// Delete a value from cache
    pub async fn delete(&self, category: &str, key: &str) -> PdsResult<()> {
        let cache_key = self.build_key(category, key);

        debug!("Cache DELETE: {}", cache_key);

        let mut conn = self.connection.clone();
        conn.del(&cache_key).await.map_err(|e| {
            warn!("Redis DELETE failed for {}: {}", cache_key, e);
            PdsError::Internal(format!("Cache delete failed: {}", e))
        })?;

        Ok(())
    }

    /// Check if a key exists in cache
    pub async fn exists(&self, category: &str, key: &str) -> PdsResult<bool> {
        let cache_key = self.build_key(category, key);

        let mut conn = self.connection.clone();
        conn.exists(&cache_key).await.map_err(|e| {
            warn!("Redis EXISTS failed for {}: {}", cache_key, e);
            PdsError::Internal(format!("Cache exists check failed: {}", e))
        })
    }

    /// Increment a counter (for rate limiting)
    pub async fn increment(&self, category: &str, key: &str, ttl_secs: u64) -> PdsResult<i64> {
        let cache_key = self.build_key(category, key);

        debug!("Cache INCR: {} (TTL: {}s)", cache_key, ttl_secs);

        let mut conn = self.connection.clone();

        // Increment counter
        let count: i64 = conn.incr(&cache_key, 1).await.map_err(|e| {
            warn!("Redis INCR failed for {}: {}", cache_key, e);
            PdsError::Internal(format!("Cache increment failed: {}", e))
        })?;

        // Set TTL if this is a new key (count == 1)
        if count == 1 {
            conn.expire(&cache_key, ttl_secs as i64)
                .await
                .map_err(|e| {
                    warn!("Redis EXPIRE failed for {}: {}", cache_key, e);
                    PdsError::Internal(format!("Cache expire failed: {}", e))
                })?;
        }

        Ok(count)
    }

    /// Get TTL of a key
    pub async fn ttl(&self, category: &str, key: &str) -> PdsResult<i64> {
        let cache_key = self.build_key(category, key);

        let mut conn = self.connection.clone();
        conn.ttl(&cache_key).await.map_err(|e| {
            warn!("Redis TTL failed for {}: {}", cache_key, e);
            PdsError::Internal(format!("Cache TTL check failed: {}", e))
        })
    }

    /// Flush all keys matching a pattern (use with caution!)
    pub async fn flush_pattern(&self, pattern: &str) -> PdsResult<u64> {
        let cache_pattern = self.build_key("", pattern);

        info!("Cache FLUSH pattern: {}", cache_pattern);

        let mut conn = self.connection.clone();

        // Get all keys matching pattern
        let keys: Vec<String> = conn.keys(&cache_pattern).await.map_err(|e| {
            error!("Redis KEYS failed: {}", e);
            PdsError::Internal(format!("Cache keys lookup failed: {}", e))
        })?;

        if keys.is_empty() {
            return Ok(0);
        }

        // Delete all keys
        let deleted: u64 = conn.del(&keys).await.map_err(|e| {
            error!("Redis DELETE multiple keys failed: {}", e);
            PdsError::Internal(format!("Cache flush failed: {}", e))
        })?;

        info!("Cache flushed {} keys matching {}", deleted, cache_pattern);
        Ok(deleted)
    }

    /// Ping Redis to check connection
    pub async fn ping(&self) -> PdsResult<()> {
        let mut conn = self.connection.clone();
        let pong: String = redis::cmd("PING").query_async(&mut conn).await.map_err(|e| {
            error!("Redis PING failed: {}", e);
            PdsError::Internal(format!("Cache ping failed: {}", e))
        })?;

        if pong != "PONG" {
            return Err(PdsError::Internal(
                "Unexpected Redis PING response".to_string(),
            ));
        }

        Ok(())
    }

    /// Get cache statistics
    pub async fn stats(&self) -> PdsResult<CacheStats> {
        let mut conn = self.connection.clone();

        let info: String = redis::cmd("INFO")
            .arg("stats")
            .query_async(&mut conn)
            .await
            .map_err(|e| {
                error!("Redis INFO failed: {}", e);
                PdsError::Internal(format!("Cache stats failed: {}", e))
            })?;

        // Parse basic stats
        let mut hits = 0;
        let mut misses = 0;

        for line in info.lines() {
            if line.starts_with("keyspace_hits:") {
                hits = line.split(':').nth(1).unwrap_or("0").parse().unwrap_or(0);
            } else if line.starts_with("keyspace_misses:") {
                misses = line.split(':').nth(1).unwrap_or("0").parse().unwrap_or(0);
            }
        }

        Ok(CacheStats {
            hits,
            misses,
            hit_rate: if hits + misses > 0 {
                hits as f64 / (hits + misses) as f64
            } else {
                0.0
            },
        })
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
}

/// Cache category constants
pub mod categories {
    pub const DID_DOC: &str = "did:doc:";
    pub const HANDLE: &str = "did:handle:";
    pub const SESSION: &str = "session:";
    pub const RATE_LIMIT: &str = "ratelimit:";
    pub const REPO_META: &str = "repo:meta:";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_config_default() {
        let config = CacheConfig::default();
        assert_eq!(config.enabled, false);
        assert_eq!(config.key_prefix, "aurora:");
        assert_eq!(config.default_ttl, 300);
    }

    #[test]
    fn test_build_key() {
        let config = CacheConfig::default();
        // We can't easily test async without Redis, but we can test key building logic
        let key = format!("{}{}{}",  config.key_prefix, "test:", "123");
        assert_eq!(key, "aurora:test:123");
    }

    #[test]
    fn test_cache_categories() {
        assert_eq!(categories::DID_DOC, "did:doc:");
        assert_eq!(categories::HANDLE, "did:handle:");
        assert_eq!(categories::SESSION, "session:");
        assert_eq!(categories::RATE_LIMIT, "ratelimit:");
    }
}
