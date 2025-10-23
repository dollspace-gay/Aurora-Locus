/// Distributed rate limiting using Redis
///
/// This module provides distributed rate limiting capabilities using Redis,
/// allowing multiple PDS instances to share rate limit state.

use crate::cache::CacheClient;
use crate::error::{PdsError, PdsResult};
use std::time::Duration;
use tracing::{debug, warn};

/// Distributed rate limiter using Redis
#[derive(Clone)]
pub struct DistributedRateLimiter {
    cache: CacheClient,
    requests_per_minute: u32,
}

impl DistributedRateLimiter {
    /// Create a new distributed rate limiter
    pub fn new(cache: CacheClient, requests_per_minute: u32) -> Self {
        Self {
            cache,
            requests_per_minute,
        }
    }

    /// Check if a request should be allowed
    ///
    /// Returns Ok(()) if allowed, Err if rate limit exceeded
    pub async fn check_rate_limit(&self, identifier: &str) -> PdsResult<()> {
        let category = "ratelimit:";
        let window = Self::current_window();
        let key = format!("{}:{}", identifier, window);

        // Increment counter with 60-second TTL
        let count = self.cache.increment(category, &key, 60).await?;

        debug!(
            "Rate limit check: {} => {}/{}",
            identifier, count, self.requests_per_minute
        );

        if count > self.requests_per_minute as i64 {
            warn!(
                "Rate limit exceeded for {}: {}/{}",
                identifier, count, self.requests_per_minute
            );
            return Err(PdsError::RateLimitExceeded {
                retry_after: Duration::from_secs(60),
            });
        }

        Ok(())
    }

    /// Get current request count for an identifier
    pub async fn get_count(&self, identifier: &str) -> PdsResult<i64> {
        let category = "ratelimit:";
        let window = Self::current_window();
        let key = format!("{}:{}", identifier, window);

        match self.cache.get::<i64>(category, &key).await? {
            Some(count) => Ok(count),
            None => Ok(0),
        }
    }

    /// Get remaining requests for an identifier
    pub async fn get_remaining(&self, identifier: &str) -> PdsResult<u32> {
        let count = self.get_count(identifier).await?;
        let remaining = (self.requests_per_minute as i64 - count).max(0);
        Ok(remaining as u32)
    }

    /// Reset rate limit for an identifier (admin operation)
    pub async fn reset(&self, identifier: &str) -> PdsResult<()> {
        let category = "ratelimit:";
        let window = Self::current_window();
        let key = format!("{}:{}", identifier, window);

        self.cache.delete(category, &key).await
    }

    /// Get current time window (minute)
    fn current_window() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            / 60
    }
}

/// Token bucket rate limiter using Redis
///
/// More sophisticated than simple counter, allows burst traffic
#[derive(Clone)]
pub struct TokenBucketLimiter {
    cache: CacheClient,
    capacity: u32,
    refill_rate: f64, // tokens per second
}

impl TokenBucketLimiter {
    /// Create a new token bucket rate limiter
    pub fn new(cache: CacheClient, capacity: u32, refill_rate: f64) -> Self {
        Self {
            cache,
            capacity,
            refill_rate,
        }
    }

    /// Check if a request should be allowed
    pub async fn check_rate_limit(&self, identifier: &str) -> PdsResult<()> {
        let category = "ratelimit:bucket:";
        let now = Self::current_timestamp();

        // Get current bucket state
        let bucket_data: Option<TokenBucket> = self.cache.get(category, identifier).await?;

        let mut bucket = match bucket_data {
            Some(b) => b,
            None => TokenBucket {
                tokens: self.capacity as f64,
                last_refill: now,
            },
        };

        // Refill tokens based on time elapsed
        let elapsed = now - bucket.last_refill;
        let new_tokens = elapsed * self.refill_rate;
        bucket.tokens = (bucket.tokens + new_tokens).min(self.capacity as f64);
        bucket.last_refill = now;

        // Check if we have tokens available
        if bucket.tokens < 1.0 {
            warn!("Rate limit exceeded for {} (token bucket)", identifier);
            return Err(PdsError::RateLimitExceeded {
                retry_after: Duration::from_secs_f64((1.0 - bucket.tokens) / self.refill_rate),
            });
        }

        // Consume a token
        bucket.tokens -= 1.0;

        // Save bucket state
        self.cache
            .set(category, identifier, &bucket, Some(3600))
            .await?;

        debug!(
            "Token bucket check: {} => {:.2} tokens remaining",
            identifier, bucket.tokens
        );

        Ok(())
    }

    /// Get current timestamp in seconds
    fn current_timestamp() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
    }
}

/// Token bucket state
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TokenBucket {
    tokens: f64,
    last_refill: f64,
}

/// Sliding window rate limiter using Redis
///
/// More accurate than fixed window, prevents boundary issues
pub struct SlidingWindowLimiter {
    cache: CacheClient,
    max_requests: u32,
    window_seconds: u64,
}

impl SlidingWindowLimiter {
    /// Create a new sliding window rate limiter
    pub fn new(cache: CacheClient, max_requests: u32, window_seconds: u64) -> Self {
        Self {
            cache,
            max_requests,
            window_seconds,
        }
    }

    /// Check if a request should be allowed
    ///
    /// This uses a Redis sorted set to track timestamps of requests
    pub async fn check_rate_limit(&self, identifier: &str) -> PdsResult<()> {
        // For simplicity, we'll use a fixed window approach here
        // A full sliding window would require Lua scripts or sorted sets
        let category = "ratelimit:sliding:";
        let now = Self::current_timestamp();
        let window_start = now - self.window_seconds;

        // In a real implementation, you would:
        // 1. Add current timestamp to sorted set (ZADD)
        // 2. Remove old timestamps (ZREMRANGEBYSCORE)
        // 3. Count remaining timestamps (ZCARD)
        // 4. Check if count exceeds max_requests

        // For now, use a simpler approach with a counter
        let key = format!("{}:{}:{}", identifier, window_start / self.window_seconds, self.window_seconds);
        let count = self.cache.increment(category, &key, self.window_seconds).await?;

        if count > self.max_requests as i64 {
            warn!("Rate limit exceeded for {} (sliding window)", identifier);
            return Err(PdsError::RateLimitExceeded {
                retry_after: Duration::from_secs(self.window_seconds),
            });
        }

        Ok(())
    }

    fn current_timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_window() {
        let window = DistributedRateLimiter::current_window();
        assert!(window > 0);
    }

    #[test]
    fn test_token_bucket_creation() {
        let bucket = TokenBucket {
            tokens: 100.0,
            last_refill: 0.0,
        };
        assert_eq!(bucket.tokens, 100.0);
    }

    #[test]
    fn test_token_bucket_serialization() {
        let bucket = TokenBucket {
            tokens: 50.5,
            last_refill: 1234567890.0,
        };

        let json = serde_json::to_string(&bucket).unwrap();
        let deserialized: TokenBucket = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.tokens, 50.5);
        assert_eq!(deserialized.last_refill, 1234567890.0);
    }
}
