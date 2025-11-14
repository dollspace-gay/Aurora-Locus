/// Rate Limiting System
///
/// This module provides comprehensive rate limiting capabilities for Aurora Locus PDS,
/// including global limits, per-endpoint limits, IP-based limits, and per-user composite limits.
///
/// # Architecture
///
/// The rate limiting system uses multiple layers:
/// 1. **Global limits**: Separate limits for authenticated, unauthenticated, admin, and cross-PDS users
/// 2. **Per-endpoint limits**: Custom limits for specific XRPC endpoints
/// 3. **IP-based limits**: Track requests per client IP address
/// 4. **Composite key limits**: Combine multiple identifiers (e.g., user+IP, DID+endpoint)
///
/// # Composite Key Rate Limiting
///
/// Composite keys enable fine-grained rate limiting by combining multiple identifiers.
/// This is essential for preventing various types of abuse:
///
/// ## Pattern 1: Identifier+IP (Login Protection)
///
/// Prevents brute-force attacks on specific accounts by rate limiting attempts
/// from each IP address for each user identifier.
///
/// ```rust,ignore
/// // In your createSession handler:
/// use crate::rate_limit::extract_client_ip;
///
/// async fn create_session(
///     State(ctx): State<AppContext>,
///     headers: HeaderMap,
///     Json(body): Json<CreateSessionRequest>,
/// ) -> Result<Json<SessionResponse>, PdsError> {
///     // Extract client IP
///     let client_ip = extract_client_ip(&headers, ctx.rate_limiter.trust_proxy)
///         .ok_or(PdsError::BadRequest("Could not determine client IP".into()))?;
///
///     // Check rate limit: identifier+IP combination
///     ctx.rate_limiter.check_identifier_ip(&body.identifier, &client_ip)?;
///
///     // Proceed with session creation...
/// }
/// ```
///
/// ## Pattern 2: DID+Endpoint (Per-User Per-Endpoint)
///
/// Allows fair usage by limiting each user's requests to specific endpoints.
/// Prevents a single user from monopolizing resources.
///
/// ```rust,ignore
/// // In your authenticated endpoint handler:
/// async fn create_record(
///     State(ctx): State<AppContext>,
///     auth: AuthContext,
///     Json(body): Json<CreateRecordRequest>,
/// ) -> Result<Json<RecordResponse>, PdsError> {
///     // Check rate limit: user DID + endpoint combination
///     ctx.rate_limiter.check_did_endpoint(
///         &auth.did,
///         "/xrpc/com.atproto.repo.createRecord"
///     )?;
///
///     // Proceed with record creation...
/// }
/// ```
///
/// ## Pattern 3: Custom Composite Keys
///
/// For more complex scenarios, use `check_composite_key()` with any string key:
///
/// ```rust,ignore
/// // Custom key combining multiple factors
/// let key = format!("{}-{}-{}", org_id, user_id, resource_type);
/// ctx.rate_limiter.check_composite_key(&key)?;
/// ```
///
/// # Bluesky PDS Compatibility
///
/// The rate limiter includes Bluesky-compatible defaults via `with_bluesky_defaults()`:
/// - createAccount: 100 requests per 5 minutes
/// - createSession: 30 requests per 5 minutes
/// - Password reset: 50 requests per 5 minutes
/// - Blob upload: 50 requests per hour
///
/// # Examples
///
/// See the `tests` module for comprehensive examples of all rate limiting patterns.
///
use crate::error::{PdsError, PdsResult};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use dashmap::DashMap;
use governor::{
    clock::DefaultClock,
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter as GovernorLimiter,
};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    num::NonZeroU32,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

/// Rate limiter configuration
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Requests per second for authenticated users
    pub authenticated_rps: u32,
    /// Requests per second for unauthenticated users
    pub unauthenticated_rps: u32,
    /// Requests per second for admin users
    pub admin_rps: u32,
    /// Requests per second for cross-PDS authenticated users (Phase 4)
    pub cross_pds_rps: u32,
    /// Burst size
    pub burst_size: u32,
    /// Trust proxy headers (X-Forwarded-For, X-Real-IP) for IP extraction
    /// Set to true if behind a reverse proxy/load balancer
    pub trust_proxy: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            authenticated_rps: 100,      // 100 req/sec for authenticated
            unauthenticated_rps: 10,     // 10 req/sec for unauthenticated
            admin_rps: 1000,             // 1000 req/sec for admins
            cross_pds_rps: 10,           // 10 req/sec for cross-PDS (10x stricter than local)
            burst_size: 50,              // Allow bursts up to 50 requests
            trust_proxy: false,          // Default to false for security (don't trust proxy headers)
        }
    }
}

/// Per-endpoint rate limit rule
#[derive(Debug, Clone)]
pub struct EndpointRateLimit {
    /// Maximum number of requests allowed in the duration
    pub max_requests: u32,
    /// Duration of the rate limit window (e.g., 300 seconds for 5 minutes)
    pub duration_secs: u64,
    /// Burst size (optional, defaults to max_requests / 2)
    pub burst_size: Option<u32>,
}

impl EndpointRateLimit {
    /// Create a new endpoint rate limit
    pub fn new(max_requests: u32, duration_secs: u64) -> Self {
        Self {
            max_requests,
            duration_secs,
            burst_size: None,
        }
    }

    /// Create a new endpoint rate limit with custom burst size
    pub fn with_burst(max_requests: u32, duration_secs: u64, burst_size: u32) -> Self {
        Self {
            max_requests,
            duration_secs,
            burst_size: Some(burst_size),
        }
    }

    /// Create quota for this endpoint limit
    fn to_quota(&self) -> Quota {
        let per_period = NonZeroU32::new(self.max_requests).unwrap_or(NonZeroU32::new(100).unwrap());
        let duration = Duration::from_secs(self.duration_secs);
        let burst = self.burst_size.unwrap_or(self.max_requests / 2);

        Quota::with_period(duration)
            .unwrap_or_else(|| Quota::per_second(per_period))
            .allow_burst(NonZeroU32::new(burst).unwrap_or(NonZeroU32::new(10).unwrap()))
    }
}

/// Configuration for per-endpoint rate limits
#[derive(Debug, Clone, Default)]
pub struct EndpointRateLimitConfig {
    /// Map of endpoint paths to rate limit rules
    /// Key is the full XRPC path, e.g., "/xrpc/com.atproto.server.createAccount"
    pub endpoints: HashMap<String, EndpointRateLimit>,
}

impl EndpointRateLimitConfig {
    /// Create a new empty configuration
    pub fn new() -> Self {
        Self {
            endpoints: HashMap::new(),
        }
    }

    /// Add a rate limit for an endpoint
    pub fn add_limit(&mut self, endpoint: &str, limit: EndpointRateLimit) {
        self.endpoints.insert(endpoint.to_string(), limit);
    }

    /// Create default Bluesky-compatible configuration
    pub fn bluesky_defaults() -> Self {
        let mut config = Self::new();

        // Account creation: 100 requests per 5 minutes (300 seconds)
        config.add_limit(
            "/xrpc/com.atproto.server.createAccount",
            EndpointRateLimit::new(100, 300),
        );

        // Login/createSession: 30 per 5 minutes (this is one of multiple limits needed)
        config.add_limit(
            "/xrpc/com.atproto.server.createSession",
            EndpointRateLimit::new(30, 300),
        );

        // Password reset: 50 per 5 minutes
        config.add_limit(
            "/xrpc/com.atproto.server.requestPasswordReset",
            EndpointRateLimit::new(50, 300),
        );

        // Email confirmation: 10 per hour
        config.add_limit(
            "/xrpc/com.atproto.server.requestEmailConfirmation",
            EndpointRateLimit::new(10, 3600),
        );

        // Blob upload: 50 per hour (prevent storage abuse)
        config.add_limit(
            "/xrpc/com.atproto.repo.uploadBlob",
            EndpointRateLimit::new(50, 3600),
        );

        // Account deletion: 3 per day
        config.add_limit(
            "/xrpc/com.atproto.server.deleteAccount",
            EndpointRateLimit::new(3, 86400),
        );

        config
    }
}

/// IP extraction and validation helpers
///
/// Extracts the real client IP address from request headers, handling proxies and load balancers
pub fn extract_client_ip(headers: &HeaderMap, trust_proxy: bool) -> Option<IpAddr> {
    if trust_proxy {
        // Priority 1: X-Forwarded-For (standard proxy header)
        if let Some(forwarded) = headers.get("x-forwarded-for") {
            if let Ok(header_value) = forwarded.to_str() {
                // X-Forwarded-For can be: "client, proxy1, proxy2"
                // We want the first (leftmost) IP which is the original client
                if let Some(first_ip) = header_value.split(',').next() {
                    if let Some(ip) = parse_ip_address(first_ip.trim()) {
                        return Some(ip);
                    }
                }
            }
        }

        // Priority 2: X-Real-IP (alternative proxy header, typically single IP)
        if let Some(real_ip) = headers.get("x-real-ip") {
            if let Ok(header_value) = real_ip.to_str() {
                if let Some(ip) = parse_ip_address(header_value.trim()) {
                    return Some(ip);
                }
            }
        }
    }

    // Priority 3: Forwarded header (RFC 7239 standard)
    if trust_proxy {
        if let Some(forwarded) = headers.get("forwarded") {
            if let Ok(header_value) = forwarded.to_str() {
                // Format: "for=192.0.2.1, for=198.51.100.17"
                for part in header_value.split(',') {
                    if let Some(for_part) = part.split(';').find(|s| s.trim().starts_with("for=")) {
                        let ip_str = for_part.trim().trim_start_matches("for=");
                        // Handle quoted IPs and brackets for IPv6
                        let ip_str = ip_str.trim_matches('"').trim_matches('[').trim_matches(']');
                        if let Some(ip) = parse_ip_address(ip_str) {
                            return Some(ip);
                        }
                    }
                }
            }
        }
    }

    // Priority 4: If no proxy headers or trust_proxy=false, use a placeholder
    // In a real implementation, this would come from the connection itself
    // For now, we'll return localhost as we can't access the socket in middleware
    Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)))
}

/// Parse and validate an IP address string
fn parse_ip_address(ip_str: &str) -> Option<IpAddr> {
    // Try parsing as is
    if let Ok(ip) = IpAddr::from_str(ip_str) {
        return Some(normalize_ip(ip));
    }

    // Handle bracketed IPv6 (e.g., "[2001:db8::1]" or "[2001:db8::1]:8080")
    if ip_str.starts_with('[') {
        if let Some(end_bracket) = ip_str.find(']') {
            let ip_part = &ip_str[1..end_bracket];
            if let Ok(ip) = IpAddr::from_str(ip_part) {
                return Some(normalize_ip(ip));
            }
        }
    }

    // Try removing port if present (e.g., "192.168.1.1:8080")
    // Only do this for IPv4 (single colon)
    if ip_str.matches(':').count() == 1 {
        if let Some((ip_part, _port)) = ip_str.rsplit_once(':') {
            if let Ok(ip) = IpAddr::from_str(ip_part) {
                return Some(normalize_ip(ip));
            }
        }
    }

    None
}

/// Normalize IP address for consistent rate limiting
fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        // IPv6 addresses can have multiple representations
        // Convert IPv4-mapped IPv6 addresses to IPv4
        IpAddr::V6(ipv6) => {
            if let Some(ipv4) = ipv6.to_ipv4_mapped() {
                IpAddr::V4(ipv4)
            } else {
                IpAddr::V6(ipv6)
            }
        }
        IpAddr::V4(ipv4) => IpAddr::V4(ipv4),
    }
}

/// Check if an IP address is private/local (should not be used for rate limiting from proxies)
#[allow(dead_code)]
fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => {
            ipv4.is_private()
                || ipv4.is_loopback()
                || ipv4.is_link_local()
                || ipv4.is_broadcast()
                || ipv4.is_documentation()
        }
        IpAddr::V6(ipv6) => {
            ipv6.is_loopback() || ipv6.is_unspecified() || ipv6.is_multicast()
        }
    }
}

/// Rate limiter manager
#[derive(Clone)]
pub struct RateLimiter {
    authenticated: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    unauthenticated: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    admin: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    cross_pds: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    /// Per-endpoint rate limiters
    endpoint_limiters: Arc<HashMap<String, Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>>>,
    /// IP-based rate limiter (keyed by IP address)
    ip_limiter: Arc<GovernorLimiter<String, DashMap<String, InMemoryState>, DefaultClock>>,
    /// Whether to trust proxy headers for IP extraction
    pub trust_proxy: bool,
}

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self::with_endpoint_config(config, EndpointRateLimitConfig::new())
    }

    /// Create a new rate limiter with per-endpoint configuration
    pub fn with_endpoint_config(config: RateLimitConfig, endpoint_config: EndpointRateLimitConfig) -> Self {
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

        let cross_pds_quota = Quota::per_second(
            NonZeroU32::new(config.cross_pds_rps)
                .unwrap_or(NonZeroU32::new(10).unwrap()),
        )
        .allow_burst(
            NonZeroU32::new(config.burst_size / 10).unwrap_or(NonZeroU32::new(5).unwrap()),
        );

        // IP-based rate limiter quota (10 requests per second per IP, burst of 20)
        let ip_quota = Quota::per_second(
            NonZeroU32::new(10).unwrap(),
        )
        .allow_burst(NonZeroU32::new(20).unwrap());

        // Create per-endpoint rate limiters
        let mut endpoint_limiters = HashMap::new();
        for (path, limit) in endpoint_config.endpoints.iter() {
            let quota = limit.to_quota();
            endpoint_limiters.insert(
                path.clone(),
                Arc::new(GovernorLimiter::direct(quota)),
            );
        }

        Self {
            authenticated: Arc::new(GovernorLimiter::direct(auth_quota)),
            unauthenticated: Arc::new(GovernorLimiter::direct(unauth_quota)),
            admin: Arc::new(GovernorLimiter::direct(admin_quota)),
            cross_pds: Arc::new(GovernorLimiter::direct(cross_pds_quota)),
            endpoint_limiters: Arc::new(endpoint_limiters),
            ip_limiter: Arc::new(GovernorLimiter::keyed(ip_quota)),
            trust_proxy: config.trust_proxy,
        }
    }

    /// Create a rate limiter with Bluesky-compatible defaults
    pub fn with_bluesky_defaults(config: RateLimitConfig) -> Self {
        Self::with_endpoint_config(config, EndpointRateLimitConfig::bluesky_defaults())
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

    /// Check rate limit for cross-PDS authenticated user (Phase 4)
    ///
    /// This is 10x stricter than local authenticated users to prevent abuse
    /// from federated instances.
    pub fn check_cross_pds(&self) -> PdsResult<()> {
        match self.cross_pds.check() {
            Ok(_) => Ok(()),
            Err(_) => Err(PdsError::RateLimitExceeded {
                retry_after: std::time::Duration::from_secs(1),
            }),
        }
    }

    /// Check rate limit for a specific endpoint
    ///
    /// Returns Ok if endpoint-specific limit allows request, or None if no endpoint-specific limit exists
    pub fn check_endpoint(&self, endpoint: &str) -> Option<PdsResult<()>> {
        self.endpoint_limiters.get(endpoint).map(|limiter| {
            match limiter.check() {
                Ok(_) => Ok(()),
                Err(_) => Err(PdsError::RateLimitExceeded {
                    retry_after: Duration::from_secs(60), // Return 60s as retry-after for endpoint limits
                }),
            }
        })
    }

    /// Check if an endpoint has a specific rate limit configured
    pub fn has_endpoint_limit(&self, endpoint: &str) -> bool {
        self.endpoint_limiters.contains_key(endpoint)
    }

    /// Check rate limit for a specific IP address
    ///
    /// Uses keyed rate limiting to track requests per IP address
    pub fn check_ip(&self, ip: &IpAddr) -> PdsResult<()> {
        let key = ip.to_string();
        match self.ip_limiter.check_key(&key) {
            Ok(_) => Ok(()),
            Err(_) => Err(PdsError::RateLimitExceeded {
                retry_after: std::time::Duration::from_secs(1),
            }),
        }
    }

    /// Check rate limit with a composite key (e.g., identifier+IP for login attempts)
    ///
    /// Useful for endpoints like createSession where we want to rate limit by both
    /// identifier and IP to prevent distributed brute-force attacks
    pub fn check_composite_key(&self, key: &str) -> PdsResult<()> {
        let key_string = key.to_string();
        match self.ip_limiter.check_key(&key_string) {
            Ok(_) => Ok(()),
            Err(_) => Err(PdsError::RateLimitExceeded {
                retry_after: std::time::Duration::from_secs(60),
            }),
        }
    }

    /// Check rate limit using identifier+IP composite key
    ///
    /// This is the recommended approach for login endpoints to prevent
    /// brute-force attacks on specific accounts
    ///
    /// # Example
    /// ```ignore
    /// // In your createSession handler:
    /// let key = RateLimiter::make_identifier_ip_key(&identifier, &client_ip);
    /// rate_limiter.check_identifier_ip(&identifier, &client_ip)?;
    /// ```
    pub fn check_identifier_ip(&self, identifier: &str, ip: &IpAddr) -> PdsResult<()> {
        let key = Self::make_identifier_ip_key(identifier, ip);
        self.check_composite_key(&key)
    }

    /// Check rate limit using DID+endpoint composite key
    ///
    /// This is useful for per-user per-endpoint rate limiting
    ///
    /// # Example
    /// ```ignore
    /// // In your authenticated endpoint handler:
    /// rate_limiter.check_did_endpoint(&user_did, "/xrpc/com.atproto.repo.createRecord")?;
    /// ```
    pub fn check_did_endpoint(&self, did: &str, endpoint: &str) -> PdsResult<()> {
        let key = Self::make_did_endpoint_key(did, endpoint);
        self.check_composite_key(&key)
    }

    /// Create a composite key for identifier+IP rate limiting
    ///
    /// Format: `{identifier}-{ip}`
    pub fn make_identifier_ip_key(identifier: &str, ip: &IpAddr) -> String {
        format!("{}-{}", identifier, ip)
    }

    /// Create a composite key for DID+endpoint rate limiting
    ///
    /// Format: `{did}-{endpoint}`
    pub fn make_did_endpoint_key(did: &str, endpoint: &str) -> String {
        format!("{}-{}", did, endpoint)
    }
}

/// Rate limiting middleware
pub async fn rate_limit_middleware(
    State(ctx): State<crate::context::AppContext>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let endpoint_path = request.uri().path();

    // Extract client IP for rate limiting and logging
    let client_ip = extract_client_ip(request.headers(), ctx.rate_limiter.trust_proxy);

    // Log client IP for debugging and security monitoring
    if let Some(ip) = client_ip {
        tracing::debug!("Request from IP: {} to endpoint: {}", ip, endpoint_path);
    }

    // Check if this is an admin endpoint
    let is_admin = endpoint_path.contains("/xrpc/com.atproto.admin");

    // Check if user is authenticated (has Authorization header)
    let has_auth_header = request
        .headers()
        .get("authorization")
        .is_some();

    // PRIORITY 1: Check endpoint-specific rate limit first
    let rate_limit_result = if let Some(endpoint_result) = ctx.rate_limiter.check_endpoint(endpoint_path) {
        // Endpoint has a specific rate limit configured - use it
        endpoint_result
    } else {
        // PRIORITY 2: Fall back to global rate limits based on user type
        if is_admin && has_auth_header {
            // Admin endpoints with auth - highest rate limit
            ctx.rate_limiter.check_admin()
        } else if has_auth_header {
            // Authenticated users - medium rate limit
            ctx.rate_limiter.check_authenticated()
        } else {
            // Unauthenticated users - check both global and IP-based limits
            // First check global limit
            let global_result = ctx.rate_limiter.check_unauthenticated();

            // Then check IP-based limit if we have an IP and global limit passed
            match (global_result, client_ip) {
                (Ok(()), Some(ip)) => {
                    // Global limit passed, now check IP-specific limit
                    ctx.rate_limiter.check_ip(&ip)
                }
                (Ok(()), None) => {
                    // Global limit passed, but no IP - allow request
                    Ok(())
                }
                (Err(e), _) => {
                    // Global limit failed - return error
                    Err(e)
                }
            }
        }
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
            cross_pds_rps: 20,
            burst_size: 5,
            trust_proxy: false,
        };
        let limiter = RateLimiter::new(config);

        // Should allow burst requests
        for _ in 0..5 {
            assert!(limiter.check_authenticated().is_ok());
        }

        // Should hit rate limit after burst
        assert!(limiter.check_authenticated().is_err());
    }

    #[test]
    fn test_endpoint_rate_limit() {
        let mut endpoint_config = EndpointRateLimitConfig::new();
        endpoint_config.add_limit(
            "/xrpc/com.atproto.server.createAccount",
            EndpointRateLimit::with_burst(5, 60, 5), // 5 requests per minute with burst of 5
        );

        let limiter = RateLimiter::with_endpoint_config(
            RateLimitConfig::default(),
            endpoint_config,
        );

        // Should have endpoint limit configured
        assert!(limiter.has_endpoint_limit("/xrpc/com.atproto.server.createAccount"));
        assert!(!limiter.has_endpoint_limit("/xrpc/com.atproto.server.createSession"));

        // Should allow first requests up to burst limit
        for i in 0..5 {
            let result = limiter.check_endpoint("/xrpc/com.atproto.server.createAccount");
            assert!(result.is_some(), "Request {} should have endpoint limit", i);
            assert!(result.unwrap().is_ok(), "Request {} should be allowed", i);
        }

        // Should hit rate limit after burst
        let result = limiter.check_endpoint("/xrpc/com.atproto.server.createAccount");
        assert!(result.is_some());
        assert!(result.unwrap().is_err(), "Request after burst should be rate limited");
    }

    #[test]
    fn test_bluesky_defaults() {
        let limiter = RateLimiter::with_bluesky_defaults(RateLimitConfig::default());

        // Should have Bluesky endpoints configured
        assert!(limiter.has_endpoint_limit("/xrpc/com.atproto.server.createAccount"));
        assert!(limiter.has_endpoint_limit("/xrpc/com.atproto.server.createSession"));
        assert!(limiter.has_endpoint_limit("/xrpc/com.atproto.repo.uploadBlob"));
        assert!(limiter.has_endpoint_limit("/xrpc/com.atproto.server.deleteAccount"));

        // Should not have unconfigured endpoints
        assert!(!limiter.has_endpoint_limit("/xrpc/com.atproto.repo.getRecord"));
    }

    #[test]
    fn test_endpoint_fallback_to_global() {
        let limiter = RateLimiter::new(RateLimitConfig::default());

        // Endpoint without specific limit should return None
        let result = limiter.check_endpoint("/xrpc/com.atproto.repo.getRecord");
        assert!(result.is_none());

        // Global limits should still work
        assert!(limiter.check_authenticated().is_ok());
        assert!(limiter.check_unauthenticated().is_ok());
    }

    #[test]
    fn test_endpoint_rate_limit_config() {
        let config = EndpointRateLimitConfig::bluesky_defaults();

        // Should have expected endpoints
        assert!(config.endpoints.contains_key("/xrpc/com.atproto.server.createAccount"));
        assert!(config.endpoints.contains_key("/xrpc/com.atproto.server.createSession"));

        // Check specific limits
        let create_account_limit = config.endpoints.get("/xrpc/com.atproto.server.createAccount").unwrap();
        assert_eq!(create_account_limit.max_requests, 100);
        assert_eq!(create_account_limit.duration_secs, 300); // 5 minutes
    }

    #[test]
    fn test_extract_ip_from_x_forwarded_for() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "192.168.1.100, 10.0.0.1".parse().unwrap());

        let ip = extract_client_ip(&headers, true);
        assert!(ip.is_some());
        assert_eq!(ip.unwrap(), IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)));
    }

    #[test]
    fn test_extract_ip_from_x_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "203.0.113.42".parse().unwrap());

        let ip = extract_client_ip(&headers, true);
        assert!(ip.is_some());
        assert_eq!(ip.unwrap(), IpAddr::V4(Ipv4Addr::new(203, 0, 113, 42)));
    }

    #[test]
    fn test_extract_ip_from_forwarded_header() {
        let mut headers = HeaderMap::new();
        headers.insert("forwarded", "for=198.51.100.17".parse().unwrap());

        let ip = extract_client_ip(&headers, true);
        assert!(ip.is_some());
        assert_eq!(ip.unwrap(), IpAddr::V4(Ipv4Addr::new(198, 51, 100, 17)));
    }

    #[test]
    fn test_extract_ip_trust_proxy_false() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "192.168.1.100".parse().unwrap());

        // With trust_proxy=false, should ignore proxy headers and return localhost
        let ip = extract_client_ip(&headers, false);
        assert!(ip.is_some());
        assert_eq!(ip.unwrap(), IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
    }

    #[test]
    fn test_extract_ip_priority_order() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());
        headers.insert("x-real-ip", "5.6.7.8".parse().unwrap());
        headers.insert("forwarded", "for=9.10.11.12".parse().unwrap());

        // X-Forwarded-For should have highest priority
        let ip = extract_client_ip(&headers, true);
        assert!(ip.is_some());
        assert_eq!(ip.unwrap(), IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)));
    }

    #[test]
    fn test_parse_ip_with_port() {
        let ip = parse_ip_address("192.168.1.1:8080");
        assert!(ip.is_some());
        assert_eq!(ip.unwrap(), IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)));
    }

    #[test]
    fn test_parse_ipv6() {
        let ip = parse_ip_address("2001:db8::1");
        assert!(ip.is_some());
        assert!(matches!(ip.unwrap(), IpAddr::V6(_)));
    }

    #[test]
    fn test_parse_ipv6_with_brackets() {
        let ip = parse_ip_address("[2001:db8::1]:8080");
        assert!(ip.is_some());
        assert!(matches!(ip.unwrap(), IpAddr::V6(_)));
    }

    #[test]
    fn test_normalize_ipv4_mapped_ipv6() {
        // IPv4-mapped IPv6 address should be converted to IPv4
        let ipv6 = IpAddr::V6("::ffff:192.0.2.1".parse().unwrap());
        let normalized = normalize_ip(ipv6);
        assert_eq!(normalized, IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)));
    }

    #[test]
    fn test_rate_limiter_trust_proxy_config() {
        let mut config = RateLimitConfig::default();
        config.trust_proxy = true;

        let limiter = RateLimiter::new(config);
        assert!(limiter.trust_proxy);
    }

    #[test]
    fn test_ip_based_rate_limiting() {
        let limiter = RateLimiter::new(RateLimitConfig::default());
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));

        // First request should succeed
        let result1 = limiter.check_ip(&ip);
        assert!(result1.is_ok());

        // Request from different IP should also succeed
        let ip2 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2));
        let result2 = limiter.check_ip(&ip2);
        assert!(result2.is_ok());

        // After many requests from same IP, rate limit should be hit
        let mut last_result = Ok(());
        for _ in 0..30 {
            last_result = limiter.check_ip(&ip);
            if last_result.is_err() {
                break;
            }
        }
        assert!(last_result.is_err());

        // But different IP should still work
        let result3 = limiter.check_ip(&ip2);
        assert!(result3.is_ok());
    }

    #[test]
    fn test_composite_key_rate_limiting() {
        let limiter = RateLimiter::new(RateLimitConfig::default());

        // Test composite keys (identifier+IP)
        let key1 = "user123-192.168.1.1";
        let key2 = "user123-192.168.1.2";

        // First request should succeed
        let result1 = limiter.check_composite_key(key1);
        assert!(result1.is_ok());

        // Request with different composite key should succeed
        let result2 = limiter.check_composite_key(key2);
        assert!(result2.is_ok());

        // After many requests with same composite key, rate limit should be hit
        let mut last_result = Ok(());
        for _ in 0..30 {
            last_result = limiter.check_composite_key(key1);
            if last_result.is_err() {
                break;
            }
        }
        assert!(last_result.is_err());

        // But different composite key should still work
        let result3 = limiter.check_composite_key(key2);
        assert!(result3.is_ok());
    }

    #[test]
    fn test_identifier_ip_rate_limiting() {
        let limiter = RateLimiter::new(RateLimitConfig::default());
        let ip1 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2));

        // Test identifier+IP combination for login attempts
        let identifier = "alice@example.com";

        // First request should succeed
        let result1 = limiter.check_identifier_ip(identifier, &ip1);
        assert!(result1.is_ok());

        // Same identifier from different IP should also succeed (different key)
        let result2 = limiter.check_identifier_ip(identifier, &ip2);
        assert!(result2.is_ok());

        // After many requests from same identifier+IP, rate limit should be hit
        let mut last_result = Ok(());
        for _ in 0..30 {
            last_result = limiter.check_identifier_ip(identifier, &ip1);
            if last_result.is_err() {
                break;
            }
        }
        assert!(last_result.is_err());

        // But same identifier from different IP should still work
        let result3 = limiter.check_identifier_ip(identifier, &ip2);
        assert!(result3.is_ok());

        // And different identifier from same IP should also work
        let result4 = limiter.check_identifier_ip("bob@example.com", &ip1);
        assert!(result4.is_ok());
    }

    #[test]
    fn test_did_endpoint_rate_limiting() {
        let limiter = RateLimiter::new(RateLimitConfig::default());

        // Test DID+endpoint combination for per-user per-endpoint limiting
        let did1 = "did:plc:user123";
        let did2 = "did:plc:user456";
        let endpoint1 = "/xrpc/com.atproto.repo.createRecord";
        let endpoint2 = "/xrpc/com.atproto.repo.putRecord";

        // First request should succeed
        let result1 = limiter.check_did_endpoint(did1, endpoint1);
        assert!(result1.is_ok());

        // Same DID, different endpoint should also succeed (different key)
        let result2 = limiter.check_did_endpoint(did1, endpoint2);
        assert!(result2.is_ok());

        // Different DID, same endpoint should also succeed
        let result3 = limiter.check_did_endpoint(did2, endpoint1);
        assert!(result3.is_ok());

        // After many requests from same DID+endpoint, rate limit should be hit
        let mut last_result = Ok(());
        for _ in 0..30 {
            last_result = limiter.check_did_endpoint(did1, endpoint1);
            if last_result.is_err() {
                break;
            }
        }
        assert!(last_result.is_err());

        // But different DID or different endpoint should still work
        let result4 = limiter.check_did_endpoint(did1, endpoint2);
        assert!(result4.is_ok());

        let result5 = limiter.check_did_endpoint(did2, endpoint1);
        assert!(result5.is_ok());
    }

    #[test]
    fn test_make_identifier_ip_key() {
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
        let key = RateLimiter::make_identifier_ip_key("alice@example.com", &ip);
        assert_eq!(key, "alice@example.com-192.168.1.100");

        // Test with IPv6
        let ipv6 = IpAddr::V6("2001:db8::1".parse().unwrap());
        let key2 = RateLimiter::make_identifier_ip_key("bob@example.com", &ipv6);
        assert_eq!(key2, "bob@example.com-2001:db8::1");
    }

    #[test]
    fn test_make_did_endpoint_key() {
        let key = RateLimiter::make_did_endpoint_key(
            "did:plc:user123",
            "/xrpc/com.atproto.repo.createRecord"
        );
        assert_eq!(key, "did:plc:user123-/xrpc/com.atproto.repo.createRecord");
    }
}
