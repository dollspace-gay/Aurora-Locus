/// Distributed rate limiting module (Redis-backed)
///
/// This module provides Redis-backed distributed rate limiting for multi-instance deployments.
///
/// For single-instance deployments, use `crate::rate_limit::RateLimiter` (in-memory, faster).
/// For multi-instance deployments, use `DistributedRateLimiter` (Redis, shared state).

pub mod distributed;

pub use distributed::DistributedRateLimiter;
