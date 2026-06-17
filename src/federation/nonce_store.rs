// Allow dead_code - nonce store features for future use
#![allow(dead_code)]

//! Nonce Store for Replay Attack Prevention
//!
//! Tracks JWT nonces (jti claims) to prevent replay attacks on service auth tokens.
//! Since service auth JWTs have <60 second lifetime, we only need to track nonces
//! for a short duration.
//!
//! Time comes from an injected [`Clock`] (#269, the #266 follow-up): production
//! wires [`SystemClock`]; tests wire `MockClock` and `advance()` past the
//! retention window instead of sleeping against the real wall clock. The store
//! moved from monotonic `Instant` to wall-clock `DateTime<Utc>` to adopt the
//! shared `identity::clock::Clock` primitive — consistent with `DidCache`'s
//! TTLs, and acceptable here because the JWT `exp` these nonces shadow is itself
//! wall-clock-validated over the same ~120s horizon.

use crate::error::PdsResult;
use crate::identity::clock::{Clock, SystemClock};
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Nonce entry with expiration
#[derive(Debug, Clone)]
struct NonceEntry {
    #[allow(dead_code)] // Kept for debugging, only expires_at is checked
    recorded_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

/// In-memory nonce store for tracking used JWTs
///
/// This prevents replay attacks by tracking JWT IDs (jti claims).
/// Nonces are automatically expired after 120 seconds (2x the max JWT lifetime).
pub struct NonceStore {
    /// Map of nonce (jti) to entry
    nonces: Arc<RwLock<HashMap<String, NonceEntry>>>,

    /// How long to keep nonces (default: 120 seconds)
    retention: Duration,

    /// Time source. `SystemClock` in production; tests inject `MockClock`.
    clock: Arc<dyn Clock>,
}

impl NonceStore {
    /// Create a new nonce store
    pub fn new() -> Self {
        Self {
            nonces: Arc::new(RwLock::new(HashMap::new())),
            retention: Duration::seconds(120), // 2x max JWT lifetime
            clock: Arc::new(SystemClock),
        }
    }

    /// Create a nonce store with custom retention duration
    pub fn with_retention(retention_seconds: u64) -> Self {
        Self {
            nonces: Arc::new(RwLock::new(HashMap::new())),
            retention: Duration::seconds(retention_seconds as i64),
            clock: Arc::new(SystemClock),
        }
    }

    /// Override the time source. Production never calls this (it keeps the
    /// `SystemClock` default); tests pass a `MockClock` to drive expiry
    /// deterministically. Mirrors `DidCache::with_clock`.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Check if a nonce has been used and record it
    ///
    /// Returns `Ok(true)` if the nonce is new (not seen before)
    /// Returns `Ok(false)` if the nonce has been used (replay attack)
    /// Returns `Err` if there's an internal error
    pub async fn check_and_record(&self, nonce: &str) -> PdsResult<bool> {
        let mut nonces = self.nonces.write().await;
        let now = self.clock.now();

        // Check if nonce exists and is not expired
        if let Some(entry) = nonces.get(nonce) {
            if now < entry.expires_at {
                warn!("Replay attack detected: nonce {} already used", nonce);
                return Ok(false); // Nonce already used - replay attack!
            } else {
                // Nonce expired, can be reused (though this shouldn't happen with unique UUIDs)
                debug!("Expired nonce {} being reused", nonce);
            }
        }

        // Record the nonce
        let entry = NonceEntry {
            recorded_at: now,
            expires_at: now + self.retention,
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

        let now = self.clock.now();
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
    use crate::identity::clock::MockClock;

    // Fixed anchor so test "time" is fully deterministic; advance the
    // MockClock instead of sleeping against the real wall clock (#269).
    fn anchor() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2020-06-15T12:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc)
    }

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
        let clock = Arc::new(MockClock::new(anchor()));
        let store = NonceStore::with_retention(1).with_clock(clock.clone()); // 1s retention

        // Record a nonce
        let is_new = store.check_and_record("expiring-nonce").await.unwrap();
        assert!(is_new);

        // Advance past the retention window (was a real 2s sleep).
        clock.advance(Duration::seconds(2));

        // Cleanup expired nonces
        let removed = store.cleanup_expired().await.unwrap();
        assert_eq!(removed, 1);

        // Nonce can be reused after expiration (though shouldn't happen with unique UUIDs)
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
        // 4s retention. nonce-1 recorded at t0, nonce-2 at t0+3s, cleanup at
        // t0+5s: nonce-1 age 5s (expired), nonce-2 age 2s (kept). With the
        // MockClock these ages are exact — the old real-sleep version could
        // dilate its 2s wait past the 4s boundary under load and wrongly
        // remove nonce-2 too (the #266 follow-up this closes).
        let clock = Arc::new(MockClock::new(anchor()));
        let store = NonceStore::with_retention(4).with_clock(clock.clone());

        store.check_and_record("nonce-1").await.unwrap();
        clock.advance(Duration::seconds(3));
        store.check_and_record("nonce-2").await.unwrap();

        clock.advance(Duration::seconds(2));

        let removed = store.cleanup_expired().await.unwrap();
        assert_eq!(removed, 1);
        assert_eq!(store.count().await, 1);
    }
}
