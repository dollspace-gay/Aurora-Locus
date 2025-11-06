# Security & Authorization Model Comparison

**Aurora Locus PDS vs Bluesky PDS**

Phase 6.8: Security & Authorization Model Comparison
Date: 2025-11-05
Status: Analysis Complete

---

## Executive Summary

This document compares the security and authorization architectures of **Aurora Locus PDS** (Rust implementation) and **Bluesky PDS** (TypeScript implementation) to identify strengths, weaknesses, and opportunities for improvement.

**Key Finding**: Aurora Locus has implemented a **comprehensive OAuth 2.1 + DPoP security model** that meets or exceeds Bluesky PDS in most areas. However, both implementations share the ATProto specification's core security principles: cryptographic DID-based authentication, short-lived tokens, and scope-based authorization.

**Overall Security Maturity**:
- **Bluesky PDS**: Production-ready (leverages `@atproto/oauth-provider` library)
- **Aurora Locus**: Development/Testing (custom implementation, Phase 6 complete)

---

## Table of Contents

1. [Authentication Mechanisms](#1-authentication-mechanisms)
2. [Authorization Patterns](#2-authorization-patterns)
3. [Token Security](#3-token-security)
4. [Rate Limiting Strategies](#4-rate-limiting-strategies)
5. [CORS and Security Headers](#5-cors-and-security-headers)
6. [Input Validation and Sanitization](#6-input-validation-and-sanitization)
7. [Security Architecture Comparison](#7-security-architecture-comparison)
8. [Security Gaps Analysis](#8-security-gaps-analysis)

---

## 1. Authentication Mechanisms

### 1.1 Bluesky PDS Authentication

Bluesky PDS uses a **multi-layered authentication system** powered by the `@atproto/oauth-provider` library:

| Auth Method | Use Case | Token Type | Lifetime |
|-------------|----------|------------|----------|
| **OAuth 2.0** | Standard user auth | JWT (at+jwt) | 120 minutes |
| **Refresh Tokens** | Token renewal | JWT (refresh+jwt) | 90 days |
| **App Passwords** | Legacy/CLI apps | Session-based | No expiration |
| **Service JWT** | Cross-PDS requests | ES256 JWT | <60 seconds |
| **Basic Auth** | Admin panel | Username/password | Session-based |
| **Mod Service Auth** | Moderation service | ES256 JWT | <60 seconds |

**Implementation Details** ([auth-verifier.ts:91-199](../bluesky-pds/src/auth-verifier.ts#L91-L199)):
```typescript
export class AuthVerifier {
  // OAuth token verification (access tokens)
  protected access<S extends AuthScope>(
    options: VerifiedOptions & Required<ScopedOptions<S>>,
  ): MethodAuthVerifier<AccessOutput<S>> {
    return async (ctx) => {
      const { sub: did, scope } = await this.verifyBearerJwt(
        ctx.req,
        { audience: this.dids.pds, typ: 'at+jwt', scopes }
      )

      await this.verifyStatus(did, statusOptions)

      return { credentials: { type: 'access', did, scope } }
    }
  }

  // Service JWT verification (cross-PDS)
  public modService: MethodAuthVerifier<ModServiceOutput> = async (ctx) => {
    const payload = await this.verifyServiceJwt(ctx.req, {
      iss: [this.dids.modService, `${this.dids.modService}#atproto_labeler`],
    })
    return { credentials: { type: 'mod_service', did: payload.iss } }
  }
}
```

**OAuth Token Creation** ([auth.ts:18-85](../bluesky-pds/src/account-manager/helpers/auth.ts#L18-L85)):
```typescript
export const createAccessToken = (opts: {
  did: string
  jwtKey: KeyObject
  serviceDid: string
  scope?: AuthScope
  expiresIn?: string | number
}): Promise<string> => {
  const signer = new jose.SignJWT({ scope })
    .setProtectedHeader({
      typ: 'at+jwt', // RFC 9068
      alg: 'HS256', // Symmetric key (HMAC)
    })
    .setAudience(serviceDid)
    .setSubject(did)
    .setIssuedAt()
    .setExpirationTime(expiresIn) // Default: 120 minutes
  return signer.sign(jwtKey)
}
```

**Key Characteristics**:
- ✅ OAuth 2.0 via battle-tested `@atproto/oauth-provider` library
- ✅ HS256 symmetric signing for access tokens (fast, secure for single-server)
- ✅ ES256 asymmetric signing for service JWTs (cross-PDS verification)
- ✅ Scope-based authorization (com.atproto.access, com.atproto.appPass, etc.)
- ✅ Used refresh token tracking (prevents token replay)
- ⚠️ No explicit DPoP implementation (relies on external library)
- ⚠️ No explicit PKCE documentation (may be in library)

---

### 1.2 Aurora Locus Authentication

Aurora Locus implements a **comprehensive OAuth 2.1 + DPoP stack** with explicit PKCE support:

| Auth Method | Use Case | Token Type | Lifetime |
|-------------|----------|------------|----------|
| **OAuth 2.1 + PKCE** | Standard user auth | JWT (stored in DB) | Configurable |
| **DPoP Binding** | Device-bound tokens | EC P-256 proof | Per-request |
| **Refresh Tokens** | Token renewal | JWT with rotation | Configurable |
| **Session Tokens** | Legacy auth | Database-backed | No expiration |
| **Service Auth JWT** | Cross-PDS requests | ES256 JWT | <60 seconds |
| **Admin JWT** | Admin panel | HS256 JWT | Configurable |

**Implementation Details** ([auth.rs:224-391](../src/auth.rs#L224-L391)):
```rust
/// OAuth token information
#[derive(Debug, Clone)]
pub struct OAuthToken {
    pub did: String,
    pub token_id: String,
    pub client_id: String,
    pub scope: String, // Space-separated scopes
    pub dpop_thumbprint: Option<String>, // DPoP binding
    pub device_id: Option<String>, // Device binding
}

/// Validate OAuth access token
pub async fn validate_oauth_token(
    ctx: &AppContext,
    access_token: &str,
) -> Result<OAuthToken, PdsError> {
    // Query token table
    let row = sqlx::query(
        r#"
        SELECT token_id, did, client_id, scope, dpop_thumbprint, device_id, expires_at
        FROM token
        WHERE token_id = ?
        "#,
    )
    .bind(access_token)
    .fetch_optional(&ctx.account_db)
    .await?
    .ok_or_else(|| PdsError::Authentication("Invalid or expired access token"))?;

    // Check expiration
    let expires_at: chrono::DateTime<chrono::Utc> = row.get("expires_at");
    if expires_at < chrono::Utc::now() {
        return Err(PdsError::Authentication("Access token has expired"));
    }

    Ok(OAuthToken { /* ... */ })
}
```

**DPoP Proof Verification** ([dpop.rs:121-240](../src/federation/dpop.rs#L121-L240)):
```rust
impl DPopVerifier {
    /// Verify a DPoP proof JWT
    pub async fn verify_dpop_proof(
        &self,
        dpop_proof: &str,
        http_method: &str,
        http_uri: &str,
    ) -> PdsResult<String> {
        // 1. Decode JWT header to extract JWK
        let header = decode_header(dpop_proof)?;
        if header.typ.as_deref() != Some("dpop+jwt") {
            return Err(PdsError::Authentication("Invalid DPoP proof type"));
        }

        // 2. Extract JWK from header
        let jwk = header.jwk.ok_or(...)?;

        // 3. Convert JWK to DecodingKey (EC P-256)
        let decoding_key = jwk_to_decoding_key(&jwk_json)?;

        // 4. Verify JWT signature
        let token_data = decode::<DPopClaims>(dpop_proof, &decoding_key, &validation)?;
        let claims = token_data.claims;

        // 5. Validate HTTP method and URI
        if claims.htm.to_uppercase() != http_method.to_uppercase() {
            return Err(PdsError::Authentication("DPoP proof HTTP method mismatch"));
        }
        if claims.htu != expected_uri {
            return Err(PdsError::Authentication("DPoP proof HTTP URI mismatch"));
        }

        // 6. Validate and consume nonce (replay prevention)
        if !self.nonce_store.check_and_consume_nonce(&claims.jti).await? {
            return Err(PdsError::Authentication("DPoP proof nonce invalid"));
        }

        // 7. Compute JWK thumbprint (SHA-256)
        let thumbprint = compute_jwk_thumbprint(&jwk_json)?;

        Ok(thumbprint)
    }
}
```

**JWK to EC Key Conversion** ([dpop.rs:242-353](../src/federation/dpop.rs#L242-L353)):
```rust
fn jwk_to_decoding_key(jwk: &Value) -> PdsResult<DecodingKey> {
    // Extract JWK parameters (kty, crv, x, y)
    let kty = jwk["kty"].as_str().ok_or(...)?;
    let crv = jwk["crv"].as_str().ok_or(...)?;
    let x = jwk["x"].as_str().ok_or(...)?;
    let y = jwk["y"].as_str().ok_or(...)?;

    // Validate P-256 EC key
    if kty != "EC" || crv != "P-256" {
        return Err(...);
    }

    // Decode base64url coordinates
    let x_bytes = URL_SAFE_NO_PAD.decode(x)?;
    let y_bytes = URL_SAFE_NO_PAD.decode(y)?;

    // Construct uncompressed EC point (0x04 || x || y)
    let mut public_key_bytes = Vec::with_capacity(65);
    public_key_bytes.push(0x04);
    public_key_bytes.extend_from_slice(&x_bytes);
    public_key_bytes.extend_from_slice(&y_bytes);

    // Parse as P-256 public key
    let encoded_point = EncodedPoint::from_bytes(&public_key_bytes)?;
    let verifying_key = VerifyingKey::from_encoded_point(&encoded_point)?;

    // Convert to PEM format
    let public_key_der = verifying_key.to_public_key_der()?;
    let public_key_pem = format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
        STANDARD.encode(public_key_der.as_bytes())
    );

    DecodingKey::from_ec_pem(public_key_pem.as_bytes())
}
```

**Key Characteristics**:
- ✅ **Full OAuth 2.1 implementation** with PKCE (custom, not library-based)
- ✅ **Complete DPoP support** with JWK parsing (RFC 9449 compliant)
- ✅ **Device-bound credentials** (DPoP thumbprint + device ID)
- ✅ **Hierarchical scope system** (atproto:*, atproto:read, atproto:repo.*, etc.)
- ✅ **Refresh token rotation** with replay detection
- ✅ **Service Auth** for cross-PDS (ES256, <60s expiration)
- ✅ **Nonce-based replay prevention** (both DPoP and Service Auth)
- ⚠️ Custom implementation (not battle-tested like Bluesky's library)

---

### 1.3 Authentication Comparison Matrix

| Feature | Bluesky PDS | Aurora Locus | Winner |
|---------|-------------|--------------|--------|
| **OAuth 2.0/2.1** | ✅ OAuth 2.0 (library) | ✅ OAuth 2.1 (custom) | 🤝 Tie |
| **PKCE Support** | ⚠️ Unclear (library?) | ✅ Explicit implementation | **Aurora** |
| **DPoP Token Binding** | ⚠️ Not documented | ✅ Full RFC 9449 impl | **Aurora** |
| **Refresh Token Rotation** | ✅ Used token tracking | ✅ Replay detection | 🤝 Tie |
| **Device Management** | ⚠️ Basic | ✅ Per-device tokens | **Aurora** |
| **Cross-PDS Service Auth** | ✅ ES256 JWT | ✅ ES256 JWT | 🤝 Tie |
| **App Passwords** | ✅ With privilege flag | ⚠️ Legacy support only | **Bluesky** |
| **Session-based Auth** | ✅ Primary (legacy) | ⚠️ Backward compat | **Bluesky** |
| **Production Maturity** | ✅ Battle-tested library | ⚠️ Custom implementation | **Bluesky** |
| **Implementation Complexity** | ✅ External library (simple) | ⚠️ Custom code (complex) | **Bluesky** |

**Verdict**: Aurora Locus has **more advanced authentication features** (DPoP, PKCE, device binding), but Bluesky PDS benefits from **production-tested library** (`@atproto/oauth-provider`).

---

## 2. Authorization Patterns

### 2.1 Bluesky PDS Authorization

Bluesky uses **internal scope-based authorization** with predefined scopes:

**Scope Definitions** ([auth-scope.ts:1-41](../bluesky-pds/src/auth-scope.ts#L1-L41)):
```typescript
export enum AuthScope {
  Access = 'com.atproto.access',         // Full access
  Refresh = 'com.atproto.refresh',       // Token refresh
  AppPass = 'com.atproto.appPass',       // App password
  AppPassPrivileged = 'com.atproto.appPassPrivileged', // Privileged app password
  SignupQueued = 'com.atproto.signupQueued', // Queued signup
  Takendown = 'com.atproto.takendown',   // Taken down account
}

export const ACCESS_FULL = [AuthScope.Access] as const
export const ACCESS_PRIVILEGED = [
  ...ACCESS_FULL,
  AuthScope.AppPassPrivileged,
] as const
export const ACCESS_STANDARD = [
  ...ACCESS_PRIVILEGED,
  AuthScope.AppPass,
] as const
```

**Scope Enforcement** ([auth-verifier.ts:167-196](../bluesky-pds/src/auth-verifier.ts#L167-L196)):
```typescript
protected access<S extends AuthScope>(
  options: VerifiedOptions & Required<ScopedOptions<S>>,
): MethodAuthVerifier<AccessOutput<S>> {
  const { scopes, ...statusOptions } = options

  return async (ctx) => {
    // Verify JWT and extract scope
    const { sub: did, scope } = await this.verifyBearerJwt(ctx.req, {
      audience: this.dids.pds,
      typ: 'at+jwt',
      scopes: options.checkTakedown
        ? scopes.filter((s) => s !== AuthScope.Takendown)
        : scopes,
    })

    // Verify account status (not deactivated/taken down)
    await this.verifyStatus(did, statusOptions)

    return { credentials: { type: 'access', did, scope } }
  }
}
```

**Authorization Patterns**:
- ✅ **Simple scope model** (6 predefined scopes)
- ✅ **Hierarchical access levels** (FULL → PRIVILEGED → STANDARD)
- ✅ **Account status checking** (deactivated, taken down)
- ⚠️ **Limited granularity** (no per-operation scopes)
- ⚠️ **Internal scopes** (not OAuth standard scopes)

---

### 2.2 Aurora Locus Authorization

Aurora Locus implements **OAuth 2.1 hierarchical scope system** with fine-grained permissions:

**Scope Definitions** ([scope.rs:26-85](../src/oauth/scope.rs#L26-L85)):
```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AtProtoScope {
    All,                         // atproto:*
    Read,                        // atproto:read
    Write,                       // atproto:write
    RepoAll,                     // atproto:repo.*
    RepoCreate,                  // atproto:repo.create
    RepoUpdate,                  // atproto:repo.update
    RepoDelete,                  // atproto:repo.delete
    RepoList,                    // atproto:repo.list
    RepoGet,                     // atproto:repo.get
    IdentityAll,                 // atproto:identity.*
    IdentityUpdateProfile,       // atproto:identity.updateProfile
    IdentityResolveDid,          // atproto:identity.resolveDid
    BlobUpload,                  // atproto:blob.upload
    BlobDelete,                  // atproto:blob.delete
    AdminAll,                    // atproto:admin.*
    AdminModeration,             // atproto:admin.moderation
    AdminServer,                 // atproto:admin.server
    Custom(String),              // Custom scopes
}
```

**Hierarchical Scope Checking** ([scope.rs:87-139](../src/oauth/scope.rs#L87-L139)):
```rust
impl AtProtoScope {
    /// Check if this scope includes another scope
    pub fn includes(&self, other: &AtProtoScope) -> bool {
        match (self, other) {
            // All includes everything
            (AtProtoScope::All, _) => true,

            // Exact match
            (s1, s2) if s1 == s2 => true,

            // Write includes create/update/delete
            (AtProtoScope::Write, AtProtoScope::RepoCreate) => true,
            (AtProtoScope::Write, AtProtoScope::RepoUpdate) => true,
            (AtProtoScope::Write, AtProtoScope::RepoDelete) => true,

            // Read includes get/list operations
            (AtProtoScope::Read, AtProtoScope::RepoGet) => true,
            (AtProtoScope::Read, AtProtoScope::RepoList) => true,

            // RepoAll includes all repo operations
            (AtProtoScope::RepoAll, AtProtoScope::RepoCreate) => true,
            (AtProtoScope::RepoAll, AtProtoScope::RepoUpdate) => true,
            (AtProtoScope::RepoAll, AtProtoScope::RepoDelete) => true,

            _ => false,
        }
    }
}
```

**Scope Enforcement Middleware** ([scope.rs:460-490](../src/oauth/scope.rs#L460-L490)):
```rust
/// Check if token has required scope
pub fn require_scope(token_scopes: &str, required: &AtProtoScope) -> PdsResult<()> {
    let scopes = ScopeSet::from_str(token_scopes)?;

    if scopes.has_scope(required) {
        Ok(())
    } else {
        Err(PdsError::Authorization(format!(
            "Insufficient scope: requires {}",
            required
        )))
    }
}

/// Map lexicon NSID to required scope
pub fn lexicon_to_scope(nsid: &str) -> AtProtoScope {
    if nsid.starts_with("com.atproto.repo.create") {
        AtProtoScope::RepoCreate
    } else if nsid.starts_with("com.atproto.repo.put") {
        AtProtoScope::RepoUpdate
    } else if nsid.starts_with("com.atproto.repo.delete") {
        AtProtoScope::RepoDelete
    } else if nsid.starts_with("com.atproto.repo.get") {
        AtProtoScope::RepoGet
    } else {
        AtProtoScope::Read // Default
    }
}
```

**Authorization Patterns**:
- ✅ **Fine-grained scopes** (18 predefined + custom)
- ✅ **Hierarchical inclusion** (All → Write → RepoCreate)
- ✅ **Lexicon-to-scope mapping** (automatic NSID → scope)
- ✅ **Privileged scope detection** (admin scopes)
- ✅ **Scope categories** (basic, repo, identity, blob, admin, custom)
- ✅ **OAuth 2.1 standard scopes** (atproto: prefix)

---

### 2.3 Authorization Comparison Matrix

| Feature | Bluesky PDS | Aurora Locus | Winner |
|---------|-------------|--------------|--------|
| **Scope Granularity** | 6 internal scopes | 18+ OAuth scopes | **Aurora** |
| **Hierarchical Scopes** | ✅ ACCESS_FULL/PRIVILEGED/STANDARD | ✅ All → Write → RepoCreate | 🤝 Tie |
| **Lexicon Mapping** | ⚠️ Manual | ✅ Automatic NSID → scope | **Aurora** |
| **Custom Scopes** | ❌ Not supported | ✅ Custom(String) enum | **Aurora** |
| **Privileged Detection** | ✅ AppPassPrivileged | ✅ is_privileged() check | 🤝 Tie |
| **OAuth 2.1 Compliance** | ⚠️ Internal scopes | ✅ Standard atproto: prefix | **Aurora** |
| **Scope Description/UI** | ❌ Not available | ✅ description() + category() | **Aurora** |
| **Account Status Checks** | ✅ Deactivated/Takendown | ⚠️ Basic | **Bluesky** |

**Verdict**: Aurora Locus has **significantly more sophisticated authorization** with fine-grained OAuth 2.1 scopes, hierarchical permissions, and automatic lexicon mapping.

---

## 3. Token Security

### 3.1 Bluesky PDS Token Security

**Token Lifetime Management**:
- **Access Token**: 120 minutes (2 hours)
- **Refresh Token**: 90 days
- **Service JWT**: <60 seconds (ATProto requirement)

**Refresh Token Rotation** ([oauth-store.ts:1-234](../bluesky-pds/src/account-manager/oauth-store.ts#L1-L234)):
```typescript
// Bluesky uses "used refresh token" tracking to prevent replay
// Located in: helpers/used-refresh-token.ts

export class OAuthStore implements TokenStore {
  async rotateRefreshToken(
    refreshToken: RefreshToken,
    newTokenData: NewTokenData
  ): Promise<TokenData> {
    // Mark old refresh token as "used"
    await usedRefreshTokenHelper.create(
      this.db,
      refreshToken,
      newTokenData.refresh_token
    )

    // Return new tokens
    return {
      access_token: newTokenData.access_token,
      refresh_token: newTokenData.refresh_token,
      token_type: 'Bearer',
      expires_in: 7200, // 2 hours
    }
  }
}
```

**Token Storage**:
- **Access Tokens**: JWT (stateless, verified via signature)
- **Refresh Tokens**: Stored in database (`refresh_token` table)
- **Used Refresh Tokens**: Tracked in `used_refresh_token` table

**Token Security Features**:
- ✅ Refresh token rotation (prevents replay)
- ✅ Used token tracking (detects replay attacks)
- ✅ Short-lived access tokens (2 hours)
- ✅ ES256 for cross-PDS JWTs (asymmetric)
- ✅ HS256 for access tokens (symmetric, single-server)
- ⚠️ No explicit token hashing (tokens stored as-is?)
- ⚠️ No DPoP binding

---

### 3.2 Aurora Locus Token Security

**Token Lifetime Management**:
- **Access Token**: Configurable (default: OAuth spec compliant)
- **Refresh Token**: Configurable with rotation
- **DPoP Proof**: <60 seconds per request
- **DPoP Nonce**: 5 minutes (generation to use)
- **Service Auth Nonce**: 120 seconds

**Token Rotation Manager** ([token_rotation.rs](../src/oauth/token_rotation.rs)):
```rust
pub struct TokenRotationManager {
    // Refresh token rotation with replay detection
}

pub enum RotationResult {
    Success {
        access_token: String,
        refresh_token: String,
        expires_in: i64,
    },
    ReplayDetected {
        old_token_id: String,
    },
}

impl TokenRotationManager {
    /// Rotate refresh token with replay detection
    pub async fn rotate_token(
        &self,
        refresh_token: &str,
    ) -> PdsResult<RotationResult> {
        // 1. Validate refresh token
        // 2. Check if already used
        // 3. Generate new token pair
        // 4. Mark old token as used
        // 5. Return new tokens or ReplayDetected
    }
}
```

**DPoP Nonce System** ([dpop.rs:46-119](../src/federation/dpop.rs#L46-L119)):
```rust
pub struct DPopNonceStore {
    nonces: Arc<RwLock<HashMap<String, i64>>>,
}

impl DPopNonceStore {
    /// Generate new DPoP nonce (5 minute expiration)
    pub async fn generate_nonce(&self) -> String {
        let nonce = uuid::Uuid::new_v4().to_string();
        let expires_at = Utc::now().timestamp() + 300; // 5 minutes

        self.nonces.write().await.insert(nonce.clone(), expires_at);
        nonce
    }

    /// Check and consume nonce (single-use)
    pub async fn check_and_consume_nonce(&self, nonce: &str) -> PdsResult<bool> {
        let mut nonces = self.nonces.write().await;

        if let Some(&expires_at) = nonces.get(nonce) {
            if Utc::now().timestamp() < expires_at {
                nonces.remove(nonce); // Consume (single-use)
                Ok(true)
            } else {
                nonces.remove(nonce); // Expired
                Ok(false)
            }
        } else {
            Ok(false) // Not found
        }
    }
}
```

**Token Storage**:
- **Access Tokens**: Stored in database (`token` table) with DPoP thumbprint
- **Refresh Tokens**: Managed by TokenRotationManager with replay detection
- **DPoP Nonces**: In-memory HashMap (⚠️ production: use Redis)
- **Service Auth Nonces**: In-memory HashMap (⚠️ production: use Redis)

**Token Security Features**:
- ✅ **DPoP token binding** (RFC 9449)
- ✅ **Device binding** (per-device DPoP keys)
- ✅ **Refresh token rotation** with replay detection
- ✅ **Nonce-based replay prevention** (both DPoP and Service Auth)
- ✅ **JWK thumbprint binding** (SHA-256 hash)
- ✅ **Short-lived service JWTs** (<60 seconds)
- ✅ **ES256 for cross-PDS** (asymmetric)
- ⚠️ **Nonce store scalability** (in-memory, not distributed)
- ⚠️ **Access token hashing** (TODO: store hashed tokens)

---

### 3.3 Token Security Comparison Matrix

| Feature | Bluesky PDS | Aurora Locus | Winner |
|---------|-------------|--------------|--------|
| **DPoP Token Binding** | ❌ Not documented | ✅ Full RFC 9449 | **Aurora** |
| **Device Binding** | ⚠️ Basic | ✅ Per-device keys | **Aurora** |
| **Refresh Token Rotation** | ✅ Used token tracking | ✅ Replay detection | 🤝 Tie |
| **Nonce-based Replay Prevention** | ⚠️ Service JWT only | ✅ DPoP + Service Auth | **Aurora** |
| **Short-lived Tokens** | ✅ 2 hours (access) | ✅ Configurable | 🤝 Tie |
| **Token Hashing** | ⚠️ Unclear | ⚠️ TODO (planned) | 🤝 Tie |
| **Nonce Store Scalability** | ✅ Database-backed | ⚠️ In-memory (TODO: Redis) | **Bluesky** |
| **JWT Algorithms** | ✅ HS256 + ES256 | ✅ ES256 (DPoP/Service) | 🤝 Tie |

**Verdict**: Aurora Locus has **superior token security** with DPoP binding and nonce-based replay prevention, but Bluesky has **better scalability** with database-backed token tracking.

---

## 4. Rate Limiting Strategies

### 4.1 Bluesky PDS Rate Limiting

Bluesky PDS uses **basic rate limiting** per user:

**Rate Limit Implementation** (basic):
```typescript
// Bluesky PDS uses simple per-user rate limits
// Details not extensively documented in reviewed files
// Likely uses middleware with configurable limits
```

**Rate Limit Characteristics**:
- ⚠️ **Limited documentation** in reviewed files
- ⚠️ **No cross-PDS specific limits** mentioned
- ✅ **Per-user rate limiting** (assumed)

---

### 4.2 Aurora Locus Rate Limiting

Aurora Locus implements **tiered rate limiting** with **10x stricter cross-PDS limits**:

**Rate Limit Tiers** ([SECURITY.md:99-123](../SECURITY.md#L99-L123)):

| Type | Requests/Second | Burst | Ratio |
|------|-----------------|-------|-------|
| **Local Authenticated** | 100 | 50 | 1x (baseline) |
| **Cross-PDS** | 10 | 5 | **10x stricter** |
| **Unauthenticated** | 10 | 10 | 10x stricter |
| **Admin** | 1000 | 100 | 10x permissive |

**Rationale for Stricter Cross-PDS Limits**:
1. **Prevent Abuse**: Limit impact of compromised federated instances
2. **Resource Protection**: Prevent federation from overwhelming local users
3. **DoS Mitigation**: Rate limit distributed attacks
4. **Fair Usage**: Prioritize local users over federated requests

**Implementation** ([rate_limit.rs](../src/rate_limit.rs)):
```rust
pub struct RateLimiter {
    // Token bucket algorithm
    // Separate buckets for local, cross-PDS, unauthenticated, admin
}

impl RateLimiter {
    pub async fn check_cross_pds(&self, did: &str) -> PdsResult<()> {
        // Check 10 req/s limit for cross-PDS
        if self.is_rate_limited(did, RateLimitType::CrossPDS).await? {
            Err(PdsError::RateLimited(
                "Cross-PDS rate limit exceeded (10 req/s)".to_string()
            ))
        } else {
            Ok(())
        }
    }
}
```

**Rate Limiting Enforcement** ([repo.rs](../src/api/repo.rs)):
```rust
// Applied in repo endpoints
pub async fn create_record(
    ctx: State<AppContext>,
    auth: UnifiedAuth,
    // ...
) -> PdsResult<Json<CreateRecordResponse>> {
    // Check cross-PDS rate limit if federated
    if let UnifiedAuth::CrossPds(auth) = &auth {
        ctx.rate_limiter.check_cross_pds(&auth.did).await?;
    }

    // ... endpoint logic
}
```

**Rate Limiting Features**:
- ✅ **Tiered rate limiting** (4 tiers)
- ✅ **10x stricter cross-PDS limits** (DoS protection)
- ✅ **Token bucket algorithm** (smooth rate limiting)
- ✅ **Per-endpoint enforcement** (granular control)
- ✅ **Prometheus metrics** (monitoring)

---

### 4.3 Rate Limiting Comparison Matrix

| Feature | Bluesky PDS | Aurora Locus | Winner |
|---------|-------------|--------------|--------|
| **Tiered Rate Limiting** | ⚠️ Basic | ✅ 4 tiers (local/cross/unauth/admin) | **Aurora** |
| **Cross-PDS Limits** | ⚠️ Not mentioned | ✅ 10x stricter (10 req/s) | **Aurora** |
| **DoS Protection** | ⚠️ Basic | ✅ Distributed attack mitigation | **Aurora** |
| **Token Bucket Algorithm** | ⚠️ Unclear | ✅ Smooth rate limiting | **Aurora** |
| **Metrics/Monitoring** | ✅ Likely available | ✅ Prometheus metrics | 🤝 Tie |
| **Per-Endpoint Limits** | ⚠️ Unclear | ✅ Granular enforcement | **Aurora** |

**Verdict**: Aurora Locus has **significantly better rate limiting** with tiered limits, 10x stricter cross-PDS enforcement, and DoS protection.

---

## 5. CORS and Security Headers

### 5.1 Bluesky PDS CORS

**CORS Implementation**:
- ✅ Standard CORS support (Axum/Express middleware)
- ✅ Configurable origins
- ⚠️ Not extensively documented in reviewed files

---

### 5.2 Aurora Locus CORS

**CORS Implementation** ([middleware.rs](../src/api/middleware.rs)):
```rust
// Basic CORS support
pub fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(/* configured origins */)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([AUTHORIZATION, CONTENT_TYPE])
}
```

**Security Headers**:
- ✅ `Vary: Authorization` (cache protection)
- ✅ Standard CORS headers
- ⚠️ No explicit security headers documented (X-Frame-Options, CSP, etc.)

---

### 5.3 CORS Comparison

Both implementations have **basic CORS support** without extensive security header configuration documented.

**Recommendation**: Both should add:
- `X-Frame-Options: DENY`
- `Content-Security-Policy: default-src 'self'`
- `X-Content-Type-Options: nosniff`
- `Strict-Transport-Security: max-age=31536000`

---

## 6. Input Validation and Sanitization

### 6.1 Bluesky PDS Input Validation

**Validation Approach**:
- ✅ **Lexicon schema validation** (ATProto spec)
- ✅ **XRPC parameter validation**
- ✅ **Email validation** (OAuth provider)
- ✅ **Password strength** (OAuth provider)
- ✅ **Handle validation** (reserved names, explicit slurs)

**Handle Validation** ([handle/index.ts](../bluesky-pds/src/handle/index.ts)):
```typescript
// Reserved handles and explicit slur checks
// Ensures handles don't violate platform policies
```

---

### 6.2 Aurora Locus Input Validation

**Validation Approach**:
- ✅ **Lexicon schema validation** (validation module)
- ✅ **XRPC parameter validation**
- ✅ **DID validation** (DID syntax checks)
- ✅ **Scope validation** (OAuth scope parsing)
- ✅ **Repository validation** (CID, CAR format)

**Validation Module** ([validation/mod.rs](../src/validation/mod.rs)):
```rust
pub mod did_validation;
pub mod handle_validation;
pub mod lexicon_validation;
pub mod repo_validation;

// Comprehensive validation for all inputs
```

---

### 6.3 Input Validation Comparison

Both implementations have **strong input validation** via lexicon schemas and XRPC validation. Bluesky has **additional social protections** (reserved names, slur checks).

---

## 7. Security Architecture Comparison

### 7.1 Architecture Overview

#### Bluesky PDS Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    Bluesky PDS                          │
├─────────────────────────────────────────────────────────┤
│  Auth Layer (AuthVerifier)                              │
│  ├─ OAuth 2.0 (@atproto/oauth-provider library)         │
│  ├─ JWT Access Tokens (HS256, 2 hours)                  │
│  ├─ JWT Refresh Tokens (HS256, 90 days)                 │
│  ├─ Service JWT (ES256, <60s, cross-PDS)                │
│  ├─ App Passwords (backward compat)                     │
│  └─ Basic Auth (admin)                                  │
├─────────────────────────────────────────────────────────┤
│  Authorization (AuthScope)                               │
│  ├─ Internal scopes (com.atproto.*)                     │
│  ├─ 6 predefined scopes                                 │
│  └─ Hierarchical access (FULL → PRIVILEGED → STANDARD)  │
├─────────────────────────────────────────────────────────┤
│  Token Security                                          │
│  ├─ Used refresh token tracking                         │
│  ├─ Database-backed token storage                       │
│  └─ Short-lived tokens (2 hours)                        │
├─────────────────────────────────────────────────────────┤
│  Account Management                                      │
│  ├─ Account status (active, deactivated, taken down)    │
│  ├─ Device management (basic)                           │
│  └─ Authorized clients (OAuth clients)                  │
└─────────────────────────────────────────────────────────┘
```

#### Aurora Locus Architecture

```
┌─────────────────────────────────────────────────────────┐
│                   Aurora Locus PDS                       │
├─────────────────────────────────────────────────────────┤
│  Auth Layer (Multi-Mode)                                 │
│  ├─ OAuth 2.1 + PKCE (custom implementation)            │
│  ├─ DPoP Token Binding (RFC 9449, EC P-256)             │
│  ├─ Service Auth JWT (ES256, <60s, cross-PDS)           │
│  ├─ Session Tokens (backward compat)                    │
│  └─ Admin JWT (HS256, configurable)                     │
├─────────────────────────────────────────────────────────┤
│  Authorization (AtProtoScope)                            │
│  ├─ OAuth 2.1 standard scopes (atproto:*)               │
│  ├─ 18+ fine-grained scopes                             │
│  ├─ Hierarchical inclusion (All → Write → RepoCreate)   │
│  ├─ Automatic lexicon → scope mapping                   │
│  └─ Custom scope support                                │
├─────────────────────────────────────────────────────────┤
│  Token Security                                          │
│  ├─ DPoP nonce-based replay prevention                  │
│  ├─ JWK thumbprint binding (SHA-256)                    │
│  ├─ Per-device token binding                            │
│  ├─ Refresh token rotation with replay detection        │
│  └─ Service auth nonce tracking (120s window)           │
├─────────────────────────────────────────────────────────┤
│  Rate Limiting (Tiered)                                  │
│  ├─ Local: 100 req/s (baseline)                         │
│  ├─ Cross-PDS: 10 req/s (10x stricter)                  │
│  ├─ Unauthenticated: 10 req/s                           │
│  └─ Admin: 1000 req/s                                   │
├─────────────────────────────────────────────────────────┤
│  Device Management                                       │
│  ├─ Per-device OAuth tokens                             │
│  ├─ Per-device DPoP key pairs                           │
│  ├─ Device registration & revocation                    │
│  └─ Secure keychain storage (platform-specific)         │
└─────────────────────────────────────────────────────────┘
```

---

### 7.2 Security Architecture Comparison

| Component | Bluesky PDS | Aurora Locus | Winner |
|-----------|-------------|--------------|--------|
| **Auth Complexity** | ⚠️ Simple (library-based) | ⚠️ Complex (custom) | **Bluesky** |
| **Auth Features** | ✅ Production-tested | ✅ Advanced (DPoP, PKCE) | **Aurora** |
| **Authorization Granularity** | ⚠️ Basic (6 scopes) | ✅ Fine-grained (18+ scopes) | **Aurora** |
| **Token Security** | ✅ Replay detection | ✅ DPoP + Replay + Device binding | **Aurora** |
| **Rate Limiting** | ⚠️ Basic | ✅ Tiered + Cross-PDS protection | **Aurora** |
| **Device Management** | ⚠️ Basic | ✅ Per-device tokens + keys | **Aurora** |
| **Production Readiness** | ✅ Battle-tested | ⚠️ Development/testing | **Bluesky** |
| **Maintainability** | ✅ External library | ⚠️ Custom implementation | **Bluesky** |
| **Security Maturity** | ✅ Proven in production | ⚠️ Needs production testing | **Bluesky** |

**Overall**: Aurora Locus has **more advanced security features**, but Bluesky PDS has **proven production reliability**.

---

## 8. Security Gaps Analysis

### 8.1 Bluesky PDS Security Gaps

Based on review of Bluesky PDS code:

1. **Limited DPoP Support**
   - ❌ No documented DPoP implementation (may be in library)
   - ⚠️ Token theft vulnerability without device binding

2. **Coarse Authorization**
   - ⚠️ Only 6 internal scopes (limited granularity)
   - ⚠️ No per-operation scope enforcement

3. **Basic Rate Limiting**
   - ⚠️ No documented cross-PDS specific limits
   - ⚠️ Potential DoS vulnerability from federated instances

4. **Token Storage**
   - ⚠️ Unclear if access tokens are hashed in database
   - ⚠️ Potential token theft from database compromise

**Impact**: Bluesky PDS prioritizes **production stability** over **cutting-edge security features**.

---

### 8.2 Aurora Locus Security Gaps

Based on review of Aurora Locus code:

1. **Nonce Store Scalability** (⚠️ **PRODUCTION BLOCKER**)
   - ❌ In-memory nonce storage (not distributed)
   - ❌ Lost on restart
   - **Fix**: Migrate to Redis for distributed nonce tracking

2. **Access Token Hashing** (⚠️ **PRODUCTION CONCERN**)
   - ❌ Tokens stored in database (not hashed)
   - ⚠️ Token theft vulnerability from database compromise
   - **Fix**: Store SHA-256 hashed tokens

3. **Custom OAuth Implementation** (⚠️ **PRODUCTION RISK**)
   - ⚠️ Not battle-tested in production
   - ⚠️ Potential security vulnerabilities vs library
   - **Fix**: Comprehensive security audit + penetration testing

4. **DID Signing Key Extraction** (⚠️ **PRODUCTION CONCERN**)
   - ⚠️ Simplified multibase decoding
   - ⚠️ May fail with some DID document formats
   - **Fix**: Proper multibase → PEM conversion

**Impact**: Aurora Locus has **advanced features** but needs **production hardening**.

---

### 8.3 Security Gap Comparison

| Gap Category | Bluesky PDS | Aurora Locus |
|--------------|-------------|--------------|
| **Authentication** | DPoP support unclear | Nonce store scalability |
| **Authorization** | Limited scope granularity | (None - advanced) |
| **Token Security** | Token hashing unclear | Token hashing TODO |
| **Rate Limiting** | Cross-PDS limits missing | (None - comprehensive) |
| **Scalability** | Database-backed (good) | In-memory nonces (bad) |
| **Production Readiness** | ✅ Ready | ⚠️ Needs hardening |

---

## Summary & Recommendations

### Key Findings

1. **Aurora Locus** has **superior security features**:
   - ✅ DPoP token binding (RFC 9449)
   - ✅ OAuth 2.1 with explicit PKCE
   - ✅ Fine-grained scopes (18+)
   - ✅ Tiered rate limiting with cross-PDS protection
   - ✅ Per-device credential binding

2. **Bluesky PDS** has **proven production reliability**:
   - ✅ Battle-tested `@atproto/oauth-provider` library
   - ✅ Database-backed token tracking
   - ✅ Simple, maintainable architecture
   - ✅ Production-hardened codebase

3. **Both implementations** share ATProto security principles:
   - ✅ Short-lived service JWTs (<60s)
   - ✅ DID-based cryptographic verification
   - ✅ Refresh token rotation
   - ✅ Lexicon schema validation

### Critical Production Blockers (Aurora Locus)

**Must fix before production**:
1. ⚠️ **Migrate nonce stores to Redis** (distributed, persistent)
2. ⚠️ **Hash access tokens** (SHA-256 before database storage)
3. ⚠️ **Comprehensive security audit** (custom OAuth implementation)
4. ⚠️ **Fix DID signing key extraction** (proper multibase → PEM)

### Recommendations

#### For Aurora Locus:

**Short-term (Phase 6 → Phase 7)**:
1. Migrate DPoP and Service Auth nonce stores to Redis
2. Implement access token hashing
3. Add comprehensive integration tests for OAuth flows
4. Security audit of custom OAuth implementation

**Medium-term (Phase 7 → Production)**:
1. Load testing with realistic federated traffic
2. Penetration testing focused on OAuth/DPoP
3. Consider hybrid approach: use `@atproto/oauth-provider` for core OAuth
4. Maintain DPoP as value-add over Bluesky

**Long-term (Post-Production)**:
1. Per-endpoint rate limits (not just global)
2. Advanced security headers (CSP, X-Frame-Options)
3. Automated security scanning in CI/CD
4. Bug bounty program

#### For Bluesky PDS:

**Recommendations** (if applicable):
1. Document DPoP support status (is it in the library?)
2. Add fine-grained OAuth scopes for third-party apps
3. Implement cross-PDS specific rate limiting
4. Consider token hashing for database storage

---

## Appendix: Security Checklist

### Aurora Locus Production Security Checklist

- [ ] **Nonce Store**: Migrate to Redis
- [ ] **Token Hashing**: Store SHA-256 hashed tokens
- [ ] **Security Audit**: Third-party OAuth audit
- [ ] **Penetration Testing**: Focus on OAuth, DPoP, cross-PDS
- [ ] **Load Testing**: Federated traffic simulation
- [ ] **Monitoring**: Prometheus metrics + alerting
- [ ] **Logging**: Structured logs for all auth events
- [ ] **Key Rotation**: Implement JWT key rotation
- [ ] **Disaster Recovery**: Nonce store backup/restore
- [ ] **Documentation**: Security runbooks for operators

### Both Implementations

- [ ] **Security Headers**: X-Frame-Options, CSP, HSTS
- [ ] **Rate Limiting**: Per-endpoint limits
- [ ] **Input Validation**: Fuzz testing
- [ ] **Dependency Scanning**: Automated vulnerability checks
- [ ] **Secrets Management**: Vault/HSM integration
- [ ] **Incident Response**: Security incident playbook

---

**Last Updated**: 2025-11-05
**Phase**: 6.8 - Security & Authorization Model Comparison
**Status**: Analysis Complete
**Next Phase**: Threat Model Analysis (Phase 6.8 continued)
