/// Rate Limiting System
use crate::error::{PdsError, PdsResult};
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use governor::{
    clock::DefaultClock,
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter as GovernorLimiter,
};
use std::{num::NonZeroU32, sync::Arc};

/// Rate limiter configuration
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Requests per second for authenticated users
    pub authenticated_rps: u32,
    /// Requests per second for unauthenticated users
    pub unauthenticated_rps: u32,
    /// Requests per second for admin users
    pub admin_rps: u32,
    /// Burst size
    pub burst_size: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            authenticated_rps: 100,      // 100 req/sec for authenticated
            unauthenticated_rps: 10,     // 10 req/sec for unauthenticated
            admin_rps: 1000,             // 1000 req/sec for admins
            burst_size: 50,              // Allow bursts up to 50 requests
        }
    }
}

/// Rate limiter manager
#[derive(Clone)]
pub struct RateLimiter {
    authenticated: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    unauthenticated: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    admin: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
}

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        let auth_quota = Quota::per_second(
            NonZeroU32::new(config.authenticated_rps)
                .unwrap_or(NonZeroU32::new(100).unwrap()),
        )
        .allow_burst(NonZeroU32::new(config.burst_size).unwrap_or(NonZeroU32::new(50).unwrap()));

        let unauth_quota = Quota::per_second(
            NonZeroU32::new(config.unauthenticated_rps)
                .unwrap_or(NonZeroU32::new(10).unwrap()),
        )
        .allow_burst(NonZeroU32::new(config.burst_size / 5).unwrap_or(NonZeroU32::new(10).unwrap()));

        let admin_quota = Quota::per_second(
            NonZeroU32::new(config.admin_rps)
                .unwrap_or(NonZeroU32::new(1000).unwrap()),
        )
        .allow_burst(
            NonZeroU32::new(config.burst_size * 2).unwrap_or(NonZeroU32::new(100).unwrap()),
        );

        Self {
            authenticated: Arc::new(GovernorLimiter::direct(auth_quota)),
            unauthenticated: Arc::new(GovernorLimiter::direct(unauth_quota)),
            admin: Arc::new(GovernorLimiter::direct(admin_quota)),
        }
    }

    /// Check rate limit for authenticated user
    pub fn check_authenticated(&self) -> PdsResult<()> {
        match self.authenticated.check() {
            Ok(_) => Ok(()),
            Err(_) => Err(PdsError::RateLimitExceeded {
                retry_after: std::time::Duration::from_secs(1),
            }),
        }
    }

    /// Check rate limit for unauthenticated user
    pub fn check_unauthenticated(&self) -> PdsResult<()> {
        match self.unauthenticated.check() {
            Ok(_) => Ok(()),
            Err(_) => Err(PdsError::RateLimitExceeded {
                retry_after: std::time::Duration::from_secs(1),
            }),
        }
    }

    /// Check rate limit for admin user
    pub fn check_admin(&self) -> PdsResult<()> {
        match self.admin.check() {
            Ok(_) => Ok(()),
            Err(_) => Err(PdsError::RateLimitExceeded {
                retry_after: std::time::Duration::from_secs(1),
            }),
        }
    }
}

/// Rate limiting middleware
pub async fn rate_limit_middleware(
    State(ctx): State<crate::context::AppContext>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Check if this is an admin endpoint
    let is_admin = request.uri().path().contains("/xrpc/com.atproto.admin");

    // Check if user is authenticated (has Authorization header)
    let has_auth_header = request
        .headers()
        .get("authorization")
        .is_some();

    // Apply appropriate rate limit based on context
    let rate_limit_result = if is_admin && has_auth_header {
        // Admin endpoints with auth - highest rate limit
        ctx.rate_limiter.check_admin()
    } else if has_auth_header {
        // Authenticated users - medium rate limit
        ctx.rate_limiter.check_authenticated()
    } else {
        // Unauthenticated users - lowest rate limit
        ctx.rate_limiter.check_unauthenticated()
    };

    // Check rate limit
    match rate_limit_result {
        Ok(_) => {
            // Rate limit check passed, continue to next handler
            let mut response = next.run(request).await;

            // Add rate limit headers to response
            let headers = response.headers_mut();
            headers.insert("X-RateLimit-Limit", "100".parse().unwrap());
            headers.insert("X-RateLimit-Remaining", "99".parse().unwrap());

            Ok(response)
        }
        Err(_) => {
            // Rate limit exceeded
            Err(StatusCode::TOO_MANY_REQUESTS)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_creation() {
        let config = RateLimitConfig::default();
        let limiter = RateLimiter::new(config);

        // Should allow first request
        assert!(limiter.check_authenticated().is_ok());
        assert!(limiter.check_unauthenticated().is_ok());
        assert!(limiter.check_admin().is_ok());
    }

    #[test]
    fn test_burst_limit() {
        let config = RateLimitConfig {
            authenticated_rps: 10,
            unauthenticated_rps: 5,
            admin_rps: 100,
            burst_size: 5,
        };
        let limiter = RateLimiter::new(config);

        // Should allow burst requests
        for _ in 0..5 {
            assert!(limiter.check_authenticated().is_ok());
        }

        // Should hit rate limit after burst
        assert!(limiter.check_authenticated().is_err());
    }
}
