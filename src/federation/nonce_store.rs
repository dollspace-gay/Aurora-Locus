//! Nonce Store for Replay Attack Prevention
//!
//! Tracks JWT nonces (jti claims) to prevent replay attacks on service auth tokens.
//! Since service auth JWTs have <60 second lifetime, we only need to track nonces
//! for a short duration.

use crate::error::PdsResult;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Nonce entry with expiration
#[derive(Debug, Clone)]
struct NonceEntry {
    #[allow(dead_code)] // Kept for debugging, only expires_at is checked
    recorded_at: Instant,
    expires_at: Instant,
}

/// In-memory nonce store for tracking used JWTs
///
/// This prevents replay attacks by tracking JWT IDs (jti claims).
/// Nonces are automatically expired after 120 seconds (2x the max JWT lifetime).
pub struct NonceStore {
    /// Map of nonce (jti) to entry
    nonces: Arc<RwLock<HashMap<String, NonceEntry>>>,

    /// How long to keep nonces (default: 120 seconds)
    retention_duration: Duration,
}

impl NonceStore {
    /// Create a new nonce store
    pub fn new() -> Self {
        Self {
            nonces: Arc::new(RwLock::new(HashMap::new())),
            retention_duration: Duration::from_secs(120), // 2x max JWT lifetime
        }
    }

    /// Create a nonce store with custom retention duration
    pub fn with_retention(retention_seconds: u64) -> Self {
        Self {
            nonces: Arc::new(RwLock::new(HashMap::new())),
            retention_duration: Duration::from_secs(retention_seconds),
        }
    }

    /// Check if a nonce has been used and record it
    ///
    /// Returns `Ok(true)` if the nonce is new (not seen before)
    /// Returns `Ok(false)` if the nonce has been used (replay attack)
    /// Returns `Err` if there's an internal error
    pub async fn check_and_record(&self, nonce: &str) -> PdsResult<bool> {
        let mut nonces = self.nonces.write().await;

        // Check if nonce exists and is not expired
        if let Some(entry) = nonces.get(nonce) {
            if Instant::now() < entry.expires_at {
                warn!("Replay attack detected: nonce {} already used", nonce);
                return Ok(false); // Nonce already used - replay attack!
            } else {
                // Nonce expired, can be reused (though this shouldn't happen with unique UUIDs)
                debug!("Expired nonce {} being reused", nonce);
            }
        }

        // Record the nonce
        let now = Instant::now();
        let entry = NonceEntry {
            recorded_at: now,
            expires_at: now + self.retention_duration,
        };

        nonces.insert(nonce.to_string(), entry);

        debug!("Recorded new nonce: {}", nonce);

        Ok(true) // Nonce is new
    }

    /// Clean up expired nonces (should be called periodically)
    ///
    /// Returns the number of nonces that were removed
    pub async fn cleanup_expired(&self) -> PdsResult<usize> {
        let mut nonces = self.nonces.write().await;

        let now = Instant::now();
        let initial_count = nonces.len();

        // Remove expired entries
        nonces.retain(|_nonce, entry| entry.expires_at > now);

        let removed_count = initial_count - nonces.len();

        if removed_count > 0 {
            debug!("Cleaned up {} expired nonces", removed_count);
        }

        Ok(removed_count)
    }

    /// Get the number of nonces currently tracked
    pub async fn count(&self) -> usize {
        self.nonces.read().await.len()
    }

    /// Clear all nonces (for testing)
    #[cfg(test)]
    pub async fn clear(&self) {
        self.nonces.write().await.clear();
    }
}

impl Default for NonceStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_nonce_check_and_record() {
        let store = NonceStore::new();

        // First use - should be new
        let is_new = store.check_and_record("test-nonce-1").await.unwrap();
        assert!(is_new);

        // Second use - should be replay attack
        let is_new_again = store.check_and_record("test-nonce-1").await.unwrap();
        assert!(!is_new_again);

        // Different nonce - should be new
        let is_new_2 = store.check_and_record("test-nonce-2").await.unwrap();
        assert!(is_new_2);
    }

    #[tokio::test]
    async fn test_nonce_expiration() {
        let store = NonceStore::with_retention(1); // 1 second retention

        // Record a nonce
        let is_new = store.check_and_record("expiring-nonce").await.unwrap();
        assert!(is_new);

        // Wait for expiration
        sleep(Duration::from_secs(2)).await;

        // Cleanup expired nonces
        let removed = store.cleanup_expired().await.unwrap();
        assert_eq!(removed, 1);

        // Nonce can be reused after expiration (though shouldn't happen with UUIDs)
        let is_new_again = store.check_and_record("expiring-nonce").await.unwrap();
        assert!(is_new_again);
    }

    #[tokio::test]
    async fn test_nonce_count() {
        let store = NonceStore::new();

        assert_eq!(store.count().await, 0);

        store.check_and_record("nonce-1").await.unwrap();
        assert_eq!(store.count().await, 1);

        store.check_and_record("nonce-2").await.unwrap();
        assert_eq!(store.count().await, 2);

        // Duplicate doesn't increase count
        store.check_and_record("nonce-1").await.unwrap();
        assert_eq!(store.count().await, 2);
    }

    #[tokio::test]
    async fn test_cleanup_removes_only_expired() {
        let store = NonceStore::with_retention(2); // 2 second retention

        // Add nonces at different times
        store.check_and_record("nonce-1").await.unwrap();

        sleep(Duration::from_millis(500)).await;
        store.check_and_record("nonce-2").await.unwrap();

        sleep(Duration::from_secs(2)).await; // nonce-1 should be expired, nonce-2 still valid

        let removed = store.cleanup_expired().await.unwrap();
        assert_eq!(removed, 1);

        // nonce-1 should be removable (expired)
        // nonce-2 should still be there
        assert_eq!(store.count().await, 1);
    }
}
