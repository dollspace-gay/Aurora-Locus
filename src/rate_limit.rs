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
    http::{HeaderMap, Method, StatusCode},
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
    net::{IpAddr, Ipv4Addr},
    num::NonZeroU32,
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Type alias for endpoint rate limiters (multiple limiters per endpoint)
type EndpointLimiters =
    Arc<HashMap<String, Vec<Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>>>>;

/// Rate limit state information for headers
#[derive(Debug, Clone)]
pub struct RateLimitInfo {
    /// Maximum requests allowed in the current window
    pub limit: u32,
    /// Requests remaining in the current window
    pub remaining: u32,
    /// Seconds until the rate limit window resets
    pub reset_seconds: u64,
}

/// Rate limiter configuration
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Master enable flag. When `false`, every rate-limit check
    /// (middleware + all per-method `check_*` calls) short-circuits to
    /// `Ok(())` without consuming tokens. Loaded from
    /// `PDS_RATE_LIMITS_ENABLED` via `config::RateLimitConfig` and
    /// propagated through `context::AppContext`. Production default:
    /// `true`. Phase B harness emits `false` so multi-call scenarios
    /// (scenario-13's three rapid sub-cases, scenario-15's N-concurrent
    /// burst) don't race the per-DID-per-endpoint bucket. See chainlink
    /// #153 for the dead-knob writeup; before #153 this field didn't
    /// exist and the env var was loaded but never reached enforcement.
    pub enabled: bool,
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
    /// Requests per second for handle resolution (outbound HTTP/DNS)
    pub handle_resolution_rps: u32,
    /// Requests per second for DID resolution (outbound HTTP to PLC directory)
    pub did_resolution_rps: u32,
    /// Bypass the rate limiter for GET requests that target admin static
    /// assets (HTML/JS/CSS/JSON under the configured admin asset paths).
    /// Defaults to `true` because the page-load fan-out (~47 parallel
    /// `<script>` and `<link>` requests per visit) exceeds the per-IP
    /// unauthenticated quota and produces spurious 429s. Production
    /// deployments rely on the reverse proxy for asset DDoS protection;
    /// the PDS rate limiter is for application-layer API protection.
    /// Setting this to `false` opts the asset paths back into the limiter.
    pub exempt_admin_assets: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,             // Rate limiting on by default; Phase B opts out
            authenticated_rps: 100,    // 100 req/sec for authenticated
            unauthenticated_rps: 10,   // 10 req/sec for unauthenticated
            admin_rps: 1000,           // 1000 req/sec for admins
            cross_pds_rps: 10,         // 10 req/sec for cross-PDS (10x stricter than local)
            burst_size: 50,            // Allow bursts up to 50 requests
            trust_proxy: false,        // Default to false for security (don't trust proxy headers)
            handle_resolution_rps: 50, // 50 req/sec for handle resolution (protect outbound)
            did_resolution_rps: 50,    // 50 req/sec for DID resolution (protect outbound)
            exempt_admin_assets: true, // Admin UI static assets bypass the limiter by default
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
    #[allow(dead_code)] // Future burst configuration
    pub fn with_burst(max_requests: u32, duration_secs: u64, burst_size: u32) -> Self {
        Self {
            max_requests,
            duration_secs,
            burst_size: Some(burst_size),
        }
    }

    /// Create quota for this endpoint limit
    fn to_quota(&self) -> Quota {
        let per_period =
            NonZeroU32::new(self.max_requests).unwrap_or(NonZeroU32::new(100).unwrap());
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
    /// Map of endpoint paths to rate limit rules (supports multiple limits per endpoint)
    /// Key is the full XRPC path, e.g., "/xrpc/com.atproto.server.createAccount"
    /// Value is a vector of rate limits that ALL must pass for the request to be allowed
    ///
    /// Example: Login endpoint with both short-term and long-term limits:
    /// ```ignore
    /// config.add_limits("/xrpc/com.atproto.server.createSession", vec![
    ///     EndpointRateLimit::new(30, 300),    // 30 per 5 minutes (burst protection)
    ///     EndpointRateLimit::new(300, 86400), // 300 per day (sustained attack protection)
    /// ]);
    /// ```
    pub endpoints: HashMap<String, Vec<EndpointRateLimit>>,
}

impl EndpointRateLimitConfig {
    /// Create a new empty configuration
    pub fn new() -> Self {
        Self {
            endpoints: HashMap::new(),
        }
    }

    /// Add a single rate limit for an endpoint
    ///
    /// If the endpoint already has limits, this appends to the existing list.
    /// All limits for an endpoint must pass for the request to be allowed.
    pub fn add_limit(&mut self, endpoint: &str, limit: EndpointRateLimit) {
        self.endpoints
            .entry(endpoint.to_string())
            .or_default()
            .push(limit);
    }

    /// Add multiple rate limits for an endpoint
    ///
    /// Replaces any existing limits for this endpoint.
    /// All limits must pass for the request to be allowed.
    ///
    /// # Example
    /// ```ignore
    /// config.add_limits("/xrpc/com.atproto.server.createSession", vec![
    ///     EndpointRateLimit::new(30, 300),    // 30 per 5 minutes (burst)
    ///     EndpointRateLimit::new(300, 86400), // 300 per day (sustained)
    /// ]);
    /// ```
    pub fn add_limits(&mut self, endpoint: &str, limits: Vec<EndpointRateLimit>) {
        self.endpoints.insert(endpoint.to_string(), limits);
    }

    /// Create default Bluesky-compatible configuration with multi-limit protection
    ///
    /// Critical endpoints have multiple simultaneous limits for sophisticated abuse prevention:
    /// - Short-term limits: Prevent burst attacks
    /// - Long-term limits: Prevent sustained attacks
    pub fn bluesky_defaults() -> Self {
        let mut config = Self::new();

        // Account creation: Multiple limits for comprehensive protection
        // - 100 per 5 minutes: Prevent rapid account creation
        // - 500 per day: Prevent sustained Sybil attacks
        config.add_limits(
            "/xrpc/com.atproto.server.createAccount",
            vec![
                EndpointRateLimit::new(100, 300),   // Short-term: 100 per 5 minutes
                EndpointRateLimit::new(500, 86400), // Long-term: 500 per day
            ],
        );

        // Login/createSession: Multiple limits for brute-force protection
        // - 30 per 5 minutes: Prevent rapid password guessing
        // - 300 per day: Prevent sustained brute-force attacks
        // Note: This should be combined with identifier+IP composite keys for maximum protection
        config.add_limits(
            "/xrpc/com.atproto.server.createSession",
            vec![
                EndpointRateLimit::new(30, 300),    // Short-term: 30 per 5 minutes
                EndpointRateLimit::new(300, 86400), // Long-term: 300 per day
            ],
        );

        // Password reset: Multiple limits to prevent abuse
        // - 50 per 5 minutes: Prevent rapid reset attempts
        // - 200 per day: Prevent email bombing
        config.add_limits(
            "/xrpc/com.atproto.server.requestPasswordReset",
            vec![
                EndpointRateLimit::new(50, 300),    // Short-term: 50 per 5 minutes
                EndpointRateLimit::new(200, 86400), // Long-term: 200 per day
            ],
        );

        // Email confirmation: Single limit (less critical)
        config.add_limit(
            "/xrpc/com.atproto.server.requestEmailConfirmation",
            EndpointRateLimit::new(10, 3600), // 10 per hour
        );

        // Blob upload: Multiple limits to prevent storage abuse
        // - 50 per hour: Prevent rapid uploads
        // - 500 per day: Prevent sustained storage attacks
        config.add_limits(
            "/xrpc/com.atproto.repo.uploadBlob",
            vec![
                EndpointRateLimit::new(50, 3600),   // Short-term: 50 per hour
                EndpointRateLimit::new(500, 86400), // Long-term: 500 per day
            ],
        );

        // Account deletion: Single strict limit (very sensitive operation)
        config.add_limit(
            "/xrpc/com.atproto.server.deleteAccount",
            EndpointRateLimit::new(3, 86400), // 3 per day
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
        IpAddr::V6(ipv6) => ipv6.is_loopback() || ipv6.is_unspecified() || ipv6.is_multicast(),
    }
}

/// Rate limiter manager
#[derive(Clone)]
pub struct RateLimiter {
    authenticated: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    unauthenticated: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    admin: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    cross_pds: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    /// Per-endpoint rate limiters (supports multiple simultaneous limits per endpoint)
    /// Each endpoint can have multiple rate limiters - ALL must pass for request to be allowed
    endpoint_limiters: EndpointLimiters,
    /// IP-based rate limiter (keyed by IP address)
    ip_limiter: Arc<GovernorLimiter<String, DashMap<String, InMemoryState>, DefaultClock>>,
    /// Whether to trust proxy headers for IP extraction
    pub trust_proxy: bool,
    /// Whether GET requests to admin static-asset paths bypass the limiter.
    /// See `is_admin_asset_exempt` for the matched path/method set.
    pub exempt_admin_assets: bool,
    /// Configuration for returning rate limit info
    config: RateLimitConfig,
    /// Request counts per window for state tracking (identifier -> (window_start, count))
    request_counts: Arc<DashMap<String, (u64, u32)>>,
    /// Handle resolution rate limiter (protects outbound HTTP/DNS requests)
    handle_resolution: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    /// DID resolution rate limiter (protects outbound HTTP to PLC directory)
    did_resolution: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    /// Per-handle rate limiter (keyed by handle being resolved)
    handle_key_limiter: Arc<GovernorLimiter<String, DashMap<String, InMemoryState>, DefaultClock>>,
    /// Per-DID rate limiter (keyed by DID being resolved)
    did_key_limiter: Arc<GovernorLimiter<String, DashMap<String, InMemoryState>, DefaultClock>>,
}

#[allow(dead_code)] // Future rate limit checking methods
impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self::with_endpoint_config(config, EndpointRateLimitConfig::new())
    }

    /// Create a new rate limiter with per-endpoint configuration
    pub fn with_endpoint_config(
        config: RateLimitConfig,
        endpoint_config: EndpointRateLimitConfig,
    ) -> Self {
        let auth_quota = Quota::per_second(
            NonZeroU32::new(config.authenticated_rps).unwrap_or(NonZeroU32::new(100).unwrap()),
        )
        .allow_burst(NonZeroU32::new(config.burst_size).unwrap_or(NonZeroU32::new(50).unwrap()));

        let unauth_quota = Quota::per_second(
            NonZeroU32::new(config.unauthenticated_rps).unwrap_or(NonZeroU32::new(10).unwrap()),
        )
        .allow_burst(
            NonZeroU32::new(config.burst_size / 5).unwrap_or(NonZeroU32::new(10).unwrap()),
        );

        let admin_quota = Quota::per_second(
            NonZeroU32::new(config.admin_rps).unwrap_or(NonZeroU32::new(1000).unwrap()),
        )
        .allow_burst(
            NonZeroU32::new(config.burst_size * 2).unwrap_or(NonZeroU32::new(100).unwrap()),
        );

        let cross_pds_quota = Quota::per_second(
            NonZeroU32::new(config.cross_pds_rps).unwrap_or(NonZeroU32::new(10).unwrap()),
        )
        .allow_burst(
            NonZeroU32::new(config.burst_size / 10).unwrap_or(NonZeroU32::new(5).unwrap()),
        );

        // IP-based rate limiter quota (10 requests per second per IP, burst of 20)
        let ip_quota = Quota::per_second(NonZeroU32::new(10).unwrap())
            .allow_burst(NonZeroU32::new(20).unwrap());

        // Identity resolution rate limiters (protect outbound requests)
        let handle_resolution_quota = Quota::per_second(
            NonZeroU32::new(config.handle_resolution_rps).unwrap_or(NonZeroU32::new(50).unwrap()),
        )
        .allow_burst(NonZeroU32::new(config.burst_size).unwrap_or(NonZeroU32::new(50).unwrap()));

        let did_resolution_quota = Quota::per_second(
            NonZeroU32::new(config.did_resolution_rps).unwrap_or(NonZeroU32::new(50).unwrap()),
        )
        .allow_burst(NonZeroU32::new(config.burst_size).unwrap_or(NonZeroU32::new(50).unwrap()));

        // Per-handle/per-DID keyed limiters (5 requests per second per key, burst of 10)
        // This prevents enumeration attacks on specific handles/DIDs
        let handle_key_quota = Quota::per_second(NonZeroU32::new(5).unwrap())
            .allow_burst(NonZeroU32::new(10).unwrap());

        let did_key_quota = Quota::per_second(NonZeroU32::new(5).unwrap())
            .allow_burst(NonZeroU32::new(10).unwrap());

        // Create per-endpoint rate limiters (supports multiple limits per endpoint)
        let mut endpoint_limiters = HashMap::new();
        for (path, limits) in endpoint_config.endpoints.iter() {
            let limiters: Vec<Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>> = limits
                .iter()
                .map(|limit| {
                    let quota = limit.to_quota();
                    Arc::new(GovernorLimiter::direct(quota))
                })
                .collect();
            endpoint_limiters.insert(path.clone(), limiters);
        }

        Self {
            authenticated: Arc::new(GovernorLimiter::direct(auth_quota)),
            unauthenticated: Arc::new(GovernorLimiter::direct(unauth_quota)),
            admin: Arc::new(GovernorLimiter::direct(admin_quota)),
            cross_pds: Arc::new(GovernorLimiter::direct(cross_pds_quota)),
            endpoint_limiters: Arc::new(endpoint_limiters),
            ip_limiter: Arc::new(GovernorLimiter::keyed(ip_quota)),
            trust_proxy: config.trust_proxy,
            exempt_admin_assets: config.exempt_admin_assets,
            config: config.clone(),
            request_counts: Arc::new(DashMap::new()),
            handle_resolution: Arc::new(GovernorLimiter::direct(handle_resolution_quota)),
            did_resolution: Arc::new(GovernorLimiter::direct(did_resolution_quota)),
            handle_key_limiter: Arc::new(GovernorLimiter::keyed(handle_key_quota)),
            did_key_limiter: Arc::new(GovernorLimiter::keyed(did_key_quota)),
        }
    }

    /// Create a rate limiter with Bluesky-compatible defaults
    pub fn with_bluesky_defaults(config: RateLimitConfig) -> Self {
        Self::with_endpoint_config(config, EndpointRateLimitConfig::bluesky_defaults())
    }

    /// Check rate limit for authenticated user
    pub fn check_authenticated(&self) -> PdsResult<()> {
        if !self.config.enabled { return Ok(()); }
        self.track_request("global:authenticated");
        match self.authenticated.check() {
            Ok(_) => Ok(()),
            Err(_) => Err(PdsError::RateLimitExceeded {
                retry_after: std::time::Duration::from_secs(1),
            }),
        }
    }

    /// Check rate limit for unauthenticated user
    pub fn check_unauthenticated(&self) -> PdsResult<()> {
        if !self.config.enabled { return Ok(()); }
        self.track_request("global:unauthenticated");
        match self.unauthenticated.check() {
            Ok(_) => Ok(()),
            Err(_) => Err(PdsError::RateLimitExceeded {
                retry_after: std::time::Duration::from_secs(1),
            }),
        }
    }

    /// Check rate limit for admin user
    pub fn check_admin(&self) -> PdsResult<()> {
        if !self.config.enabled { return Ok(()); }
        self.track_request("global:admin");
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
        if !self.config.enabled { return Ok(()); }
        self.track_request("global:cross_pds");
        match self.cross_pds.check() {
            Ok(_) => Ok(()),
            Err(_) => Err(PdsError::RateLimitExceeded {
                retry_after: std::time::Duration::from_secs(1),
            }),
        }
    }

    /// Check rate limit for handle resolution (outbound HTTP/DNS requests)
    ///
    /// This protects against:
    /// 1. Using the PDS as a proxy for handle enumeration
    /// 2. DDoS amplification attacks via handle resolution
    /// 3. Excessive outbound requests that could get the PDS blocked
    ///
    /// Uses both global limit and per-handle keyed limit to prevent
    /// both aggregate abuse and targeted enumeration.
    pub fn check_handle_resolution(&self, handle: &str) -> PdsResult<()> {
        if !self.config.enabled { return Ok(()); }
        self.track_request("identity:handle_resolution");

        // Check global handle resolution limit
        match self.handle_resolution.check() {
            Ok(_) => {}
            Err(_) => {
                return Err(PdsError::RateLimitExceeded {
                    retry_after: std::time::Duration::from_secs(1),
                });
            }
        }

        // Check per-handle limit (prevents targeted enumeration)
        let handle_key = handle.to_lowercase();
        match self.handle_key_limiter.check_key(&handle_key) {
            Ok(_) => Ok(()),
            Err(_) => Err(PdsError::RateLimitExceeded {
                retry_after: std::time::Duration::from_secs(5),
            }),
        }
    }

    /// Check rate limit for DID resolution (outbound HTTP to PLC directory)
    ///
    /// This protects against:
    /// 1. Using the PDS as a proxy for DID enumeration
    /// 2. DDoS amplification attacks via DID resolution
    /// 3. Excessive load on the PLC directory
    ///
    /// Uses both global limit and per-DID keyed limit to prevent
    /// both aggregate abuse and targeted enumeration.
    pub fn check_did_resolution(&self, did: &str) -> PdsResult<()> {
        if !self.config.enabled { return Ok(()); }
        self.track_request("identity:did_resolution");

        // Check global DID resolution limit
        match self.did_resolution.check() {
            Ok(_) => {}
            Err(_) => {
                return Err(PdsError::RateLimitExceeded {
                    retry_after: std::time::Duration::from_secs(1),
                });
            }
        }

        // Check per-DID limit (prevents targeted enumeration)
        match self.did_key_limiter.check_key(&did.to_string()) {
            Ok(_) => Ok(()),
            Err(_) => Err(PdsError::RateLimitExceeded {
                retry_after: std::time::Duration::from_secs(5),
            }),
        }
    }

    /// Check rate limit for signing key extraction
    ///
    /// This is a combined check for DID resolution (since keys come from DID docs)
    /// with an additional identifier for tracking key-specific requests.
    pub fn check_signing_key_resolution(&self, did: &str) -> PdsResult<()> {
        if !self.config.enabled { return Ok(()); }
        self.track_request("identity:signing_key");

        // Use DID resolution limits since signing keys come from DID documents
        self.check_did_resolution(did)
    }

    /// Check rate limit for a specific endpoint
    ///
    /// If the endpoint has multiple rate limiters configured, ALL must pass for the request to be allowed.
    /// Returns Ok if all endpoint-specific limits allow request, or None if no endpoint-specific limit exists.
    ///
    /// This enables sophisticated abuse prevention like:
    /// - Short-term limit (burst protection): 30 requests per 5 minutes
    /// - Long-term limit (sustained attack protection): 300 requests per day
    pub fn check_endpoint(&self, endpoint: &str) -> Option<PdsResult<()>> {
        if !self.config.enabled { return Some(Ok(())); }
        self.endpoint_limiters.get(endpoint).map(|limiters| {
            // Check ALL limiters - if any fail, return error
            for limiter in limiters.iter() {
                match limiter.check() {
                    Ok(_) => continue,
                    Err(_) => {
                        return Err(PdsError::RateLimitExceeded {
                            retry_after: Duration::from_secs(60), // Return 60s as retry-after for endpoint limits
                        });
                    }
                }
            }
            // All limiters passed
            Ok(())
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
        if !self.config.enabled { return Ok(()); }
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
        if !self.config.enabled { return Ok(()); }
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

    /// Get current timestamp in seconds
    fn current_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    /// Track a request for a given identifier in the current window
    fn track_request(&self, identifier: &str) {
        let now = Self::current_timestamp();
        let window_start = now; // For per-second quotas, each second is a new window

        self.request_counts
            .entry(identifier.to_string())
            .and_modify(|(win, count)| {
                if *win == window_start {
                    *count += 1;
                } else {
                    *win = window_start;
                    *count = 1;
                }
            })
            .or_insert((window_start, 1));
    }

    /// Get rate limit info for a specific limiter type
    ///
    /// Returns information about the rate limit (limit, remaining, reset) for use in response headers
    pub fn get_limit_info(&self, identifier: &str, limiter_type: &str) -> RateLimitInfo {
        let now = Self::current_timestamp();

        // Get the appropriate limit based on limiter type
        let limit = match limiter_type {
            "authenticated" => self.config.authenticated_rps,
            "unauthenticated" => self.config.unauthenticated_rps,
            "admin" => self.config.admin_rps,
            "cross_pds" => self.config.cross_pds_rps,
            _ => self.config.unauthenticated_rps, // Default to most restrictive
        };

        // Get request count for current window
        let (remaining, reset_seconds) = self
            .request_counts
            .get(identifier)
            .map(|entry| {
                let (window_start, count) = *entry;
                if window_start == now {
                    let remaining = limit.saturating_sub(count);
                    (remaining, 1) // Resets in 1 second (per-second quota)
                } else {
                    // Old window, reset happened
                    (limit, 1)
                }
            })
            .unwrap_or((limit, 1)); // No requests yet, full limit available

        RateLimitInfo {
            limit,
            remaining,
            reset_seconds,
        }
    }

    /// Get the current rate limit configuration
    ///
    /// Returns the configuration used to create this rate limiter.
    /// Useful for admin endpoints to view current settings.
    pub fn get_config(&self) -> &RateLimitConfig {
        &self.config
    }

    /// Master enable flag accessor. When `false`, every `check_*`
    /// method short-circuits to `Ok(())` and the middleware skips
    /// distributed + governor enforcement. Driven by
    /// `PDS_RATE_LIMITS_ENABLED` via `config::RateLimitConfig` (see
    /// chainlink #153). Consumed by the rate-limit middleware to gate
    /// the distributed-bucket layer alongside the method-level gates.
    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get the list of endpoints with custom rate limits
    ///
    /// Returns endpoints that have per-endpoint rate limiting configured.
    pub fn get_endpoint_limits(&self) -> Vec<(String, Vec<(u32, u64)>)> {
        // We can't directly access the original EndpointRateLimit config,
        // but we can report which endpoints have custom limits
        self.endpoint_limiters
            .keys()
            .map(|k| {
                // We don't have the original config, but we know the endpoint has limits
                // Return placeholder values - the actual limits are in the governor limiters
                (k.clone(), vec![(0, 0)]) // Placeholder indicating custom limits exist
            })
            .collect()
    }

    /// Get all endpoints with custom rate limits
    pub fn get_rate_limited_endpoints(&self) -> Vec<String> {
        self.endpoint_limiters.keys().cloned().collect()
    }

    /// Get current request count statistics
    ///
    /// Returns a snapshot of request counts per identifier for the current window.
    pub fn get_request_counts(&self) -> Vec<(String, u32)> {
        let now = Self::current_timestamp();
        self.request_counts
            .iter()
            .filter_map(|entry| {
                let (window_start, count) = *entry.value();
                // Only return counts from the current window (within last second)
                if now.saturating_sub(window_start) < 5 {
                    Some((entry.key().clone(), count))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get the total number of tracked request identifiers
    pub fn get_tracked_identifiers_count(&self) -> usize {
        self.request_counts.len()
    }

    /// Clear old request count entries (older than 60 seconds)
    ///
    /// This is useful for memory management in long-running servers.
    pub fn cleanup_old_counts(&self) {
        let now = Self::current_timestamp();
        self.request_counts
            .retain(|_, (window_start, _)| now.saturating_sub(*window_start) < 60);
    }
}

/// Determine whether a request targets an admin UI static asset that should
/// bypass the rate limiter when the exemption is enabled.
///
/// The admin UI's `index.html` fans out to ~47 parallel `<script>` / `<link>`
/// requests on first paint, which trivially exceeds the per-IP unauthenticated
/// quota (10 req/sec) and produces spurious 429s during smoke testing. The
/// PDS rate limiter is intended for application-layer API protection
/// (`/xrpc/*`, auth surfaces); asset-level DDoS protection is the reverse
/// proxy's responsibility.
///
/// The exemption is intentionally narrow:
///
/// * Method must be `GET`. Any non-GET (POST, PUT, DELETE, …) under
///   `/admin/*` remains rate-limited so future dynamic admin endpoints
///   inherit limiter coverage by default.
/// * Path must be one of:
///   - the three top-level admin HTML entry points
///     (`/admin/index.html`, `/admin/login.html`, `/admin/debug.html`)
///   - anything under `/admin/scripts/`, `/admin/styles/`,
///     `/admin/i18n/`, or `/admin/login/`
///
/// Anything else under `/admin/*` (including bare `/admin` and any future
/// subdirectory not listed above) stays rate-limited normally. Auth surfaces
/// such as `/admin-oauth/*` are unaffected — the prefix `/admin/` does not
/// match the hyphenated namespace.
pub fn is_admin_asset_exempt(path: &str, method: &Method) -> bool {
    if method != Method::GET {
        return false;
    }

    matches!(
        path,
        "/admin/index.html"
            | "/admin/login.html"
            | "/admin/password-login.html"
            | "/admin/debug.html"
    ) || path.starts_with("/admin/scripts/")
        || path.starts_with("/admin/styles/")
        || path.starts_with("/admin/i18n/")
        || path.starts_with("/admin/login/")
}

/// Rate limiting middleware
pub async fn rate_limit_middleware(
    State(ctx): State<crate::context::AppContext>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let endpoint_path = request.uri().path();

    // Master enable gate (chainlink #153). When PDS_RATE_LIMITS_ENABLED=false,
    // skip the distributed-bucket layer below + all governor checks in this
    // middleware. Method-level `check_*` calls in handlers (check_did_endpoint
    // etc.) are gated separately at the method entry, so the env flag fully
    // disables enforcement everywhere. Production default `true` preserves
    // existing behavior; Phase B harness emits `false` for deterministic
    // multi-call scenarios.
    if !ctx.rate_limiter.enabled() {
        return Ok(next.run(request).await);
    }

    // Bypass the limiter for admin UI static assets when the exemption is
    // enabled. This is path-and-method specific (see `is_admin_asset_exempt`)
    // so non-GET requests and non-asset admin paths still go through the
    // limiter. Returning early skips rate-limit header injection on purpose:
    // these responses are not rate-limited and shouldn't advertise a counter.
    if ctx.rate_limiter.exempt_admin_assets
        && is_admin_asset_exempt(endpoint_path, request.method())
    {
        return Ok(next.run(request).await);
    }

    // Extract client IP for rate limiting and logging
    let client_ip = extract_client_ip(request.headers(), ctx.rate_limiter.trust_proxy);

    // Log client IP for debugging and security monitoring
    if let Some(ip) = client_ip {
        tracing::debug!("Request from IP: {} to endpoint: {}", ip, endpoint_path);
    }

    // Check if this is an admin endpoint
    let is_admin = endpoint_path.contains("/xrpc/com.atproto.admin");

    // Check if user is authenticated (has Authorization header)
    let has_auth_header = request.headers().get("authorization").is_some();

    // PRIORITY 0: cross-instance distributed-rate-limit check
    // (Arc 7 Step 3). In Distributed mode this runs BEFORE the
    // governor's per-endpoint check so cross-instance correctness
    // is enforced first. The governor still runs as
    // per-instance defense-in-depth (PRIORITY 1+ below).
    //
    // Bucket key is `endpoint|<path>` — one bucket per
    // endpoint path, no IP/DID composite. IP/DID-keyed limits
    // remain governor-only (per-instance limits are about
    // protecting one instance from a single source; the
    // distributed substrate addresses the
    // per-endpoint-across-the-deployment view).
    //
    // Rate parameters: 100 tokens/sec, max=100. Tuned to be
    // tighter than the governor's default per-endpoint
    // quotas (which are typically minutes/day-scaled) so
    // the distributed check rate-limits FIRST under
    // contention; tuneability lands in a v0.6 candidate
    // entry alongside per-endpoint-config plumbing.
    if let Some(dist) = ctx.distributed_rate_limiter.as_ref() {
        let bucket_key = format!("endpoint|{}", endpoint_path);
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        match dist
            .try_consume(&bucket_key, 1, 100, 100, now_ms)
            .await
        {
            Ok(RateLimitOutcome::Allowed { .. }) => {
                // Continue to governor checks below.
            }
            Ok(RateLimitOutcome::RateLimited) => {
                let mut response =
                    Response::new(axum::body::Body::from("Too Many Requests"));
                *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
                response
                    .headers_mut()
                    .insert("Retry-After", "1".parse().unwrap());
                return Ok(response);
            }
            Err(e) => {
                // Distributed-store failure is operator-meaningful
                // but request-non-fatal: log and fall through to
                // the governor's per-instance check. Operators
                // with monitoring on `distributed_store_errors`
                // see this surface; end users see the request
                // continue.
                tracing::warn!(
                    bucket = %bucket_key,
                    error = %e,
                    "distributed rate-limit consult failed, falling through to governor"
                );
            }
        }
    }

    // PRIORITY 1: Check endpoint-specific rate limit first
    let rate_limit_result =
        if let Some(endpoint_result) = ctx.rate_limiter.check_endpoint(endpoint_path) {
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

    // Determine limiter type for header generation
    let limiter_type = if is_admin && has_auth_header {
        "admin"
    } else if has_auth_header {
        "authenticated"
    } else {
        "unauthenticated"
    };

    // Get identifier for rate limit info
    let identifier = format!("global:{}", limiter_type);

    // Check rate limit
    match rate_limit_result {
        Ok(_) => {
            // Get rate limit info for headers
            let limit_info = ctx.rate_limiter.get_limit_info(&identifier, limiter_type);

            // Rate limit check passed, continue to next handler
            let mut response = next.run(request).await;

            // Add dynamic rate limit headers to response (RFC 6585 + draft-polli-ratelimit-headers)
            let headers = response.headers_mut();

            // Standard headers (draft-polli-ratelimit-headers)
            headers.insert(
                "RateLimit-Limit",
                limit_info.limit.to_string().parse().unwrap(),
            );
            headers.insert(
                "RateLimit-Remaining",
                limit_info.remaining.to_string().parse().unwrap(),
            );
            headers.insert(
                "RateLimit-Reset",
                limit_info.reset_seconds.to_string().parse().unwrap(),
            );

            // Legacy headers for backward compatibility
            headers.insert(
                "X-RateLimit-Limit",
                limit_info.limit.to_string().parse().unwrap(),
            );
            headers.insert(
                "X-RateLimit-Remaining",
                limit_info.remaining.to_string().parse().unwrap(),
            );
            headers.insert(
                "X-RateLimit-Reset",
                limit_info.reset_seconds.to_string().parse().unwrap(),
            );

            Ok(response)
        }
        Err(e) => {
            // Rate limit exceeded - extract retry_after from error
            let retry_after_secs = if let PdsError::RateLimitExceeded { retry_after } = &e {
                retry_after.as_secs()
            } else {
                60 // Default to 60 seconds
            };

            // Create 429 response with Retry-After header
            let mut response = Response::new(axum::body::Body::from("Too Many Requests"));
            *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;

            let headers = response.headers_mut();
            headers.insert("Retry-After", retry_after_secs.to_string().parse().unwrap());

            // Also include rate limit headers showing exhausted limit
            let limit_info = ctx.rate_limiter.get_limit_info(&identifier, limiter_type);
            headers.insert(
                "RateLimit-Limit",
                limit_info.limit.to_string().parse().unwrap(),
            );
            headers.insert("RateLimit-Remaining", "0".parse().unwrap());
            headers.insert(
                "RateLimit-Reset",
                retry_after_secs.to_string().parse().unwrap(),
            );

            Ok(response)
        }
    }
}

// ============================================================================
// Arc 7 Step 3 — distributed-store rate-limit primitive.
//
// Sits alongside the in-process governor `RateLimiter` above.
// Selected by `DistributedStateMode`: `Distributed` constructs
// a `DistributedRateLimiter` that runs server-side arithmetic
// UPDATEs against `rate_limit_buckets`;
// `SingleInstanceInmemory` skips construction and the
// governor path runs unchanged. Per V04_DESIGN.md §6.3.5 the
// arithmetic UPDATE replaces the CAS-loop-and-retries pattern
// — concurrent requests for the same bucket serialise through
// Postgres's row lock rather than spinning on version
// conflicts.
//
// The SQL stays within sqlx::Any's portable subset: `CASE
// WHEN ... THEN ... ELSE ... END` instead of Postgres-only
// `LEAST(...)`. Verbose but works on both backends without
// runtime dispatch.
// ============================================================================

use sqlx::AnyPool;

/// Distributed rate-limit primitive. One pool reference + a
/// table-name constant. Mode-gated construction in
/// `AppContext::new` (Distributed mode only).
#[derive(Clone)]
pub struct DistributedRateLimiter {
    /// Maintenance pool — same one the substrate's
    /// `PostgresCasStore` uses. Isolated from `account_db` so
    /// rate-limit roundtrips can't starve regular request
    /// handling.
    pool: Arc<AnyPool>,
}

/// Outcome of a [`DistributedRateLimiter::try_consume`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitOutcome {
    /// Request allowed; the row's `tokens_remaining` after the
    /// deduction is included for caller-side observability
    /// (Prometheus gauges, rate-limit-headers, etc.).
    Allowed { tokens_remaining: i64 },
    /// Request rate-limited; insufficient tokens after refill.
    /// The caller decides the retry-after policy.
    RateLimited,
}

impl DistributedRateLimiter {
    pub fn new(pool: Arc<AnyPool>) -> Self {
        Self { pool }
    }

    /// Try to consume `cost` tokens from the bucket identified
    /// by `bucket_key`. Server-side arithmetic UPDATE per
    /// V04_DESIGN.md §6.3.5:
    ///
    /// 1. Compute the time-based refill: current tokens +
    ///    `(now - window_start) * refill_rate / 1000`, capped
    ///    at `max_tokens`.
    /// 2. If the refilled total is `>= cost`, deduct cost,
    ///    update `window_start` to now, increment version.
    ///    Otherwise the UPDATE affects zero rows — the
    ///    caller sees `RateLimited`.
    ///
    /// First-touch buckets (no row yet) get a best-effort
    /// INSERT at `max_tokens - cost`. Concurrent first-touch
    /// races resolve via the table's PRIMARY KEY: the loser
    /// retries the UPDATE.
    ///
    /// `refill_rate` is tokens-per-second (BIGINT). Sub-second
    /// precision requires scaling at the caller (multiply
    /// both refill_rate and the time-divisor).
    pub async fn try_consume(
        &self,
        bucket_key: &str,
        cost: i64,
        refill_rate: i64,
        max_tokens: i64,
        now_epoch_ms: i64,
    ) -> PdsResult<RateLimitOutcome> {
        // Single-statement atomic UPDATE: compute refill, check
        // sufficiency, deduct, all in one trip. Returns the
        // post-deduction `tokens_remaining` on success; affects
        // zero rows when either (a) bucket doesn't exist or
        // (b) bucket exists but is exhausted post-refill.
        //
        // CASE-based portability: Postgres LEAST() is not
        // portable to SQLite scalar form. The CASE expression
        // computes the capped refill twice (once in SET, once
        // in WHERE); both branches are folded into Postgres's
        // query plan, no measurable cost vs LEAST().
        let updated: Option<(i64,)> = sqlx::query_as(
            r#"
            UPDATE rate_limit_buckets
            SET
                tokens_remaining = CASE
                    WHEN tokens_remaining + (($1 - window_start_at_epoch_ms) * refill_rate / 1000) > max_tokens
                        THEN max_tokens - $2
                    ELSE tokens_remaining + (($1 - window_start_at_epoch_ms) * refill_rate / 1000) - $2
                END,
                window_start_at_epoch_ms = $1,
                version = version + 1
            WHERE bucket_key = $3
              AND (
                CASE
                    WHEN tokens_remaining + (($1 - window_start_at_epoch_ms) * refill_rate / 1000) > max_tokens
                        THEN max_tokens
                    ELSE tokens_remaining + (($1 - window_start_at_epoch_ms) * refill_rate / 1000)
                END
              ) >= $2
            RETURNING tokens_remaining
            "#,
        )
        .bind(now_epoch_ms)
        .bind(cost)
        .bind(bucket_key)
        .fetch_optional(self.pool.as_ref())
        .await
        .map_err(PdsError::Database)?;

        if let Some((remaining,)) = updated {
            return Ok(RateLimitOutcome::Allowed {
                tokens_remaining: remaining,
            });
        }

        // Zero rows affected — disambiguate. Probe for the row's
        // existence in a separate (small) SELECT. This path runs
        // on first-touch and on rate-limited buckets; both are
        // less common than the happy path above.
        let exists: Option<(i64,)> = sqlx::query_as(
            "SELECT 1 FROM rate_limit_buckets WHERE bucket_key = $1 LIMIT 1",
        )
        .bind(bucket_key)
        .fetch_optional(self.pool.as_ref())
        .await
        .map_err(PdsError::Database)?;

        if exists.is_some() {
            // Bucket exists; the UPDATE's WHERE refused. That's
            // a real rate-limit rejection (tokens insufficient
            // even after refill).
            return Ok(RateLimitOutcome::RateLimited);
        }

        // First-touch: INSERT a fresh bucket at (max_tokens - cost).
        // PRIMARY KEY collision with a concurrent first-touch
        // resolves by retrying the UPDATE — the racing caller
        // succeeded, and now the UPDATE path should work.
        let initial_tokens = max_tokens - cost;
        let insert_result = sqlx::query(
            r#"
            INSERT INTO rate_limit_buckets
                (bucket_key, tokens_remaining, max_tokens, refill_rate,
                 window_start_at_epoch_ms, version)
            VALUES ($1, $2, $3, $4, $5, 0)
            "#,
        )
        .bind(bucket_key)
        .bind(initial_tokens)
        .bind(max_tokens)
        .bind(refill_rate)
        .bind(now_epoch_ms)
        .execute(self.pool.as_ref())
        .await;

        match insert_result {
            Ok(_) => Ok(RateLimitOutcome::Allowed {
                tokens_remaining: initial_tokens,
            }),
            Err(e) if is_unique_violation_sql_err(&e) => {
                // Concurrent first-touch happened. The bucket
                // now exists; retry the UPDATE. Bounded retry —
                // one extra UPDATE max; if it again returns
                // zero rows the bucket was created and
                // immediately exhausted (legitimate rate-limit).
                let retry: Option<(i64,)> = sqlx::query_as(
                    r#"
                    UPDATE rate_limit_buckets
                    SET
                        tokens_remaining = CASE
                            WHEN tokens_remaining + (($1 - window_start_at_epoch_ms) * refill_rate / 1000) > max_tokens
                                THEN max_tokens - $2
                            ELSE tokens_remaining + (($1 - window_start_at_epoch_ms) * refill_rate / 1000) - $2
                        END,
                        window_start_at_epoch_ms = $1,
                        version = version + 1
                    WHERE bucket_key = $3
                      AND (
                        CASE
                            WHEN tokens_remaining + (($1 - window_start_at_epoch_ms) * refill_rate / 1000) > max_tokens
                                THEN max_tokens
                            ELSE tokens_remaining + (($1 - window_start_at_epoch_ms) * refill_rate / 1000)
                        END
                      ) >= $2
                    RETURNING tokens_remaining
                    "#,
                )
                .bind(now_epoch_ms)
                .bind(cost)
                .bind(bucket_key)
                .fetch_optional(self.pool.as_ref())
                .await
                .map_err(PdsError::Database)?;

                Ok(match retry {
                    Some((remaining,)) => RateLimitOutcome::Allowed {
                        tokens_remaining: remaining,
                    },
                    None => RateLimitOutcome::RateLimited,
                })
            }
            Err(e) => Err(PdsError::Database(e)),
        }
    }
}

/// Backend-specific unique-violation detection — mirrors the
/// helper in `src/distributed/postgres_cas.rs`. Not re-exported
/// because the dependency direction is inverted (this is the
/// rate-limit module reusing the substrate's pattern locally).
fn is_unique_violation_sql_err(err: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = err {
        matches!(
            db_err.code().as_deref(),
            Some("23505") | Some("1555") | Some("2067")
        )
    } else {
        false
    }
}

#[cfg(test)]
mod distributed_tests {
    //! `DistributedRateLimiter` unit tests against in-memory
    //! SQLite. Cross-instance behavior against real Postgres
    //! is exercised by `tests/distributed_substrate_test.rs`.
    use super::*;
    use sqlx::any::AnyPoolOptions;
    use std::sync::Once;

    async fn fresh_pool() -> Arc<AnyPool> {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE rate_limit_buckets (
                bucket_key                 TEXT PRIMARY KEY,
                tokens_remaining           BIGINT NOT NULL,
                max_tokens                 BIGINT NOT NULL,
                refill_rate                BIGINT NOT NULL,
                window_start_at_epoch_ms   BIGINT NOT NULL,
                version                    BIGINT NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        Arc::new(pool)
    }

    #[tokio::test]
    async fn first_touch_creates_bucket_and_allows() {
        let pool = fresh_pool().await;
        let limiter = DistributedRateLimiter::new(pool);
        let now = chrono::Utc::now().timestamp_millis();
        let outcome = limiter
            .try_consume("first-touch", 1, 10, 100, now)
            .await
            .unwrap();
        assert!(matches!(outcome, RateLimitOutcome::Allowed { .. }));
        if let RateLimitOutcome::Allowed { tokens_remaining } = outcome {
            assert_eq!(tokens_remaining, 99, "max_tokens=100, cost=1 → 99");
        }
    }

    #[tokio::test]
    async fn second_consume_deducts_from_existing_bucket() {
        let pool = fresh_pool().await;
        let limiter = DistributedRateLimiter::new(Arc::clone(&pool));
        let now = chrono::Utc::now().timestamp_millis();
        // First-touch creates at 99/100.
        limiter
            .try_consume("steady", 1, 10, 100, now)
            .await
            .unwrap();
        // Second consume 1ms later: refill ≈ 0 (no time), deduct
        // 1 more. Remaining = 98.
        let outcome = limiter
            .try_consume("steady", 1, 10, 100, now + 1)
            .await
            .unwrap();
        match outcome {
            RateLimitOutcome::Allowed { tokens_remaining } => {
                assert_eq!(tokens_remaining, 98);
            }
            other => panic!("expected Allowed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn exhausted_bucket_returns_rate_limited() {
        let pool = fresh_pool().await;
        let limiter = DistributedRateLimiter::new(pool);
        let now = chrono::Utc::now().timestamp_millis();
        // First-touch creates at 1/2 (max=2, cost=1).
        limiter
            .try_consume("small", 1, 0, 2, now)
            .await
            .unwrap();
        // Second deducts to 0.
        limiter
            .try_consume("small", 1, 0, 2, now + 1)
            .await
            .unwrap();
        // Third must rate-limit (refill_rate=0 → no refill).
        let outcome = limiter.try_consume("small", 1, 0, 2, now + 2).await.unwrap();
        assert!(matches!(outcome, RateLimitOutcome::RateLimited));
    }

    #[tokio::test]
    async fn refill_recovers_capacity_over_time() {
        let pool = fresh_pool().await;
        let limiter = DistributedRateLimiter::new(pool);
        let now = chrono::Utc::now().timestamp_millis();
        // 10 tokens/sec refill, max=10. Drain everything.
        for _ in 0..10 {
            limiter
                .try_consume("refilling", 1, 10, 10, now)
                .await
                .unwrap();
        }
        // Empty now. Without refill, next call rate-limits.
        let outcome = limiter
            .try_consume("refilling", 1, 10, 10, now + 1)
            .await
            .unwrap();
        assert!(matches!(outcome, RateLimitOutcome::RateLimited));

        // Wait 1 second → refill_rate * 1000ms / 1000 = 10 tokens.
        // Capped at max=10. Deduct 1 → 9 remaining.
        let later = now + 1000;
        let outcome = limiter
            .try_consume("refilling", 1, 10, 10, later)
            .await
            .unwrap();
        match outcome {
            RateLimitOutcome::Allowed { tokens_remaining } => {
                assert_eq!(tokens_remaining, 9, "refill capped at max, then deduct");
            }
            other => panic!("expected Allowed post-refill, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn refill_respects_max_tokens_cap() {
        let pool = fresh_pool().await;
        let limiter = DistributedRateLimiter::new(pool);
        let now = chrono::Utc::now().timestamp_millis();
        // First-touch at 1/10 (drained-ish).
        limiter
            .try_consume("capped", 9, 100, 10, now)
            .await
            .unwrap();
        // Wait an absurdly long time (one day). Refill would
        // overflow without the cap; should saturate at max=10
        // minus the next request's cost.
        let later = now + 86_400_000;
        let outcome = limiter
            .try_consume("capped", 1, 100, 10, later)
            .await
            .unwrap();
        match outcome {
            RateLimitOutcome::Allowed { tokens_remaining } => {
                assert_eq!(tokens_remaining, 9, "refill capped, then deduct 1");
            }
            other => panic!("expected Allowed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn distinct_buckets_have_independent_state() {
        let pool = fresh_pool().await;
        let limiter = DistributedRateLimiter::new(pool);
        let now = chrono::Utc::now().timestamp_millis();
        // Drain bucket A.
        limiter
            .try_consume("bucket-A", 1, 0, 1, now)
            .await
            .unwrap();
        let a_again = limiter.try_consume("bucket-A", 1, 0, 1, now + 1).await.unwrap();
        assert!(matches!(a_again, RateLimitOutcome::RateLimited));

        // Bucket B is independent — first-touch, fresh budget.
        let b = limiter.try_consume("bucket-B", 1, 0, 1, now + 1).await.unwrap();
        assert!(matches!(b, RateLimitOutcome::Allowed { .. }));
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
            ..Default::default()
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

        let limiter =
            RateLimiter::with_endpoint_config(RateLimitConfig::default(), endpoint_config);

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
        assert!(
            result.unwrap().is_err(),
            "Request after burst should be rate limited"
        );
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
        assert!(config
            .endpoints
            .contains_key("/xrpc/com.atproto.server.createAccount"));
        assert!(config
            .endpoints
            .contains_key("/xrpc/com.atproto.server.createSession"));

        // Check specific limits
        let create_account_limits = config
            .endpoints
            .get("/xrpc/com.atproto.server.createAccount")
            .unwrap();
        let create_account_limit = &create_account_limits[0];
        assert_eq!(create_account_limit.max_requests, 100);
        assert_eq!(create_account_limit.duration_secs, 300); // 5 minutes
    }

    #[test]
    fn test_extract_ip_from_x_forwarded_for() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "192.168.1.100, 10.0.0.1".parse().unwrap(),
        );

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
        let config = RateLimitConfig {
            trust_proxy: true,
            ..RateLimitConfig::default()
        };

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

    /// chainlink #153 — PDS_RATE_LIMITS_ENABLED=false must short-circuit
    /// every check_* method to Ok(()) regardless of bucket exhaustion.
    /// This mirrors the test_did_endpoint_rate_limiting saturation pattern
    /// (~30 rapid same-key calls) which is exactly what trips a default
    /// bucket — with `enabled: false`, the saturation never produces a
    /// 429 because each check returns immediately at the gate.
    #[test]
    fn test_disabled_flag_bypasses_all_checks() {
        let config = RateLimitConfig { enabled: false, ..Default::default() };
        let limiter = RateLimiter::new(config);

        // Saturate the per-DID-per-endpoint bucket — under default
        // enabled=true this is the exact pattern that hits 429 in
        // test_did_endpoint_rate_limiting; with enabled=false EVERY
        // call must succeed.
        let did = "did:plc:disabledtest";
        let endpoint = "/xrpc/com.atproto.repo.createRecord";
        for _ in 0..200 {
            assert!(
                limiter.check_did_endpoint(did, endpoint).is_ok(),
                "check_did_endpoint must short-circuit when enabled=false"
            );
        }

        // Same shape for check_identifier_ip (composite-key path,
        // distinct call site).
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        for _ in 0..200 {
            assert!(
                limiter.check_identifier_ip("disabled@test", &ip).is_ok(),
                "check_identifier_ip must short-circuit when enabled=false"
            );
        }

        // Direct check_composite_key — single bucket, fully gateable.
        for _ in 0..200 {
            assert!(
                limiter.check_composite_key("composite-disabled-key").is_ok(),
                "check_composite_key must short-circuit when enabled=false"
            );
        }

        // Per-auth-type globals.
        for _ in 0..1000 {
            assert!(limiter.check_authenticated().is_ok());
            assert!(limiter.check_unauthenticated().is_ok());
            assert!(limiter.check_admin().is_ok());
            assert!(limiter.check_cross_pds().is_ok());
        }

        // Outbound resolution buckets.
        for _ in 0..200 {
            assert!(limiter.check_handle_resolution("disabled.test").is_ok());
            assert!(limiter.check_did_resolution(did).is_ok());
            assert!(limiter.check_signing_key_resolution(did).is_ok());
        }

        // check_endpoint returns Option<PdsResult<()>>; when gated it
        // returns Some(Ok(())) so callers see "limit configured + allowed"
        // rather than the None "no limit configured" branch.
        let ep_result = limiter.check_endpoint("/xrpc/com.atproto.repo.createRecord");
        assert!(matches!(ep_result, Some(Ok(()))));

        // check_ip on a hot key — 200 rapid same-IP checks.
        for _ in 0..200 {
            assert!(limiter.check_ip(&ip).is_ok());
        }

        // Sanity: the SAME limiter with enabled=true still saturates
        // (so the test would fail if the gate was missing — proves the
        // gate is the only reason the calls above succeeded).
        let enabled_config = RateLimitConfig { enabled: true, ..Default::default() };
        let enabled_limiter = RateLimiter::new(enabled_config);
        let mut got_err = false;
        for _ in 0..200 {
            if enabled_limiter.check_did_endpoint(did, endpoint).is_err() {
                got_err = true;
                break;
            }
        }
        assert!(got_err, "enabled=true limiter MUST saturate at default burst");
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
            "/xrpc/com.atproto.repo.createRecord",
        );
        assert_eq!(key, "did:plc:user123-/xrpc/com.atproto.repo.createRecord");
    }

    // ========== Tests for Multiple Simultaneous Rate Limits ==========

    #[test]
    fn test_multiple_limits_per_endpoint() {
        // Test that endpoints can have multiple rate limits and ALL must pass
        let mut config = EndpointRateLimitConfig::new();

        // Add two limits: tight limit (5/minute) and loose limit (20/minute)
        config.add_limits(
            "/xrpc/test.endpoint",
            vec![
                EndpointRateLimit::with_burst(5, 60, 5), // 5 per minute with burst of 5
                EndpointRateLimit::with_burst(20, 60, 20), // 20 per minute with burst of 20
            ],
        );

        let limiter = RateLimiter::with_endpoint_config(RateLimitConfig::default(), config);

        // First 5 requests should succeed (within both limits)
        for i in 0..5 {
            let result = limiter.check_endpoint("/xrpc/test.endpoint");
            assert!(result.is_some(), "Request {} should have endpoint limit", i);
            assert!(
                result.unwrap().is_ok(),
                "Request {} should pass both limits",
                i
            );
        }

        // 6th request should fail (exceeds first limit of 5)
        let result = limiter.check_endpoint("/xrpc/test.endpoint");
        assert!(result.is_some());
        assert!(
            result.unwrap().is_err(),
            "Request 6 should fail the 5/minute limit"
        );
    }

    #[test]
    fn test_add_limit_appends() {
        // Test that add_limit appends to existing limits
        let mut config = EndpointRateLimitConfig::new();

        // Add first limit
        config.add_limit("/xrpc/test.endpoint", EndpointRateLimit::new(10, 60));

        // Add second limit (should append, not replace)
        config.add_limit("/xrpc/test.endpoint", EndpointRateLimit::new(5, 60));

        // Should have 2 limits for this endpoint
        assert_eq!(
            config.endpoints.get("/xrpc/test.endpoint").unwrap().len(),
            2
        );
    }

    #[test]
    fn test_add_limits_replaces() {
        // Test that add_limits replaces existing limits
        let mut config = EndpointRateLimitConfig::new();

        // Add initial limit
        config.add_limit("/xrpc/test.endpoint", EndpointRateLimit::new(10, 60));

        // Replace with multiple limits
        config.add_limits(
            "/xrpc/test.endpoint",
            vec![
                EndpointRateLimit::new(5, 60),
                EndpointRateLimit::new(20, 3600),
            ],
        );

        // Should have exactly 2 limits (not 3)
        assert_eq!(
            config.endpoints.get("/xrpc/test.endpoint").unwrap().len(),
            2
        );
    }

    #[test]
    fn test_multiple_limits_short_and_long_term() {
        // Simulate Bluesky-style login protection with short-term and long-term limits
        let mut config = EndpointRateLimitConfig::new();

        // Short-term: 10/minute (burst protection)
        // Long-term: 100/hour (sustained attack protection)
        config.add_limits(
            "/xrpc/com.atproto.server.createSession",
            vec![
                EndpointRateLimit::with_burst(10, 60, 10), // 10 per minute
                EndpointRateLimit::with_burst(100, 3600, 100), // 100 per hour
            ],
        );

        let limiter = RateLimiter::with_endpoint_config(RateLimitConfig::default(), config);

        // First 10 requests should succeed (within both limits)
        for i in 0..10 {
            let result = limiter.check_endpoint("/xrpc/com.atproto.server.createSession");
            assert!(result.is_some(), "Request {} should have endpoint limit", i);
            assert!(
                result.unwrap().is_ok(),
                "Request {} should pass both short-term and long-term limits",
                i
            );
        }

        // 11th request should fail (exceeds short-term limit of 10/minute)
        let result = limiter.check_endpoint("/xrpc/com.atproto.server.createSession");
        assert!(result.is_some());
        assert!(
            result.unwrap().is_err(),
            "Request 11 should fail the 10/minute short-term limit"
        );
    }

    #[test]
    fn test_all_limits_must_pass() {
        // Test that if ANY limit fails, the request is blocked
        let mut config = EndpointRateLimitConfig::new();

        // Two very different limits
        config.add_limits(
            "/xrpc/test.strict",
            vec![
                EndpointRateLimit::with_burst(3, 60, 3), // Very strict: 3/minute
                EndpointRateLimit::with_burst(1000, 60, 1000), // Very loose: 1000/minute
            ],
        );

        let limiter = RateLimiter::with_endpoint_config(RateLimitConfig::default(), config);

        // First 3 requests should succeed
        for i in 0..3 {
            let result = limiter.check_endpoint("/xrpc/test.strict");
            assert!(result.unwrap().is_ok(), "Request {} should pass", i);
        }

        // 4th request should fail due to strict limit, even though loose limit would allow it
        let result = limiter.check_endpoint("/xrpc/test.strict");
        assert!(
            result.unwrap().is_err(),
            "Request 4 should fail the 3/minute limit (most restrictive)"
        );
    }

    #[test]
    fn test_endpoint_with_single_limit_still_works() {
        // Ensure backward compatibility: single limit still works
        let mut config = EndpointRateLimitConfig::new();

        config.add_limit("/xrpc/test.single", EndpointRateLimit::with_burst(5, 60, 5));

        let limiter = RateLimiter::with_endpoint_config(RateLimitConfig::default(), config);

        // Should work like before
        for i in 0..5 {
            let result = limiter.check_endpoint("/xrpc/test.single");
            assert!(result.unwrap().is_ok(), "Request {} should pass", i);
        }

        let result = limiter.check_endpoint("/xrpc/test.single");
        assert!(result.unwrap().is_err(), "Request 6 should fail");
    }

    #[test]
    fn test_different_endpoints_independent_limits() {
        // Test that different endpoints have independent limit vectors
        let mut config = EndpointRateLimitConfig::new();

        // Endpoint 1: Two limits
        config.add_limits(
            "/xrpc/endpoint1",
            vec![
                EndpointRateLimit::new(5, 60),
                EndpointRateLimit::new(10, 60),
            ],
        );

        // Endpoint 2: One limit
        config.add_limit("/xrpc/endpoint2", EndpointRateLimit::new(20, 60));

        let limiter = RateLimiter::with_endpoint_config(RateLimitConfig::default(), config);

        // Endpoint 1 should have limits
        assert!(limiter.check_endpoint("/xrpc/endpoint1").is_some());

        // Endpoint 2 should have limits
        assert!(limiter.check_endpoint("/xrpc/endpoint2").is_some());

        // Endpoint 3 should NOT have limits
        assert!(limiter.check_endpoint("/xrpc/endpoint3").is_none());
    }

    // ========== Tests for Identity Resolution Rate Limiting ==========

    #[test]
    fn test_handle_resolution_rate_limiting() {
        let limiter = RateLimiter::new(RateLimitConfig::default());

        // First request should succeed
        let result = limiter.check_handle_resolution("alice.bsky.social");
        assert!(result.is_ok(), "First handle resolution should pass");

        // Different handle should also succeed (different key)
        let result = limiter.check_handle_resolution("bob.bsky.social");
        assert!(result.is_ok(), "Different handle should pass");
    }

    #[test]
    fn test_handle_resolution_per_handle_limit() {
        let limiter = RateLimiter::new(RateLimitConfig::default());
        let handle = "targeted.handle";

        // First several requests should succeed
        for i in 0..10 {
            let result = limiter.check_handle_resolution(handle);
            assert!(result.is_ok(), "Request {} for same handle should pass", i);
        }

        // After burst limit (10), should be rate limited
        let result = limiter.check_handle_resolution(handle);
        assert!(
            result.is_err(),
            "Request after burst should be rate limited"
        );

        // But different handle should still work
        let result = limiter.check_handle_resolution("other.handle");
        assert!(result.is_ok(), "Different handle should still pass");
    }

    #[test]
    fn test_did_resolution_rate_limiting() {
        let limiter = RateLimiter::new(RateLimitConfig::default());

        // First request should succeed
        let result = limiter.check_did_resolution("did:plc:user123");
        assert!(result.is_ok(), "First DID resolution should pass");

        // Different DID should also succeed (different key)
        let result = limiter.check_did_resolution("did:plc:user456");
        assert!(result.is_ok(), "Different DID should pass");
    }

    #[test]
    fn test_did_resolution_per_did_limit() {
        let limiter = RateLimiter::new(RateLimitConfig::default());
        let did = "did:plc:targeted";

        // First several requests should succeed
        for i in 0..10 {
            let result = limiter.check_did_resolution(did);
            assert!(result.is_ok(), "Request {} for same DID should pass", i);
        }

        // After burst limit (10), should be rate limited
        let result = limiter.check_did_resolution(did);
        assert!(
            result.is_err(),
            "Request after burst should be rate limited"
        );

        // But different DID should still work
        let result = limiter.check_did_resolution("did:plc:other");
        assert!(result.is_ok(), "Different DID should still pass");
    }

    #[test]
    fn test_signing_key_resolution_uses_did_limits() {
        let limiter = RateLimiter::new(RateLimitConfig::default());
        let did = "did:plc:keytest";

        // First request should succeed
        let result = limiter.check_signing_key_resolution(did);
        assert!(result.is_ok(), "First signing key resolution should pass");

        // Should share limits with DID resolution
        // Exhaust the per-DID limit
        for _ in 0..9 {
            let _ = limiter.check_signing_key_resolution(did);
        }

        // Both should now be rate limited for this DID
        let result = limiter.check_did_resolution(did);
        assert!(
            result.is_err(),
            "DID resolution should be rate limited after signing key exhausts limit"
        );
    }

    #[test]
    fn test_handle_case_insensitive() {
        let limiter = RateLimiter::new(RateLimitConfig::default());

        // Check handle in lowercase
        for _ in 0..10 {
            let _ = limiter.check_handle_resolution("alice.bsky.social");
        }

        // Same handle in different case should be rate limited (normalized to lowercase)
        let result = limiter.check_handle_resolution("ALICE.BSKY.SOCIAL");
        assert!(
            result.is_err(),
            "Uppercase handle should share limit with lowercase"
        );
    }

    #[test]
    fn test_identity_resolution_config() {
        let config = RateLimitConfig {
            handle_resolution_rps: 100,
            did_resolution_rps: 75,
            ..Default::default()
        };

        let limiter = RateLimiter::new(config);

        // Should be able to make multiple requests with higher global limit
        // Note: per-handle limit is still 5 rps with burst of 10, so use different handles
        for i in 0..50 {
            let handle = format!("handle{}.bsky.social", i);
            assert!(limiter.check_handle_resolution(&handle).is_ok());
        }
    }

    #[test]
    fn test_global_vs_per_key_limits() {
        // Test that global and per-key limits work independently
        let config = RateLimitConfig::default();
        let limiter = RateLimiter::new(config);

        // Per-key limit is 5 rps with burst of 10
        // Exhaust the per-handle limit for one handle
        for _ in 0..10 {
            let _ = limiter.check_handle_resolution("exhausted.handle");
        }

        // This handle is exhausted
        assert!(limiter.check_handle_resolution("exhausted.handle").is_err());

        // But other handles should still work (global limit not exhausted)
        assert!(limiter.check_handle_resolution("fresh.handle").is_ok());
    }

    #[test]
    fn test_admin_asset_exempt_listed_html_pages() {
        // The three top-level admin entry pages are exempt on GET.
        assert!(is_admin_asset_exempt("/admin/index.html", &Method::GET));
        assert!(is_admin_asset_exempt("/admin/login.html", &Method::GET));
        assert!(is_admin_asset_exempt("/admin/debug.html", &Method::GET));
    }

    #[test]
    fn test_admin_asset_exempt_subtree_dirs() {
        // Anything nested under the four asset subdirectories is exempt on GET.
        assert!(is_admin_asset_exempt(
            "/admin/scripts/app.js",
            &Method::GET
        ));
        assert!(is_admin_asset_exempt(
            "/admin/scripts/nested/dir/file.js",
            &Method::GET
        ));
        assert!(is_admin_asset_exempt(
            "/admin/styles/app.css",
            &Method::GET
        ));
        assert!(is_admin_asset_exempt(
            "/admin/styles/themes/dark.css",
            &Method::GET
        ));
        assert!(is_admin_asset_exempt("/admin/i18n/en.json", &Method::GET));
        assert!(is_admin_asset_exempt(
            "/admin/i18n/locales.json",
            &Method::GET
        ));
        assert!(is_admin_asset_exempt(
            "/admin/login/index.html",
            &Method::GET
        ));
    }

    #[test]
    fn test_admin_asset_exempt_method_must_be_get() {
        // Non-GET requests against asset paths stay rate-limited so future
        // dynamic admin endpoints inherit limiter coverage by default.
        for method in [
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::PATCH,
            Method::HEAD,
            Method::OPTIONS,
        ] {
            assert!(
                !is_admin_asset_exempt("/admin/scripts/app.js", &method),
                "method {} should not be exempt",
                method
            );
            assert!(
                !is_admin_asset_exempt("/admin/index.html", &method),
                "method {} should not be exempt for top-level html",
                method
            );
        }
    }

    #[test]
    fn test_admin_asset_exempt_only_listed_dirs() {
        // Paths inside /admin/ that aren't in the exemption list stay limited.
        assert!(!is_admin_asset_exempt("/admin", &Method::GET));
        assert!(!is_admin_asset_exempt("/admin/", &Method::GET));
        assert!(!is_admin_asset_exempt(
            "/admin/something_else",
            &Method::GET
        ));
        assert!(!is_admin_asset_exempt(
            "/admin/api/handler",
            &Method::GET
        ));
        // Bare directory prefixes without trailing slash should not match the
        // subtree rule (avoid an /admin/scriptsblah.js style false positive).
        assert!(!is_admin_asset_exempt("/admin/scripts", &Method::GET));
        assert!(!is_admin_asset_exempt(
            "/admin/scriptsblah.js",
            &Method::GET
        ));
    }

    #[test]
    fn test_admin_asset_exempt_xrpc_not_affected() {
        // API endpoints stay rate-limited regardless of method.
        assert!(!is_admin_asset_exempt(
            "/xrpc/com.atproto.repo.getRecord",
            &Method::GET
        ));
        assert!(!is_admin_asset_exempt(
            "/xrpc/com.atproto.server.createSession",
            &Method::POST
        ));
    }

    #[test]
    fn test_admin_asset_exempt_admin_oauth_not_affected() {
        // /admin-oauth/* is a hyphenated namespace, not under /admin/. The
        // exemption must not match it (auth surface stays rate-limited).
        assert!(!is_admin_asset_exempt(
            "/admin-oauth/callback",
            &Method::GET
        ));
        assert!(!is_admin_asset_exempt(
            "/admin-oauth/scripts/foo.js",
            &Method::GET
        ));
    }

    #[test]
    fn test_admin_asset_exempt_config_default_is_true() {
        // The exemption defaults to on so the admin UI loads without 429s
        // out of the box.
        assert!(RateLimitConfig::default().exempt_admin_assets);
    }

    #[test]
    fn test_admin_asset_exempt_flag_propagates_to_limiter() {
        // The runtime RateLimiter exposes the flag so the middleware can
        // read it without re-touching configuration.
        let on = RateLimiter::new(RateLimitConfig {
            exempt_admin_assets: true,
            ..RateLimitConfig::default()
        });
        assert!(on.exempt_admin_assets);

        let off = RateLimiter::new(RateLimitConfig {
            exempt_admin_assets: false,
            ..RateLimitConfig::default()
        });
        assert!(!off.exempt_admin_assets);
    }
}
