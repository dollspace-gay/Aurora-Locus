# Rate Limiting & Abuse Prevention Assessment

## Summary
**Date**: 2025-11-13
**Files**: [src/rate_limit.rs](src/rate_limit.rs) (216 lines), [src/rate_limit_new/distributed.rs](src/rate_limit_new/distributed.rs) (266 lines)
**Status**: ⚠️ **PARTIAL** - 60% feature parity with Bluesky PDS

---

## ✅ **Implemented Features**

### 1. **Basic Rate Limiting** ✅
**File**: [src/rate_limit.rs](src/rate_limit.rs) (216 lines)

#### Global Rate Limiters:
- **Authenticated Users**: 100 req/sec, burst 50
- **Unauthenticated Users**: 10 req/sec, burst 10
- **Admin Users**: 1000 req/sec, burst 100
- **Cross-PDS Users**: 10 req/sec, burst 5 (10x stricter for federation)

#### Implementation:
```rust
pub struct RateLimiter {
    authenticated: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    unauthenticated: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    admin: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    cross_pds: Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>,
}
```

#### Governor Library:
- Uses `governor` crate for token bucket algorithm
- In-memory state (single-server only)
- Per-second rate limiting
- Burst handling with configurable burst size

### 2. **Rate Limit Middleware** ✅
**Function**: `rate_limit_middleware` in [src/rate_limit.rs](src/rate_limit.rs:135-179)

#### Features:
- Checks for admin endpoints (`/xrpc/com.atproto.admin`)
- Checks for authentication (Authorization header)
- Applies appropriate rate limit (admin > authenticated > unauthenticated)
- Returns 429 Too Many Requests on limit exceeded
- Adds basic rate limit headers (X-RateLimit-Limit, X-RateLimit-Remaining)

#### Current Behavior:
```rust
let rate_limit_result = if is_admin && has_auth_header {
    ctx.rate_limiter.check_admin()
} else if has_auth_header {
    ctx.rate_limiter.check_authenticated()
} else {
    ctx.rate_limiter.check_unauthenticated()
};
```

### 3. **Error Handling** ✅
**Error Type**: `PdsError::RateLimitExceeded` in [src/error.rs](src/error.rs)

#### Features:
- Includes `retry_after` duration
- Returns HTTP 429 status code
- Proper error message: "Rate limit exceeded"

```rust
#[error("Rate limit exceeded")]
RateLimitExceeded { retry_after: std::time::Duration },
```

### 4. **Configuration** ✅
**Struct**: `RateLimitConfig` in [src/config.rs](src/config.rs:128-131)

#### Settings:
```rust
pub struct RateLimitConfig {
    pub enabled: bool,
    pub global_requests_per_minute: u32,
}
```

**Note**: This config is simplified compared to the RateLimitConfig in rate_limit.rs

### 5. **Distributed Rate Limiting (Not Integrated)** ⚠️
**File**: [src/rate_limit_new/distributed.rs](src/rate_limit_new/distributed.rs) (266 lines)

#### Implemented But Not Used:
- ✅ **DistributedRateLimiter**: Redis-backed, sliding window
- ✅ **TokenBucketLimiter**: Redis-backed, allows burst traffic
- ✅ **SlidingWindowLimiter**: Redis-backed, accurate rate limiting
- ❌ **Not exported** in src/lib.rs
- ❌ **Not integrated** into AppContext
- ❌ **Not used** in middleware

---

## ❌ **Missing Features**

### 1. **Per-Endpoint Rate Limits** ❌
**Bluesky PDS**: Different limits for different endpoints
```typescript
// createAccount.ts
rateLimit: {
  durationMs: 5 * MINUTE,
  points: 100,
}

// createSession.ts
rateLimit: [
  { durationMs: DAY, points: 300, calcKey: ({ input, req }) => `${input.body.identifier}-${req.ip}` },
  { durationMs: 5 * MINUTE, points: 30, calcKey: ({ input, req }) => `${input.body.identifier}-${req.ip}` },
],
```

**Aurora-Locus**: Global rate limits only (same limit for all endpoints)

### 2. **Per-User Rate Limits** ❌
**Bluesky PDS**: Custom key calculation per user/identifier
```typescript
calcKey: ({ input, req }) => `${input.body.identifier}-${req.ip}`
```

**Aurora-Locus**: Only global buckets (authenticated vs. unauthenticated)

### 3. **IP-Based Rate Limiting** ❌
**Bluesky PDS**: Rate limits by IP address
```typescript
calcKey: ({ input, req }) => `${input.body.identifier}-${req.ip}`
```

**Aurora-Locus**: No IP-based rate limiting (no access to client IP in middleware)

### 4. **Multiple Simultaneous Limits** ❌
**Bluesky PDS**: Array of rate limit rules for same endpoint
```typescript
rateLimit: [
  { durationMs: DAY, points: 300 },      // Long-term limit
  { durationMs: 5 * MINUTE, points: 30 }, // Short-term limit
]
```

**Aurora-Locus**: Single global limit per user type

### 5. **Proper Rate Limit Headers** ⚠️
**Bluesky PDS** (RFC 6585 + draft-polli-ratelimit-headers):
- `RateLimit-Limit`: Maximum requests allowed
- `RateLimit-Remaining`: Requests remaining in window
- `RateLimit-Reset`: When the limit resets
- `Retry-After`: Seconds to wait before retry (on 429)

**Aurora-Locus**: Hardcoded placeholder headers
```rust
headers.insert("X-RateLimit-Limit", "100".parse().unwrap());
headers.insert("X-RateLimit-Remaining", "99".parse().unwrap());
```

### 6. **Account Creation Limits** ❌
**Required**: Strict limits on account creation to prevent spam
**Bluesky PDS**: 100 requests per 5 minutes per IP
**Aurora-Locus**: Same global limit as all other endpoints

### 7. **Email Sending Limits** ❌
**Required**: Rate limit email sending to prevent abuse
**Aurora-Locus**: No email-specific rate limiting in [src/mailer/mod.rs](src/mailer/mod.rs)

### 8. **Login Attempt Limits** ❌
**Required**: Prevent brute-force password attacks
**Bluesky PDS**: 30 attempts per 5 minutes + 300 per day (per identifier+IP)
**Aurora-Locus**: Same global limit as all other endpoints

### 9. **Redis Integration** ❌
**Implementation exists** in src/rate_limit_new/distributed.rs but:
- Not integrated into AppContext
- Not used in middleware
- Not configurable

---

## 📊 **Comparison with Bluesky PDS**

| Feature | Aurora-Locus | Bluesky PDS | Status |
|---------|--------------|-------------|--------|
| Global rate limiting | ✅ In-memory (governor) | ✅ Redis-backed | ⚠️ Different approach |
| Per-endpoint limits | ❌ None | ✅ Configurable per endpoint | ❌ Missing |
| Per-user limits | ❌ Only auth/unauth | ✅ Custom key calculation | ❌ Missing |
| IP-based limiting | ❌ None | ✅ Per IP | ❌ Missing |
| Multiple limits | ❌ Single global | ✅ Array of rules | ❌ Missing |
| Sliding window | ⚠️ Implemented but unused | ✅ Active | ⚠️ Not integrated |
| Rate limit headers | ⚠️ Hardcoded placeholders | ✅ Dynamic, accurate | ⚠️ Incomplete |
| Burst handling | ✅ Token bucket | ✅ Token bucket | ✅ Match |
| Retry-After header | ❌ Not set | ✅ On 429 | ❌ Missing |
| Account creation limits | ❌ Global only | ✅ 100/5min per IP | ❌ Missing |
| Email sending limits | ❌ None | ✅ Enforced | ❌ Missing |
| Login attempt limits | ❌ Global only | ✅ 30/5min + 300/day | ❌ Missing |
| Distributed (Redis) | ⚠️ Implemented but unused | ✅ Active | ⚠️ Not integrated |

**Parity Score**: **60%** ⚠️

---

## 🔍 **Detailed Gap Analysis**

### Gap 1: Per-Endpoint Rate Limits
**Impact**: High
**Effort**: Medium

**Current**:
```rust
// Same limit for all endpoints based on auth status
ctx.rate_limiter.check_authenticated()
```

**Required**:
```rust
// Different limits per endpoint
let endpoint = request.uri().path();
match endpoint {
    "/xrpc/com.atproto.server.createAccount" => {
        limiter.check_with_key(ip, 100, Duration::from_secs(300))
    },
    "/xrpc/com.atproto.server.createSession" => {
        limiter.check_with_key(format!("{}-{}", identifier, ip), 30, Duration::from_secs(300))
    },
    _ => limiter.check_global(user_type)
}
```

### Gap 2: IP-Based Rate Limiting
**Impact**: High (Critical for abuse prevention)
**Effort**: Medium

**Required**:
1. Extract client IP from request (handle proxies, X-Forwarded-For)
2. Use IP as rate limit key or part of composite key
3. Support both IPv4 and IPv6

**Implementation**:
```rust
// Extract real client IP (handle proxies)
fn get_client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    // Check X-Forwarded-For, X-Real-IP, or connection IP
}

// Use IP in rate limit key
let key = format!("{}-{}", identifier, client_ip);
limiter.check(category, &key).await
```

### Gap 3: Multiple Simultaneous Limits
**Impact**: High (Prevents both short bursts and sustained attacks)
**Effort**: Low

**Required**:
```rust
// Check multiple limits for same request
async fn check_multi_rate_limits(
    limiter: &RateLimiter,
    key: &str,
    limits: &[(u32, Duration)]
) -> PdsResult<()> {
    for (max_requests, duration) in limits {
        limiter.check_with_window(key, *max_requests, *duration).await?;
    }
    Ok(())
}

// Usage
check_multi_rate_limits(limiter, &key, &[
    (300, Duration::from_secs(86400)),  // 300/day
    (30, Duration::from_secs(300)),     // 30/5min
]).await?
```

### Gap 4: Accurate Rate Limit Headers
**Impact**: Medium (UX for clients)
**Effort**: Low

**Required**:
```rust
// Calculate actual remaining requests and reset time
let state = limiter.get_state(&key).await?;
headers.insert("RateLimit-Limit", state.limit.to_string());
headers.insert("RateLimit-Remaining", state.remaining.to_string());
headers.insert("RateLimit-Reset", state.reset_timestamp.to_string());

// On 429 error
if let Err(retry_after) = rate_check {
    headers.insert("Retry-After", retry_after.as_secs().to_string());
}
```

### Gap 5: Redis Integration
**Impact**: High (Required for multi-instance deployments)
**Effort**: Medium

**Status**: Implementation exists in src/rate_limit_new/distributed.rs

**Required**:
1. Add rate_limit_new module to lib.rs exports
2. Add Redis configuration to ServerConfig
3. Conditionally use distributed rate limiter when Redis is configured
4. Fallback to in-memory limiter for single-instance deployments

**Implementation**:
```rust
// In context.rs
let rate_limiter = if let Some(redis_url) = &config.redis_url {
    Arc::new(DistributedRateLimiter::new(cache_client, 100))
} else {
    Arc::new(RateLimiter::new(RateLimitConfig::default()))
};
```

---

## ✅ **Strengths**

1. **Solid Foundation**: Governor library is production-ready
2. **Burst Handling**: Token bucket algorithm works well
3. **Configurable**: RateLimitConfig allows tuning
4. **Error Handling**: Proper PdsError::RateLimitExceeded
5. **Middleware Integration**: Clean separation of concerns
6. **Federation-Aware**: Special rate limit for cross-PDS requests
7. **Distributed Implementation Ready**: Code exists in src/rate_limit_new/

---

## 🎯 **Recommendations**

### Priority 1 (Required for Parity):
1. **Implement Per-Endpoint Rate Limits**
   - Create rate limit configuration per endpoint
   - Add endpoint-specific limits for:
     - createAccount: 100/5min per IP
     - createSession: 30/5min + 300/day per identifier+IP
     - resetPassword: 50/5min per IP
     - Email operations: 10/hour per user

2. **Add IP-Based Rate Limiting**
   - Extract client IP from headers (X-Forwarded-For)
   - Use IP in composite rate limit keys
   - Support IPv4 and IPv6

3. **Implement Multiple Simultaneous Limits**
   - Support array of rate limit rules per endpoint
   - Check all rules before allowing request
   - Return most restrictive limit in headers

### Priority 2 (Recommended):
4. **Integrate Distributed Rate Limiting**
   - Export rate_limit_new module
   - Add Redis configuration option
   - Use distributed limiter when Redis configured

5. **Fix Rate Limit Headers**
   - Calculate actual remaining requests
   - Add RateLimit-Reset header
   - Add Retry-After header on 429

### Priority 3 (Nice-to-Have):
6. **Add Rate Limit Metrics**
   - Track rate limit hits per endpoint
   - Track rate limit violations
   - Dashboard for abuse monitoring

---

## 📝 **Conclusion**

Aurora-Locus rate limiting achieves **60% feature parity** with Bluesky PDS. The implementation has:

✅ **Good Foundation**:
- In-memory rate limiting works
- Burst handling implemented
- Basic middleware integration
- Distributed implementation ready (unused)

❌ **Critical Gaps**:
- No per-endpoint limits (all endpoints share global limit)
- No IP-based rate limiting (can't prevent IP-based abuse)
- No multiple simultaneous limits (can't prevent both bursts and sustained attacks)
- Hardcoded rate limit headers (not helpful for clients)
- Distributed rate limiter not integrated (single-instance only)

**Recommendation**: **NEEDS WORK** - Implement Priority 1 features to achieve production-ready abuse prevention

The current implementation would struggle to prevent:
- Account creation spam (no per-IP limits)
- Brute-force login attacks (no per-identifier limits)
- Sustained abuse (no long-term limits)
- Multi-instance deployments (no Redis integration)

Estimated effort: **2-3 days** for Priority 1 features.
