// Allow dead_code - email rate limiting for future use
#![allow(dead_code)]

//! Email Rate Limiting
//!
//! Prevents email spam by limiting the number of emails sent to each recipient.

use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Email rate limit configuration
#[derive(Debug, Clone)]
pub struct EmailRateLimitConfig {
    /// Maximum emails per recipient per hour
    pub max_per_hour: u32,
    /// Maximum emails per recipient per day
    pub max_per_day: u32,
    /// Maximum total emails per hour (global)
    pub max_global_per_hour: u32,
}

impl Default for EmailRateLimitConfig {
    fn default() -> Self {
        Self {
            max_per_hour: 5,      // 5 emails per recipient per hour
            max_per_day: 20,      // 20 emails per recipient per day
            max_global_per_hour: 1000, // 1000 total emails per hour
        }
    }
}

/// Email send record for rate limiting
#[derive(Debug, Clone)]
struct EmailSendRecord {
    timestamp: DateTime<Utc>,
}

/// Email rate limiter
pub struct EmailRateLimiter {
    config: EmailRateLimitConfig,
    /// Per-recipient send history
    recipient_history: Arc<RwLock<HashMap<String, Vec<EmailSendRecord>>>>,
    /// Global send history
    global_history: Arc<RwLock<Vec<EmailSendRecord>>>,
}

impl EmailRateLimiter {
    /// Create new rate limiter with config
    pub fn new(config: EmailRateLimitConfig) -> Self {
        Self {
            config,
            recipient_history: Arc::new(RwLock::new(HashMap::new())),
            global_history: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Check if email can be sent to recipient
    pub async fn check_rate_limit(&self, recipient: &str) -> PdsResult<()> {
        let now = Utc::now();

        // Check per-recipient limits
        {
            let history = self.recipient_history.read().await;
            if let Some(records) = history.get(recipient) {
                // Check hourly limit
                let one_hour_ago = now - Duration::hours(1);
                let hourly_count = records
                    .iter()
                    .filter(|r| r.timestamp > one_hour_ago)
                    .count() as u32;

                if hourly_count >= self.config.max_per_hour {
                    tracing::warn!(
                        "Email hourly rate limit exceeded for {}: {} emails in last hour (max: {})",
                        recipient, hourly_count, self.config.max_per_hour
                    );
                    return Err(PdsError::RateLimitExceeded {
                        retry_after: std::time::Duration::from_secs(3600), // Retry after 1 hour
                    });
                }

                // Check daily limit
                let one_day_ago = now - Duration::days(1);
                let daily_count = records
                    .iter()
                    .filter(|r| r.timestamp > one_day_ago)
                    .count() as u32;

                if daily_count >= self.config.max_per_day {
                    tracing::warn!(
                        "Email daily rate limit exceeded for {}: {} emails in last day (max: {})",
                        recipient, daily_count, self.config.max_per_day
                    );
                    return Err(PdsError::RateLimitExceeded {
                        retry_after: std::time::Duration::from_secs(86400), // Retry after 24 hours
                    });
                }
            }
        }

        // Check global limit
        {
            let global = self.global_history.read().await;
            let one_hour_ago = now - Duration::hours(1);
            let global_count = global
                .iter()
                .filter(|r| r.timestamp > one_hour_ago)
                .count() as u32;

            if global_count >= self.config.max_global_per_hour {
                tracing::warn!(
                    "Global email rate limit exceeded: {} emails in last hour (max: {})",
                    global_count, self.config.max_global_per_hour
                );
                return Err(PdsError::RateLimitExceeded {
                    retry_after: std::time::Duration::from_secs(3600), // Retry after 1 hour
                });
            }
        }

        Ok(())
    }

    /// Record email send
    pub async fn record_send(&self, recipient: &str) {
        let now = Utc::now();
        let record = EmailSendRecord { timestamp: now };

        // Record for recipient
        {
            let mut history = self.recipient_history.write().await;
            history
                .entry(recipient.to_string())
                .or_insert_with(Vec::new)
                .push(record.clone());
        }

        // Record globally
        {
            let mut global = self.global_history.write().await;
            global.push(record);
        }

        // Cleanup old records in background
        self.cleanup_old_records().await;
    }

    /// Cleanup old records to prevent memory growth
    async fn cleanup_old_records(&self) {
        let now = Utc::now();
        let cutoff = now - Duration::days(1);

        // Cleanup recipient history
        {
            let mut history = self.recipient_history.write().await;
            for records in history.values_mut() {
                records.retain(|r| r.timestamp > cutoff);
            }
            // Remove empty entries
            history.retain(|_, v| !v.is_empty());
        }

        // Cleanup global history
        {
            let mut global = self.global_history.write().await;
            global.retain(|r| r.timestamp > cutoff);
        }
    }

    /// Get send count for recipient in last hour
    pub async fn get_hourly_count(&self, recipient: &str) -> u32 {
        let now = Utc::now();
        let one_hour_ago = now - Duration::hours(1);

        let history = self.recipient_history.read().await;
        history
            .get(recipient)
            .map(|records| {
                records
                    .iter()
                    .filter(|r| r.timestamp > one_hour_ago)
                    .count() as u32
            })
            .unwrap_or(0)
    }

    /// Get send count for recipient in last day
    pub async fn get_daily_count(&self, recipient: &str) -> u32 {
        let now = Utc::now();
        let one_day_ago = now - Duration::days(1);

        let history = self.recipient_history.read().await;
        history
            .get(recipient)
            .map(|records| {
                records
                    .iter()
                    .filter(|r| r.timestamp > one_day_ago)
                    .count() as u32
            })
            .unwrap_or(0)
    }

    /// Get global send count in last hour
    pub async fn get_global_hourly_count(&self) -> u32 {
        let now = Utc::now();
        let one_hour_ago = now - Duration::hours(1);

        let global = self.global_history.read().await;
        global
            .iter()
            .filter(|r| r.timestamp > one_hour_ago)
            .count() as u32
    }
}

impl Default for EmailRateLimiter {
    fn default() -> Self {
        Self::new(EmailRateLimitConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limit_per_recipient() {
        let config = EmailRateLimitConfig {
            max_per_hour: 2,
            max_per_day: 5,
            max_global_per_hour: 1000,
        };

        let limiter = EmailRateLimiter::new(config);

        // First two emails should succeed
        assert!(limiter.check_rate_limit("alice@example.com").await.is_ok());
        limiter.record_send("alice@example.com").await;

        assert!(limiter.check_rate_limit("alice@example.com").await.is_ok());
        limiter.record_send("alice@example.com").await;

        // Third email should fail (hourly limit)
        assert!(limiter.check_rate_limit("alice@example.com").await.is_err());

        // Different recipient should still work
        assert!(limiter.check_rate_limit("bob@example.com").await.is_ok());
    }

    #[tokio::test]
    async fn test_rate_limit_counts() {
        let limiter = EmailRateLimiter::default();

        limiter.record_send("alice@example.com").await;
        limiter.record_send("alice@example.com").await;

        assert_eq!(limiter.get_hourly_count("alice@example.com").await, 2);
        assert_eq!(limiter.get_daily_count("alice@example.com").await, 2);
        assert_eq!(limiter.get_global_hourly_count().await, 2);
    }

    #[tokio::test]
    async fn test_global_rate_limit() {
        let config = EmailRateLimitConfig {
            max_per_hour: 1000,
            max_per_day: 5000,
            max_global_per_hour: 3,
        };

        let limiter = EmailRateLimiter::new(config);

        // Send to different recipients
        limiter.record_send("alice@example.com").await;
        limiter.record_send("bob@example.com").await;
        limiter.record_send("charlie@example.com").await;

        // Fourth email should fail (global limit)
        assert!(limiter.check_rate_limit("dave@example.com").await.is_err());
    }
}
