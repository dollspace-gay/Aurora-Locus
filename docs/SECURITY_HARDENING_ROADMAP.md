# Security Hardening Roadmap

**Aurora Locus PDS - Path to Production Security**

Phase 6.8: Security & Authorization Model Comparison (Security Hardening Roadmap)
Date: 2025-11-05
Status: Roadmap Complete

---

## Executive Summary

This roadmap outlines **prioritized security enhancements** required to bring Aurora Locus PDS from **development/testing** to **production-ready** status.

**Timeline**: 3 phases over 6-9 months
- **Phase 7 (Critical)**: Production blockers (2-3 months)
- **Phase 8 (High Priority)**: Security hardening (2-3 months)
- **Phase 9 (Maintenance)**: Ongoing improvements (continuous)

**Current Status**: Phase 6 complete (OAuth 2.1 + DPoP implemented)
**Target**: Production deployment with security audit approval

---

## Table of Contents

1. [Roadmap Overview](#1-roadmap-overview)
2. [Phase 7: Critical Fixes (Production Blockers)](#2-phase-7-critical-fixes-production-blockers)
3. [Phase 8: Security Hardening](#3-phase-8-security-hardening)
4. [Phase 9: Continuous Improvement](#4-phase-9-continuous-improvement)
5. [Security Audit Checklist](#5-security-audit-checklist)
6. [Deployment Security](#6-deployment-security)

---

## 1. Roadmap Overview

### 1.1 Phases Summary

| Phase | Duration | Focus | Status |
|-------|----------|-------|--------|
| **Phase 6** | Complete | OAuth 2.1 + DPoP implementation | ✅ Done |
| **Phase 7** | 2-3 months | **Production blockers** (token hashing, nonce store, security audit) | 🔴 Critical |
| **Phase 8** | 2-3 months | **Security hardening** (circuit breaker, client verification, MFA) | 🟠 High |
| **Phase 9** | Ongoing | **Continuous improvement** (bug bounty, monitoring, audits) | 🟡 Maintenance |

### 1.2 Priority Levels

| Priority | Definition | Impact | Timeline |
|----------|------------|--------|----------|
| **P0 (Blocker)** | Must fix before production | Service compromise, mass data breach | Immediate |
| **P1 (Critical)** | High security risk | Single account compromise, DoS | 1-2 months |
| **P2 (High)** | Moderate security risk | Limited impact, defense-in-depth | 3-6 months |
| **P3 (Medium)** | Best practice, nice-to-have | Compliance, user experience | 6+ months |

---

## 2. Phase 7: Critical Fixes (Production Blockers)

**Duration**: 2-3 months
**Goal**: Fix all P0 issues preventing production deployment

### 2.1 P0-1: Access Token Hashing

**Issue**: Access tokens stored in plaintext in database
**Risk**: 🔴 **Critical** - Mass account compromise if database breached
**Current State**: Tokens stored as `token_id VARCHAR` in database

**Implementation**:

```rust
// File: src/oauth/token.rs

use sha2::{Sha256, Digest};

/// Hash access token with SHA-256 before storage
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

// When issuing token
pub async fn issue_access_token(
    ctx: &AppContext,
    client_id: &str,
    did: &str,
    scope: &str,
) -> PdsResult<String> {
    // Generate token (cryptographically random)
    let token = generate_random_token(32); // 256-bit token

    // Hash for database storage
    let token_hash = hash_token(&token);

    // Store hash (not plaintext)
    sqlx::query(
        "INSERT INTO token (token_id, did, client_id, scope, dpop_thumbprint, device_id, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&token_hash) // Store hash
    .bind(did)
    .bind(client_id)
    .bind(scope)
    .bind(dpop_thumbprint)
    .bind(device_id)
    .bind(expires_at)
    .execute(&ctx.account_db)
    .await?;

    // Return plaintext token to client (only time it's visible)
    Ok(token)
}

// When validating token
pub async fn validate_oauth_token(
    ctx: &AppContext,
    access_token: &str,
) -> Result<OAuthToken, PdsError> {
    // Hash incoming token
    let token_hash = hash_token(access_token);

    // Look up by hash
    let row = sqlx::query(
        "SELECT token_id, did, client_id, scope, dpop_thumbprint, device_id, expires_at
         FROM token
         WHERE token_id = ?"
    )
    .bind(&token_hash) // Query by hash
    .fetch_optional(&ctx.account_db)
    .await?
    .ok_or_else(|| PdsError::Authentication("Invalid or expired access token"))?;

    // ... rest of validation
}
```

**Files to Modify**:
- [src/oauth/token.rs](../src/oauth/token.rs) - Add hashing functions
- [src/auth.rs:312-349](../src/auth.rs#L312-L349) - Update validation
- [src/account/mod.rs](../src/account/mod.rs) - Update token generation

**Testing**:
- [ ] Unit tests for `hash_token()`
- [ ] Integration tests for token issuance + validation
- [ ] Migration script for existing tokens (⚠️ requires re-issuance)

**Acceptance Criteria**:
- ✅ All new tokens stored as SHA-256 hashes
- ✅ Database never contains plaintext tokens
- ✅ Token validation works with hashed lookup
- ✅ Existing tokens migrated (or expired)

**Timeline**: 2 weeks
**Assignee**: Security Team
**Status**: 🔴 Not started

---

### 2.2 P0-2: Nonce Store Migration to Redis

**Issue**: DPoP and Service Auth nonces stored in-memory HashMap
**Risk**: 🔴 **Critical** - Nonce store DoS, replay attacks after restart
**Current State**: `Arc<RwLock<HashMap<String, i64>>>` in memory

**Implementation**:

```rust
// File: src/federation/nonce_store_redis.rs

use redis::{Client, Commands, RedisResult};

pub struct RedisNonceStore {
    client: Client,
    prefix: String, // "dpop_nonce:" or "service_nonce:"
    ttl: i64,       // Time-to-live in seconds
}

impl RedisNonceStore {
    pub fn new(redis_url: &str, prefix: &str, ttl: i64) -> PdsResult<Self> {
        let client = Client::open(redis_url)
            .map_err(|e| PdsError::Internal(format!("Redis connection failed: {}", e)))?;

        Ok(Self {
            client,
            prefix: prefix.to_string(),
            ttl,
        })
    }

    /// Generate new nonce
    pub async fn generate_nonce(&self) -> PdsResult<String> {
        let nonce = uuid::Uuid::new_v4().to_string();
        let key = format!("{}{}", self.prefix, nonce);

        // Store with TTL
        let mut conn = self.client.get_connection()
            .map_err(|e| PdsError::Internal(format!("Redis error: {}", e)))?;

        conn.set_ex(&key, 1, self.ttl as usize)
            .map_err(|e| PdsError::Internal(format!("Redis SET failed: {}", e)))?;

        debug!("Generated nonce: {} (TTL: {}s)", nonce, self.ttl);

        Ok(nonce)
    }

    /// Check and consume nonce (single-use)
    pub async fn check_and_consume_nonce(&self, nonce: &str) -> PdsResult<bool> {
        let key = format!("{}{}", self.prefix, nonce);

        let mut conn = self.client.get_connection()
            .map_err(|e| PdsError::Internal(format!("Redis error: {}", e)))?;

        // Check if exists, then delete (atomic operation)
        let exists: bool = conn.exists(&key)
            .map_err(|e| PdsError::Internal(format!("Redis EXISTS failed: {}", e)))?;

        if exists {
            conn.del(&key)
                .map_err(|e| PdsError::Internal(format!("Redis DEL failed: {}", e)))?;

            debug!("Nonce consumed: {}", nonce);
            Ok(true)
        } else {
            warn!("Nonce not found or already used: {}", nonce);
            Ok(false)
        }
    }

    /// Cleanup expired nonces (handled by Redis TTL)
    pub async fn cleanup_expired(&self) -> PdsResult<usize> {
        // Redis automatically expires keys based on TTL
        // This method is a no-op but kept for interface compatibility
        Ok(0)
    }
}
```

**Configuration** ([.env.example:115-118](../.env.example#L115-L118)):

```env
# Redis Configuration
REDIS_URL=redis://localhost:6379
DPOP_NONCE_TTL=300         # 5 minutes
SERVICE_NONCE_TTL=120      # 2 minutes
```

**Files to Modify**:
- [src/federation/dpop.rs:46-119](../src/federation/dpop.rs#L46-L119) - Replace HashMap with Redis
- [src/federation/nonce_store.rs](../src/federation/nonce_store.rs) - Update to use Redis
- [src/context.rs](../src/context.rs) - Add Redis client initialization
- [Cargo.toml](../Cargo.toml) - Add `redis` crate

**Testing**:
- [ ] Unit tests for Redis nonce operations
- [ ] Integration tests for replay prevention
- [ ] Load testing (1000 nonces/second)
- [ ] Redis failover testing

**Acceptance Criteria**:
- ✅ All nonces stored in Redis
- ✅ TTL enforced by Redis (no manual cleanup)
- ✅ Single-use consumption (atomic check-and-delete)
- ✅ Survives PDS restart (nonces persist)
- ✅ Handles Redis connection failures gracefully

**Timeline**: 2 weeks
**Assignee**: Backend Team
**Status**: 🔴 Not started

---

### 2.3 P0-3: Security Audit of Custom OAuth Implementation

**Issue**: Custom OAuth 2.1 implementation not battle-tested
**Risk**: 🔴 **Critical** - Unknown security vulnerabilities
**Current State**: Phase 6 implementation complete, not audited

**Audit Scope**:

1. **OAuth 2.1 Core**:
   - Authorization code flow
   - PKCE implementation
   - Token issuance and validation
   - Refresh token rotation
   - Scope enforcement

2. **DPoP Implementation**:
   - JWK parsing and validation
   - DPoP proof verification
   - Nonce-based replay prevention
   - Thumbprint computation (RFC 7638)

3. **Service Auth**:
   - Cross-PDS JWT verification
   - DID resolution and key extraction
   - Nonce tracking

4. **Attack Vectors**:
   - CSRF attacks
   - Authorization code interception
   - Token theft
   - Replay attacks
   - Scope escalation
   - JWT forgery

**Audit Process**:

```
Week 1-2: Code Review
├─ Static analysis (Clippy, cargo-audit)
├─ Manual code review by security expert
└─ Threat modeling

Week 3-4: Penetration Testing
├─ OAuth flow fuzzing
├─ DPoP proof tampering
├─ Token theft attempts
├─ Cross-PDS attack simulation
└─ Race condition testing

Week 5: Remediation
├─ Fix critical vulnerabilities
├─ Address high-priority findings
└─ Re-test fixed issues

Week 6: Re-Audit
├─ Verify all fixes
└─ Final security approval
```

**Recommended Auditors**:
- Trail of Bits (OAuth/Web3 security)
- NCC Group (Application security)
- Cure53 (Web application security)

**Deliverables**:
- [ ] Security audit report (vulnerabilities, recommendations)
- [ ] Penetration test results
- [ ] Remediation plan
- [ ] Final security approval letter

**Acceptance Criteria**:
- ✅ No critical or high-severity vulnerabilities
- ✅ All medium-severity vulnerabilities addressed or accepted
- ✅ Penetration testing passed
- ✅ Security audit sign-off

**Timeline**: 6-8 weeks
**Cost**: $50,000 - $100,000
**Status**: 🔴 Not started

---

### 2.4 P0-4: DID Signing Key Extraction Fix

**Issue**: Simplified multibase decoding may fail with some DID formats
**Risk**: 🟠 **High** - Service auth failures with certain DIDs
**Current State**: Basic multibase → bytes conversion

**Implementation**:

```rust
// File: src/identity/resolver.rs

use multibase::Base;
use p256::ecdsa::VerifyingKey;
use p256::pkcs8::DecodePublicKey;

/// Extract signing key from DID document with proper multibase handling
pub async fn get_signing_key(&self, did: &str) -> PdsResult<Vec<u8>> {
    // 1. Resolve DID document
    let did_doc = self.resolve_did(did).await?;

    // 2. Find atproto verification method
    let verification_method = did_doc
        .verification_method
        .iter()
        .find(|vm| vm.id.ends_with("#atproto"))
        .ok_or_else(|| PdsError::Internal("No atproto verification method found"))?;

    // 3. Extract multibase-encoded public key
    let multibase_key = &verification_method.public_key_multibase;

    // 4. Decode multibase (proper handling of all bases)
    let (base, key_bytes) = multibase::decode(multibase_key)
        .map_err(|e| PdsError::Internal(format!("Invalid multibase encoding: {}", e)))?;

    // 5. Verify base is supported (base58btc is standard)
    if base != Base::Base58Btc {
        warn!("Unexpected multibase encoding: {:?}, expected Base58Btc", base);
    }

    // 6. Parse as P-256 public key (SPKI format)
    let verifying_key = VerifyingKey::from_public_key_der(&key_bytes)
        .map_err(|e| PdsError::Internal(format!("Invalid P-256 public key: {}", e)))?;

    // 7. Convert to PEM format for jsonwebtoken
    let public_key_der = verifying_key.to_public_key_der()
        .map_err(|e| PdsError::Internal(format!("Failed to encode public key: {}", e)))?;

    let public_key_pem = format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
        base64::engine::general_purpose::STANDARD.encode(public_key_der.as_bytes())
    );

    Ok(public_key_pem.into_bytes())
}
```

**Testing**:
- [ ] Test with did:plc (PLC directory)
- [ ] Test with did:web (DNS-based)
- [ ] Test with did:key (self-describing keys)
- [ ] Test with various multibase encodings (base58btc, base64, etc.)

**Acceptance Criteria**:
- ✅ Handles all DID methods (plc, web, key)
- ✅ Handles all multibase encodings
- ✅ Proper error messages for invalid keys
- ✅ Backward compatible with existing DIDs

**Timeline**: 1 week
**Assignee**: Identity Team
**Status**: 🔴 Not started

---

### Phase 7 Summary

| Task | Priority | Timeline | Status |
|------|----------|----------|--------|
| **P0-1: Token Hashing** | 🔴 Blocker | 2 weeks | Not started |
| **P0-2: Redis Nonce Store** | 🔴 Blocker | 2 weeks | Not started |
| **P0-3: Security Audit** | 🔴 Blocker | 6-8 weeks | Not started |
| **P0-4: DID Key Extraction** | 🟠 High | 1 week | Not started |

**Total Duration**: 8-10 weeks (parallel execution)
**Estimated Cost**: $50,000 - $100,000 (audit only)

---

## 3. Phase 8: Security Hardening

**Duration**: 2-3 months
**Goal**: Implement defense-in-depth security enhancements

### 3.1 P1-1: Circuit Breaker for Federated Calls

**Issue**: No circuit breaker for cross-PDS calls (cascading failures)
**Risk**: 🟠 **High** - Service degradation from failing federated instances
**Current State**: Direct calls to federated PDS instances

**Implementation**:

```rust
// File: src/federation/circuit_breaker.rs

use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CircuitState {
    Closed,   // Normal operation
    Open,     // Failing, reject requests
    HalfOpen, // Testing recovery
}

pub struct CircuitBreaker {
    /// State per PDS instance
    states: Arc<RwLock<HashMap<String, CircuitBreakerState>>>,

    /// Failure threshold (consecutive failures before opening)
    failure_threshold: u32,

    /// Timeout duration when circuit is open
    timeout_duration: chrono::Duration,

    /// Success threshold in half-open state before closing
    success_threshold: u32,
}

struct CircuitBreakerState {
    state: CircuitState,
    failure_count: u32,
    success_count: u32,
    last_failure_time: Option<DateTime<Utc>>,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, timeout_duration: chrono::Duration) -> Self {
        Self {
            states: Arc::new(RwLock::new(HashMap::new())),
            failure_threshold,
            timeout_duration,
            success_threshold: 2, // Require 2 successes to close
        }
    }

    /// Check if request is allowed
    pub async fn call_allowed(&self, pds_did: &str) -> bool {
        let mut states = self.states.write().await;
        let state = states.entry(pds_did.to_string())
            .or_insert_with(|| CircuitBreakerState {
                state: CircuitState::Closed,
                failure_count: 0,
                success_count: 0,
                last_failure_time: None,
            });

        match state.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check if timeout expired
                if let Some(last_failure) = state.last_failure_time {
                    let elapsed = Utc::now().signed_duration_since(last_failure);
                    if elapsed > self.timeout_duration {
                        // Transition to half-open
                        state.state = CircuitState::HalfOpen;
                        state.success_count = 0;
                        info!("Circuit breaker for {} transitioned to HalfOpen", pds_did);
                        return true;
                    }
                }
                false // Still open
            }
            CircuitState::HalfOpen => true, // Allow limited requests
        }
    }

    /// Record success
    pub async fn record_success(&self, pds_did: &str) {
        let mut states = self.states.write().await;
        if let Some(state) = states.get_mut(pds_did) {
            match state.state {
                CircuitState::HalfOpen => {
                    state.success_count += 1;
                    if state.success_count >= self.success_threshold {
                        // Close circuit
                        state.state = CircuitState::Closed;
                        state.failure_count = 0;
                        state.success_count = 0;
                        info!("Circuit breaker for {} closed (recovered)", pds_did);
                    }
                }
                CircuitState::Closed => {
                    // Reset failure count on success
                    state.failure_count = 0;
                }
                _ => {}
            }
        }
    }

    /// Record failure
    pub async fn record_failure(&self, pds_did: &str) {
        let mut states = self.states.write().await;
        let state = states.entry(pds_did.to_string())
            .or_insert_with(|| CircuitBreakerState {
                state: CircuitState::Closed,
                failure_count: 0,
                success_count: 0,
                last_failure_time: None,
            });

        state.failure_count += 1;
        state.last_failure_time = Some(Utc::now());

        if state.failure_count >= self.failure_threshold {
            if state.state != CircuitState::Open {
                state.state = CircuitState::Open;
                warn!("Circuit breaker for {} opened (too many failures)", pds_did);
            }
        }
    }
}

// Usage in federated search
pub async fn search_federated_pds(
    &self,
    pds_did: &str,
    query: &str,
) -> PdsResult<Vec<SearchResult>> {
    // Check circuit breaker
    if !self.circuit_breaker.call_allowed(pds_did).await {
        warn!("Circuit breaker open for {}, skipping", pds_did);
        return Ok(Vec::new()); // Return empty results
    }

    // Make request
    match self.make_federated_request(pds_did, query).await {
        Ok(results) => {
            self.circuit_breaker.record_success(pds_did).await;
            Ok(results)
        }
        Err(e) => {
            self.circuit_breaker.record_failure(pds_did).await;
            Err(e)
        }
    }
}
```

**Configuration**:
```env
CIRCUIT_BREAKER_FAILURE_THRESHOLD=3    # Open after 3 failures
CIRCUIT_BREAKER_TIMEOUT_SECONDS=60     # Stay open for 60 seconds
CIRCUIT_BREAKER_SUCCESS_THRESHOLD=2    # Require 2 successes to close
```

**Timeline**: 2 weeks
**Status**: 🟠 Not started

---

### 3.2 P1-2: OAuth Client Verification (Domain Ownership)

**Issue**: No verification that OAuth client owns redirect_uri domain
**Risk**: 🔴 **Critical** - Phishing attacks (malicious apps impersonate legitimate apps)
**Current State**: Basic client registration without verification

**Implementation** (DNS TXT Record Verification):

```rust
// File: src/oauth/client_verification.rs

use trust_dns_resolver::TokioAsyncResolver;
use trust_dns_resolver::config::*;

pub struct ClientVerifier {
    dns_resolver: TokioAsyncResolver,
}

impl ClientVerifier {
    pub fn new() -> PdsResult<Self> {
        let resolver = TokioAsyncResolver::tokio(
            ResolverConfig::default(),
            ResolverOpts::default(),
        );

        Ok(Self {
            dns_resolver: resolver,
        })
    }

    /// Verify domain ownership via DNS TXT record
    pub async fn verify_domain_ownership(
        &self,
        domain: &str,
        verification_token: &str,
    ) -> PdsResult<bool> {
        // Look up TXT records for domain
        let txt_records = self.dns_resolver.txt_lookup(domain).await
            .map_err(|e| PdsError::Internal(format!("DNS lookup failed: {}", e)))?;

        // Check if verification token exists
        let expected = format!("atproto-verify={}", verification_token);

        for record in txt_records {
            for txt in record.txt_data() {
                if String::from_utf8_lossy(txt) == expected {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }
}

// Client registration flow
pub async fn register_client(
    ctx: State<AppContext>,
    auth: AdminAuthContext,
    body: Json<RegisterClientBody>,
) -> PdsResult<Json<ClientRegistrationResponse>> {
    // 1. Validate redirect_uri
    let redirect_url = Url::parse(&body.redirect_uri)
        .map_err(|e| PdsError::Validation(format!("Invalid redirect_uri: {}", e)))?;

    let domain = redirect_url.host_str()
        .ok_or_else(|| PdsError::Validation("Invalid domain in redirect_uri"))?;

    // 2. Generate verification token
    let verification_token = generate_verification_token();

    // 3. Store pending client
    sqlx::query(
        "INSERT INTO pending_oauth_clients (client_id, client_name, redirect_uri, domain, verification_token, created_at)
         VALUES (?, ?, ?, ?, ?, ?)"
    )
    .bind(&body.client_id)
    .bind(&body.client_name)
    .bind(&body.redirect_uri)
    .bind(domain)
    .bind(&verification_token)
    .bind(Utc::now())
    .execute(&ctx.account_db)
    .await?;

    // 4. Return verification instructions
    Ok(Json(ClientRegistrationResponse {
        client_id: body.client_id.clone(),
        verification_token,
        instructions: format!(
            "Add the following DNS TXT record to {} to verify domain ownership:\n\n\
             Name: {}\n\
             Type: TXT\n\
             Value: atproto-verify={}\n\n\
             Then call POST /oauth/clients/{}/verify",
            domain, domain, verification_token, body.client_id
        ),
    }))
}

// Verification endpoint
pub async fn verify_client(
    ctx: State<AppContext>,
    auth: AdminAuthContext,
    client_id: Path<String>,
) -> PdsResult<Json<ClientVerificationResponse>> {
    // 1. Get pending client
    let pending = sqlx::query_as::<_, PendingClient>(
        "SELECT * FROM pending_oauth_clients WHERE client_id = ?"
    )
    .bind(&*client_id)
    .fetch_optional(&ctx.account_db)
    .await?
    .ok_or_else(|| PdsError::NotFound("Client not found"))?;

    // 2. Verify domain ownership
    let verified = ctx.client_verifier
        .verify_domain_ownership(&pending.domain, &pending.verification_token)
        .await?;

    if !verified {
        return Err(PdsError::Validation(
            "Domain ownership not verified. Please add the DNS TXT record."
        ));
    }

    // 3. Move to verified clients
    sqlx::query(
        "INSERT INTO oauth_clients (client_id, client_name, redirect_uri, verified, created_at)
         VALUES (?, ?, ?, TRUE, ?)"
    )
    .bind(&pending.client_id)
    .bind(&pending.client_name)
    .bind(&pending.redirect_uri)
    .bind(Utc::now())
    .execute(&ctx.account_db)
    .await?;

    // 4. Delete pending client
    sqlx::query("DELETE FROM pending_oauth_clients WHERE client_id = ?")
        .bind(&*client_id)
        .execute(&ctx.account_db)
        .await?;

    Ok(Json(ClientVerificationResponse {
        client_id: pending.client_id,
        verified: true,
        message: "Client verified successfully!".to_string(),
    }))
}
```

**UI Changes**:
- [ ] Add "Verified" badge on consent screen for verified clients
- [ ] Show warning for unverified clients
- [ ] Add client management page for users (revoke access)

**Timeline**: 3 weeks
**Status**: 🟠 Not started

---

### 3.3 P1-3: Multi-Factor Authentication (MFA) for Admin

**Issue**: Admin accounts protected by password only
**Risk**: 🟠 **High** - Admin account takeover via password compromise
**Current State**: Admin JWT with password authentication

**Implementation** (TOTP):

```rust
// File: src/admin/mfa.rs

use totp_lite::{totp, totp_custom};
use rand::Rng;
use base32;

pub struct MfaManager {
    // ...
}

impl MfaManager {
    /// Generate MFA secret for admin
    pub fn generate_secret() -> String {
        let secret: Vec<u8> = (0..20).map(|_| rand::thread_rng().gen()).collect();
        base32::encode(base32::Alphabet::RFC4648 { padding: false }, &secret)
    }

    /// Generate QR code URL for setup
    pub fn generate_qr_url(admin_did: &str, secret: &str, issuer: &str) -> String {
        format!(
            "otpauth://totp/{}:{}?secret={}&issuer={}",
            issuer, admin_did, secret, issuer
        )
    }

    /// Verify TOTP code
    pub fn verify_totp(secret: &str, code: &str, time: u64) -> bool {
        let secret_bytes = base32::decode(base32::Alphabet::RFC4648 { padding: false }, secret)
            .unwrap_or_default();

        let expected = totp_custom::<totp_lite::Sha1>(30, 6, &secret_bytes, time);

        expected == code
    }
}

// Admin login flow with MFA
pub async fn admin_login_mfa(
    ctx: State<AppContext>,
    body: Json<AdminLoginBody>,
) -> PdsResult<Json<AdminLoginResponse>> {
    // 1. Verify password (existing)
    let admin = ctx.admin_manager.verify_password(&body.did, &body.password).await?;

    // 2. Check if MFA enabled
    if let Some(mfa_secret) = admin.mfa_secret {
        // 3. Require TOTP code
        let code = body.totp_code.ok_or_else(|| {
            PdsError::Authentication("MFA code required")
        })?;

        // 4. Verify TOTP
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if !MfaManager::verify_totp(&mfa_secret, &code, time) {
            return Err(PdsError::Authentication("Invalid MFA code"));
        }
    }

    // 5. Issue admin JWT
    let jwt = create_admin_jwt(&admin.did, &ctx.config.jwt_secret)?;

    Ok(Json(AdminLoginResponse { jwt }))
}
```

**Timeline**: 2 weeks
**Status**: 🟠 Not started

---

### Phase 8 Summary

| Task | Priority | Timeline | Status |
|------|----------|----------|--------|
| **P1-1: Circuit Breaker** | 🟠 High | 2 weeks | Not started |
| **P1-2: Client Verification** | 🔴 Critical | 3 weeks | Not started |
| **P1-3: Admin MFA** | 🟠 High | 2 weeks | Not started |
| **P1-4: Enhanced Monitoring** | 🟡 Medium | 2 weeks | Not started |
| **P1-5: Incident Response Plan** | 🟡 Medium | 1 week | Not started |

**Total Duration**: 8-10 weeks

---

## 4. Phase 9: Continuous Improvement

**Duration**: Ongoing (post-production)
**Goal**: Maintain and improve security posture

### 4.1 Security Monitoring

**Metrics to Track**:

```rust
// Prometheus metrics
lazy_static! {
    pub static ref OAUTH_TOKEN_ISSUED_TOTAL: IntCounterVec = register_int_counter_vec!(
        "oauth_token_issued_total",
        "Total OAuth tokens issued",
        &["client_id", "scope"]
    ).unwrap();

    pub static ref AUTH_FAILURES_TOTAL: IntCounterVec = register_int_counter_vec!(
        "auth_failures_total",
        "Total authentication failures",
        &["reason"]
    ).unwrap();

    pub static ref ADMIN_ACTIONS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "admin_actions_total",
        "Total admin actions performed",
        &["admin_did", "action"]
    ).unwrap();

    pub static ref CROSS_PDS_REQUESTS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "cross_pds_requests_total",
        "Total cross-PDS requests",
        &["pds_did", "status"]
    ).unwrap();

    pub static ref CIRCUIT_BREAKER_STATE: IntGaugeVec = register_int_gauge_vec!(
        "circuit_breaker_state",
        "Circuit breaker state (0=closed, 1=open, 2=half-open)",
        &["pds_did"]
    ).unwrap();
}
```

**Alerting Rules** (Prometheus):

```yaml
groups:
  - name: security_alerts
    rules:
      # Alert on high authentication failure rate
      - alert: HighAuthFailureRate
        expr: rate(auth_failures_total[5m]) > 10
        for: 5m
        annotations:
          summary: "High authentication failure rate detected"
          description: "{{ $value }} auth failures/sec"

      # Alert on admin actions
      - alert: AdminActionPerformed
        expr: increase(admin_actions_total[1m]) > 0
        annotations:
          summary: "Admin action performed"
          description: "Admin {{ $labels.admin_did }} performed {{ $labels.action }}"

      # Alert on circuit breaker opening
      - alert: CircuitBreakerOpened
        expr: circuit_breaker_state == 1
        for: 1m
        annotations:
          summary: "Circuit breaker opened for {{ $labels.pds_did }}"
```

---

### 4.2 Bug Bounty Program

**Scope**:
- OAuth 2.1 implementation
- DPoP token binding
- Cross-PDS authentication
- Admin authorization
- API endpoints

**Rewards**:
- **Critical**: $5,000 - $10,000
- **High**: $2,000 - $5,000
- **Medium**: $500 - $2,000
- **Low**: $100 - $500

**Platform**: HackerOne or Bugcrowd

---

### 4.3 Regular Security Audits

**Schedule**:
- **Annual comprehensive audit**: Full codebase review
- **Quarterly penetration testing**: OAuth flows, API endpoints
- **Monthly dependency scanning**: `cargo audit`, Dependabot

---

## 5. Security Audit Checklist

**Pre-Production Audit Checklist**:

### Authentication & Authorization
- [ ] ✅ OAuth 2.1 with PKCE implemented
- [ ] ✅ DPoP token binding implemented
- [ ] ⚠️ Access tokens hashed in database (P0-1)
- [ ] ✅ Refresh token rotation with replay detection
- [ ] ✅ Service auth JWT (<60 seconds)
- [ ] ✅ Scope-based authorization (18+ scopes)
- [ ] ⚠️ Client domain verification (P1-2)

### Infrastructure
- [ ] ⚠️ Nonce store migrated to Redis (P0-2)
- [ ] ⚠️ Circuit breaker for federated calls (P1-1)
- [ ] ✅ Rate limiting (10x stricter cross-PDS)
- [ ] ⚠️ Admin MFA (P1-3)

### Monitoring & Response
- [ ] ✅ Prometheus metrics
- [ ] ⚠️ Security alerting (P1-4)
- [ ] ⚠️ Incident response plan (P1-5)
- [ ] ⚠️ Bug bounty program (P3)

### Compliance
- [ ] ⚠️ Security audit completed (P0-3)
- [ ] ⚠️ Penetration testing passed (P0-3)
- [ ] ✅ Security documentation complete
- [ ] ⚠️ Data protection compliance (GDPR, CCPA)

---

## 6. Deployment Security

### 6.1 Infrastructure Security

**Required**:
- [ ] TLS 1.3 for all connections
- [ ] Database encryption at rest
- [ ] Secrets management (HashiCorp Vault or AWS Secrets Manager)
- [ ] VPC with private subnets
- [ ] WAF (Web Application Firewall)
- [ ] DDoS protection (Cloudflare, AWS Shield)

### 6.2 Operational Security

**Required**:
- [ ] SSH key-based authentication (no passwords)
- [ ] Bastion host for database access
- [ ] Audit logging enabled (all database operations)
- [ ] Automated security patching
- [ ] Backup encryption
- [ ] Disaster recovery plan

---

## Conclusion

**Aurora Locus PDS Security Roadmap Summary**:

**Phase 7 (Critical)** - 2-3 months:
- ⚠️ Token hashing (P0-1)
- ⚠️ Redis nonce store (P0-2)
- ⚠️ Security audit (P0-3)
- ⚠️ DID key extraction fix (P0-4)

**Phase 8 (Hardening)** - 2-3 months:
- Circuit breaker (P1-1)
- Client verification (P1-2)
- Admin MFA (P1-3)
- Enhanced monitoring (P1-4)

**Phase 9 (Continuous)** - Ongoing:
- Bug bounty program
- Regular security audits
- Dependency scanning
- Incident response

**Estimated Total Timeline**: 6-9 months to production-ready
**Estimated Total Cost**: $75,000 - $150,000 (audits + security tools)

---

**Last Updated**: 2025-11-05
**Phase**: 6.8 - Security Hardening Roadmap
**Status**: Complete
**Next**: Begin Phase 7 (Critical Fixes)
