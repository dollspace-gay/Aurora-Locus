# Authorization Pattern Recommendations

**Best Practices for OAuth 2.1 + ATProto Authorization**

Phase 6.8: Security & Authorization Model Comparison (Authorization Patterns)
Date: 2025-11-05
Status: Recommendations Complete

---

## Executive Summary

This document provides **best practice recommendations** for implementing authorization in Aurora Locus PDS and Bluesky PDS, based on OAuth 2.1, ATProto specification, and industry security standards.

**Key Recommendation**: Adopt **fine-grained, hierarchical OAuth 2.1 scopes** with **automatic lexicon mapping** and **principle of least privilege** enforcement.

---

## Table of Contents

1. [Authorization Principles](#1-authorization-principles)
2. [Scope Design Patterns](#2-scope-design-patterns)
3. [Authorization Enforcement](#3-authorization-enforcement)
4. [OAuth Client Patterns](#4-oauth-client-patterns)
5. [Cross-PDS Authorization](#5-cross-pds-authorization)
6. [Admin Authorization](#6-admin-authorization)
7. [Best Practices Summary](#7-best-practices-summary)

---

## 1. Authorization Principles

### 1.1 Principle of Least Privilege

**Definition**: Grant the minimum permissions required for a task

**Implementation**:

```rust
// ❌ BAD: Requesting overly broad scope
let scope = "atproto:*"; // Full access - rarely needed

// ✅ GOOD: Requesting specific scope
let scope = "atproto:repo.create atproto:read";
```

**Recommendations**:

1. **Default to read-only**: Apps should request `atproto:read` by default
2. **Explicit write scopes**: Request `atproto:write` or `atproto:repo.create` only when needed
3. **Never request `atproto:*`**: Reserved for first-party apps only
4. **User-visible permissions**: Show scope descriptions on consent screen

**Aurora Locus Implementation** ([scope.rs:152-174](../src/oauth/scope.rs#L152-L174)):
```rust
impl AtProtoScope {
    /// Get human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            AtProtoScope::Read => "Read-only access to all data",
            AtProtoScope::RepoCreate => "Create new records",
            AtProtoScope::RepoDelete => "Delete records",
            // ... show on consent screen
        }
    }
}
```

---

### 1.2 Defense in Depth

**Layers of Authorization**:

1. **Network Layer**: TLS 1.3, DDoS protection
2. **Authentication Layer**: Verify identity (OAuth, DPoP, Service JWT)
3. **Authorization Layer**: Check permissions (scopes)
4. **Business Logic Layer**: Validate resource ownership
5. **Data Layer**: Database access controls

**Example Multi-Layer Authorization**:

```rust
pub async fn delete_record(
    ctx: State<AppContext>,
    auth: OAuthAuthContext, // Layer 2: Authentication
    query: Query<DeleteRecordQuery>,
) -> PdsResult<Json<DeleteRecordResponse>> {
    // Layer 3: Authorization - check scope
    require_scope(&auth.token.scope, &AtProtoScope::RepoDelete)?;

    // Layer 4: Business logic - verify repo ownership
    if query.repo != auth.did {
        return Err(PdsError::Authorization(
            "Cannot delete records from another user's repo".to_string()
        ));
    }

    // Layer 5: Data layer - database ACLs (enforced by DB)
    ctx.actor_store.delete_record(&query.repo, &query.collection, &query.rkey).await?;

    Ok(Json(DeleteRecordResponse { /* ... */ }))
}
```

---

### 1.3 Fail Secure (Deny by Default)

**Principle**: If authorization check fails or is ambiguous, deny access

**Implementation**:

```rust
// ❌ BAD: Fails open (grants access on error)
pub fn check_scope(scope: &str) -> bool {
    match parse_scope(scope) {
        Ok(s) => s.has_permission(),
        Err(_) => true, // ⚠️ Grants access on parse error!
    }
}

// ✅ GOOD: Fails closed (denies access on error)
pub fn check_scope(scope: &str) -> PdsResult<()> {
    let scope_set = ScopeSet::from_str(scope)
        .map_err(|e| PdsError::Authorization(format!("Invalid scope: {}", e)))?;

    if scope_set.has_scope(&required_scope) {
        Ok(())
    } else {
        Err(PdsError::Authorization("Insufficient scope".to_string()))
    }
}
```

**Recommendations**:
1. **Always return Result**: Use `PdsResult<()>` for authorization checks
2. **Explicit errors**: Clear error messages for debugging (don't leak sensitive info to users)
3. **Logging**: Log all authorization failures with context

---

### 1.4 Complete Mediation

**Principle**: Check authorization on every request, even for cached data

**Implementation**:

```rust
// ❌ BAD: Caching bypasses authorization
let cached_record = cache.get(&record_id);
if cached_record.is_some() {
    return Ok(cached_record); // ⚠️ No auth check!
}

// ✅ GOOD: Always check authorization
let cached_record = cache.get(&record_id);

// Even if cached, verify current user has permission
require_scope(&auth.token.scope, &AtProtoScope::RepoGet)?;
if record.repo != auth.did && !is_public(record) {
    return Err(PdsError::Authorization("Cannot access private record"));
}

if let Some(record) = cached_record {
    return Ok(record);
}
```

---

## 2. Scope Design Patterns

### 2.1 Hierarchical Scopes (Recommended)

**Pattern**: Parent scopes include child scopes

**Aurora Locus Implementation** ([scope.rs:87-139](../src/oauth/scope.rs#L87-L139)):

```
atproto:*                    (All - includes everything)
├── atproto:read             (Read - includes all get/list operations)
│   ├── atproto:repo.get
│   ├── atproto:repo.list
│   └── atproto:identity.resolveDid
├── atproto:write            (Write - includes all create/update/delete)
│   ├── atproto:repo.create
│   ├── atproto:repo.update
│   ├── atproto:repo.delete
│   ├── atproto:blob.upload
│   └── atproto:blob.delete
└── atproto:repo.*           (Repo - includes all repo operations)
    ├── atproto:repo.create
    ├── atproto:repo.update
    ├── atproto:repo.delete
    ├── atproto:repo.list
    └── atproto:repo.get
```

**Benefits**:
- ✅ **Flexibility**: Apps can request broad or narrow scopes
- ✅ **Simplicity**: Users understand "read" vs "write"
- ✅ **Forward compatibility**: New operations inherit parent scope

**Example**:
```rust
// App requests "atproto:write"
let requested_scope = "atproto:write";

// User authorizes
let granted_scope = "atproto:write";

// Later, app calls createRecord endpoint
require_scope(&granted_scope, &AtProtoScope::RepoCreate)?; // ✅ Passes

// Later, app calls deleteRecord endpoint
require_scope(&granted_scope, &AtProtoScope::RepoDelete)?; // ✅ Passes

// Later, app calls getRecord endpoint
require_scope(&granted_scope, &AtProtoScope::RepoGet)?; // ❌ Fails (needs read)
```

---

### 2.2 Namespace-Based Scopes

**Pattern**: Group scopes by resource type

**Examples**:
- `atproto:repo.*` - All repository operations
- `atproto:identity.*` - All identity operations
- `atproto:blob.*` - All blob operations
- `atproto:admin.*` - All admin operations

**Implementation**:
```rust
pub enum AtProtoScope {
    RepoAll,          // atproto:repo.*
    IdentityAll,      // atproto:identity.*
    BlobAll,          // atproto:blob.* (if we add it)
    AdminAll,         // atproto:admin.*
}
```

**Benefits**:
- ✅ **Intuitive**: Developers understand "all repo operations"
- ✅ **Scalable**: Easy to add new operations under existing namespace
- ✅ **Auditable**: Clear permission boundaries

---

### 2.3 Action-Based Scopes (Fine-Grained)

**Pattern**: One scope per action

**Examples**:
- `atproto:repo.create` - Create records only
- `atproto:repo.delete` - Delete records only
- `atproto:identity.updateProfile` - Update profile only

**Use Case**: Highly sensitive apps (banking, healthcare) requiring audit trails

**Example**:
```rust
// App only needs to create posts, not delete
let requested_scope = "atproto:repo.create";

// User authorizes
let granted_scope = "atproto:repo.create";

// Later, app tries to delete
require_scope(&granted_scope, &AtProtoScope::RepoDelete)?; // ❌ Fails
```

**Benefits**:
- ✅ **Maximum security**: Principle of least privilege
- ✅ **Auditability**: Precise permission tracking
- ❌ **Complexity**: More scopes to manage

---

### 2.4 Hybrid Approach (Recommended)

**Pattern**: Combine hierarchical + namespace + action-based

**Aurora Locus Scopes**:

```rust
// Level 1: Global (hierarchical)
atproto:*           // All access (admin/first-party only)
atproto:read        // Read-only
atproto:write       // Write access

// Level 2: Namespace
atproto:repo.*      // All repo operations
atproto:identity.*  // All identity operations
atproto:admin.*     // All admin operations

// Level 3: Action-based (fine-grained)
atproto:repo.create
atproto:repo.update
atproto:repo.delete
atproto:repo.get
atproto:repo.list
```

**Benefits**:
- ✅ **Flexibility**: Apps choose granularity level
- ✅ **Simplicity**: Broad scopes for simple apps
- ✅ **Security**: Fine-grained scopes for sensitive apps

---

## 3. Authorization Enforcement

### 3.1 Middleware-Based Enforcement (Recommended)

**Pattern**: Centralized authorization checks in middleware

**Aurora Locus Implementation** ([middleware.rs](../src/api/middleware.rs)):

```rust
/// Unified authentication middleware
pub async fn require_auth_unified(
    ctx: State<AppContext>,
    headers: HeaderMap,
    mut req: Request,
) -> PdsResult<Request> {
    // Try OAuth first
    if let Some(token) = extract_bearer_token(&headers) {
        let oauth_token = validate_oauth_token(&ctx, &token).await?;
        req.extensions_mut().insert(UnifiedAuth::OAuth(oauth_token));
        return Ok(req);
    }

    // Try session auth
    if let Some(session) = validate_session(&ctx, &headers).await? {
        req.extensions_mut().insert(UnifiedAuth::Session(session));
        return Ok(req);
    }

    // Try cross-PDS service JWT
    if let Some(jwt) = extract_service_jwt(&headers) {
        let claims = verify_service_jwt(&ctx, &jwt).await?;
        req.extensions_mut().insert(UnifiedAuth::CrossPds(claims));
        return Ok(req);
    }

    Err(PdsError::Authentication("No valid authentication".to_string()))
}

/// Scope enforcement middleware
pub fn enforce_scope(required: AtProtoScope) -> impl Fn(&UnifiedAuth) -> PdsResult<()> {
    move |auth| {
        match auth {
            UnifiedAuth::OAuth(token) => {
                require_scope(&token.scope, &required)
            }
            UnifiedAuth::Session(_) => {
                // Sessions have full access (legacy auth)
                Ok(())
            }
            UnifiedAuth::CrossPds(_) => {
                // Cross-PDS has limited access
                if required.is_privileged() {
                    Err(PdsError::Authorization("Cross-PDS cannot perform privileged operations"))
                } else {
                    Ok(())
                }
            }
        }
    }
}
```

**Benefits**:
- ✅ **Centralized**: Single source of truth for auth logic
- ✅ **Reusable**: Apply to multiple endpoints
- ✅ **Testable**: Unit test middleware independently
- ✅ **Auditable**: Log all authorization decisions

---

### 3.2 Endpoint-Level Enforcement

**Pattern**: Check authorization in each endpoint handler

**Example**:
```rust
pub async fn create_record(
    ctx: State<AppContext>,
    auth: OAuthAuthContext, // Extractor handles OAuth validation
    query: Query<CreateRecordQuery>,
    body: Json<CreateRecordBody>,
) -> PdsResult<Json<CreateRecordResponse>> {
    // 1. Check scope
    require_scope(&auth.token.scope, &AtProtoScope::RepoCreate)?;

    // 2. Check repo ownership
    if query.repo != auth.did {
        return Err(PdsError::Authorization(
            "Cannot create records in another user's repo".to_string()
        ));
    }

    // 3. Validate input
    validate_collection(&query.collection)?;
    validate_rkey(&query.rkey)?;

    // 4. Create record
    let record = ctx.actor_store.create_record(
        &query.repo,
        &query.collection,
        &query.rkey,
        &body.record,
    ).await?;

    Ok(Json(CreateRecordResponse { uri: record.uri, cid: record.cid }))
}
```

**Benefits**:
- ✅ **Explicit**: Authorization logic visible in endpoint
- ✅ **Flexible**: Can customize per endpoint
- ❌ **Repetitive**: Must repeat checks in every endpoint

---

### 3.3 Automatic Lexicon-Based Enforcement (Best Practice)

**Pattern**: Automatically map XRPC method to required scope

**Aurora Locus Implementation** ([scope.rs:546-581](../src/oauth/scope.rs#L546-L581)):

```rust
/// Map ATProto lexicon NSID to required scope
pub fn lexicon_to_scope(nsid: &str) -> AtProtoScope {
    if nsid.starts_with("com.atproto.repo.create") {
        AtProtoScope::RepoCreate
    } else if nsid.starts_with("com.atproto.repo.put") {
        AtProtoScope::RepoUpdate
    } else if nsid.starts_with("com.atproto.repo.delete") {
        AtProtoScope::RepoDelete
    } else if nsid.starts_with("com.atproto.repo.get") {
        AtProtoScope::RepoGet
    } else if nsid.starts_with("com.atproto.repo.list") {
        AtProtoScope::RepoList
    } else if nsid.starts_with("com.atproto.identity.resolve") {
        AtProtoScope::IdentityResolveDid
    } else {
        AtProtoScope::Read // Safe default
    }
}

// Usage in middleware
pub async fn auto_scope_check(req: &Request, auth: &OAuthAuthContext) -> PdsResult<()> {
    let nsid = extract_nsid_from_path(req.uri())?;
    let required_scope = lexicon_to_scope(&nsid);

    require_scope(&auth.token.scope, &required_scope)
}
```

**Benefits**:
- ✅ **Automatic**: No manual scope checks needed
- ✅ **Consistent**: Same logic for all endpoints
- ✅ **Maintainable**: Update mapping in one place

---

## 4. OAuth Client Patterns

### 4.1 Client Types

#### Confidential Clients (Server-Side)

**Characteristics**:
- Can securely store client secret
- Server-to-server communication
- Full OAuth 2.1 flow with client authentication

**Example**: Backend service, server-side app

**Authorization Code Flow**:
```
1. Client redirects user to /oauth/authorize
2. User authorizes
3. Server redirects back with authorization code
4. Client exchanges code + client_secret for tokens
5. Client uses access token for API calls
```

**Security**:
- ✅ Client secret stored securely on server
- ✅ PKCE optional but recommended
- ✅ Can use refresh tokens safely

---

#### Public Clients (Browser/Mobile)

**Characteristics**:
- Cannot securely store client secret
- Client-side JavaScript or mobile app
- PKCE mandatory

**Example**: React app, mobile app

**Authorization Code Flow with PKCE**:
```
1. Client generates code_verifier (random string)
2. Client computes code_challenge = SHA256(code_verifier)
3. Client redirects to /oauth/authorize?code_challenge=...&code_challenge_method=S256
4. User authorizes
5. Server redirects back with authorization code
6. Client exchanges code + code_verifier for tokens
7. Server verifies: SHA256(code_verifier) == code_challenge
8. Client uses access token for API calls
```

**Security**:
- ✅ PKCE prevents authorization code interception
- ❌ No client secret (public client)
- ⚠️ Refresh tokens risky (can be stolen from browser)

---

### 4.2 Client Registration Patterns

#### Manual Registration (Current)

```rust
pub struct OAuthClient {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub client_type: ClientType, // Confidential or Public
    pub allowed_scopes: Vec<AtProtoScope>,
}

// Admin manually registers client
let client = OAuthClient {
    client_id: "my-app",
    client_name: "My Awesome App",
    redirect_uris: vec!["https://myapp.com/callback".to_string()],
    client_type: ClientType::Confidential,
    allowed_scopes: vec![AtProtoScope::Read, AtProtoScope::RepoCreate],
};

ctx.oauth_store.register_client(client).await?;
```

---

#### Dynamic Registration (RFC 7591) (Future)

**Pattern**: Clients self-register via API

```json
POST /oauth/register
{
  "client_name": "My Awesome App",
  "redirect_uris": ["https://myapp.com/callback"],
  "scope": "atproto:read atproto:repo.create",
  "grant_types": ["authorization_code", "refresh_token"],
  "token_endpoint_auth_method": "client_secret_basic"
}

Response:
{
  "client_id": "s6BhdRkqt3",
  "client_secret": "ZJYCqe3GGRvdrudKyZS0XhGv_Z45DuKhCUk0gBR1vZk",
  "client_secret_expires_at": 0,
  "redirect_uris": ["https://myapp.com/callback"]
}
```

**Benefits**:
- ✅ Self-service (no admin intervention)
- ✅ Standardized (RFC 7591)
- ⚠️ Requires client verification

---

### 4.3 Client Verification (Recommendation)

**Problem**: Malicious clients can impersonate legitimate apps

**Solution**: Domain ownership verification

**Pattern 1: DNS TXT Record**
```
1. Client registers with redirect_uri: https://myapp.com/callback
2. Server generates verification token: "atproto-verify=abc123"
3. Client adds DNS TXT record: myapp.com TXT "atproto-verify=abc123"
4. Server verifies DNS record
5. Client is marked as "verified"
```

**Pattern 2: HTTPS Callback Verification**
```
1. Client registers with redirect_uri: https://myapp.com/callback
2. Server generates verification URL: https://myapp.com/.well-known/atproto-client?token=abc123
3. Server fetches URL and checks for token
4. Client is marked as "verified"
```

**Benefits**:
- ✅ Prevents phishing (verified badge on consent screen)
- ✅ User trust (domain ownership proven)
- ✅ Abuse prevention (can revoke unverified clients)

---

## 5. Cross-PDS Authorization

### 5.1 Service Auth Pattern (ATProto Spec)

**Pattern**: Short-lived JWT signed with user's atproto key

**Aurora Locus Implementation** ([service_auth.rs:55-117](../src/federation/service_auth.rs#L55-L117)):

```rust
/// Create service auth JWT for cross-PDS request
pub async fn create_service_jwt(
    &self,
    user_did: &str,
    target_service_did: &str,
    endpoint: Option<&str>,
) -> PdsResult<String> {
    // Get user's signing key from DID document
    let signing_key = self.identity_resolver.get_signing_key(user_did).await?;

    // Create JWT claims
    let claims = ServiceAuthClaims {
        iss: user_did.to_string(),       // Issuer: User DID
        aud: target_service_did.to_string(), // Audience: Target PDS DID
        exp: (Utc::now() + Duration::seconds(59)).timestamp(), // <60 seconds
        iat: Utc::now().timestamp(),
        lxm: endpoint.map(|s| s.to_string()), // Optional endpoint
        jti: Uuid::new_v4().to_string(),  // Nonce
    };

    // Sign with ES256
    let header = Header {
        typ: Some("at+jwt".to_string()),
        alg: Algorithm::ES256,
        ..Default::default()
    };

    let token = encode(&header, &claims, &EncodingKey::from_ec_pem(&signing_key)?)?;

    Ok(token)
}
```

**Verification** ([service_auth.rs:119-220](../src/federation/service_auth.rs#L119-L220)):

```rust
/// Verify service auth JWT from another PDS
pub async fn verify_service_jwt(
    &self,
    token: &str,
    expected_audience: &str,
) -> PdsResult<ServiceAuthClaims> {
    // 1. Decode JWT to get issuer DID (without verification)
    let unverified = decode::<ServiceAuthClaims>(token, &DecodingKey::from_secret(&[]), &Validation::default())?;
    let issuer_did = &unverified.claims.iss;

    // 2. Resolve issuer's DID document to get public key
    let signing_key = self.identity_resolver.get_signing_key(issuer_did).await?;

    // 3. Verify JWT signature
    let mut validation = Validation::new(Algorithm::ES256);
    validation.set_audience(&[expected_audience]);
    validation.leeway = 0; // Strict expiration

    let token_data = decode::<ServiceAuthClaims>(token, &DecodingKey::from_ec_pem(&signing_key)?, &validation)?;

    // 4. Additional validation
    let claims = token_data.claims;
    let time_to_expire = claims.exp - Utc::now().timestamp();

    if time_to_expire > 60 {
        return Err(PdsError::Authentication("JWT expiration exceeds 60 second limit"));
    }

    Ok(claims)
}
```

**Security Properties**:
- ✅ **No callback to origin PDS**: Purely cryptographic verification
- ✅ **DID-based trust**: Public key from DID document
- ✅ **Short-lived**: <60 seconds (ATProto requirement)
- ✅ **Nonce-based replay prevention**: JTI must be unique

---

### 5.2 Rate Limiting for Cross-PDS (Critical)

**Pattern**: 10x stricter rate limits for federated requests

**Aurora Locus Implementation** ([rate_limit.rs](../src/rate_limit.rs)):

```rust
pub enum RateLimitType {
    Local,       // 100 req/s (baseline)
    CrossPDS,    // 10 req/s (10x stricter)
    Unauthenticated, // 10 req/s
    Admin,       // 1000 req/s
}

pub async fn check_cross_pds(&self, did: &str) -> PdsResult<()> {
    if self.is_rate_limited(did, RateLimitType::CrossPDS).await? {
        Err(PdsError::RateLimited(
            "Cross-PDS rate limit exceeded (10 req/s)".to_string()
        ))
    } else {
        Ok(())
    }
}
```

**Rationale**:
1. **DoS Protection**: Prevent single compromised PDS from overwhelming target
2. **Fair Usage**: Prioritize local users over federated requests
3. **Resource Protection**: Limit impact of federation on local service

**Recommendation for Bluesky PDS**: Add similar cross-PDS rate limiting

---

### 5.3 Trust Boundaries

**Pattern**: Define trust levels for federated instances

```rust
pub enum PdsTrust {
    Trusted,    // Whitelisted, no special limits
    Known,      // Seen before, standard cross-PDS limits
    Unknown,    // Never seen, stricter limits
    Blocked,    // Malicious, reject all requests
}

pub async fn get_pds_trust_level(&self, pds_did: &str) -> PdsTrust {
    if self.blocked_list.contains(pds_did) {
        return PdsTrust::Blocked;
    }

    if self.trusted_list.contains(pds_did) {
        return PdsTrust::Trusted;
    }

    if self.known_instances.contains_key(pds_did) {
        return PdsTrust::Known;
    }

    PdsTrust::Unknown
}
```

**Benefits**:
- ✅ **Granular control**: Different limits per trust level
- ✅ **Abuse mitigation**: Block malicious instances
- ✅ **Performance**: Trusted instances get higher limits

---

## 6. Admin Authorization

### 6.1 Role-Based Access Control (RBAC)

**Pattern**: Assign roles with different privilege levels

**Aurora Locus Implementation** ([admin/mod.rs](../src/admin/mod.rs)):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    SuperAdmin,  // Full access
    Admin,       // Most operations
    Moderator,   // Moderation only
}

impl Role {
    pub fn can_act_as(&self, required: Role) -> bool {
        match (self, required) {
            (Role::SuperAdmin, _) => true, // SuperAdmin can do anything
            (Role::Admin, Role::Moderator) => true, // Admin can moderate
            (s, r) if s == &r => true, // Exact match
            _ => false,
        }
    }
}

// Usage
pub async fn delete_user(
    auth: AdminAuthContext,
    query: Query<DeleteUserQuery>,
) -> PdsResult<()> {
    // Require SuperAdmin role
    require_admin_role!(auth, Role::SuperAdmin)?;

    // Delete user
    ctx.account_manager.delete_account(&query.did).await?;

    Ok(())
}
```

**Benefits**:
- ✅ **Separation of duties**: Moderators can't delete accounts
- ✅ **Auditability**: Role logged with every admin action
- ✅ **Scalability**: Easy to add new roles

---

### 6.2 Admin Action Logging

**Pattern**: Log all admin actions for audit trail

```rust
pub async fn admin_action_log(
    admin_did: &str,
    admin_role: Role,
    action: &str,
    target_did: Option<&str>,
    metadata: serde_json::Value,
) -> PdsResult<()> {
    tracing::info!(
        admin_did = admin_did,
        admin_role = admin_role.as_str(),
        action = action,
        target_did = target_did,
        metadata = ?metadata,
        "Admin action performed"
    );

    // Also write to dedicated audit log database
    sqlx::query(
        "INSERT INTO admin_audit_log (admin_did, admin_role, action, target_did, metadata, timestamp)
         VALUES (?, ?, ?, ?, ?, ?)"
    )
    .bind(admin_did)
    .bind(admin_role.as_str())
    .bind(action)
    .bind(target_did)
    .bind(metadata)
    .bind(Utc::now())
    .execute(&ctx.account_db)
    .await?;

    Ok(())
}

// Usage
pub async fn delete_user(
    ctx: State<AppContext>,
    auth: AdminAuthContext,
    query: Query<DeleteUserQuery>,
) -> PdsResult<()> {
    require_admin_role!(auth, Role::SuperAdmin)?;

    // Log BEFORE action
    admin_action_log(
        &auth.did,
        auth.role,
        "delete_user",
        Some(&query.did),
        serde_json::json!({ "reason": query.reason }),
    ).await?;

    // Perform action
    ctx.account_manager.delete_account(&query.did).await?;

    Ok(())
}
```

**Benefits**:
- ✅ **Auditability**: Complete trail of admin actions
- ✅ **Compliance**: Meet regulatory requirements
- ✅ **Incident response**: Investigate admin abuse

---

## 7. Best Practices Summary

### 7.1 Authorization Checklist

#### Scope Design
- [ ] ✅ Use hierarchical scopes (All → Write → RepoCreate)
- [ ] ✅ Use namespace-based scopes (repo.*, identity.*, admin.*)
- [ ] ✅ Provide fine-grained action scopes (repo.create, repo.delete)
- [ ] ✅ Implement automatic lexicon → scope mapping
- [ ] ✅ Default to read-only (principle of least privilege)
- [ ] ✅ Mark privileged scopes (admin.*)
- [ ] ✅ Provide human-readable scope descriptions

#### Authorization Enforcement
- [ ] ✅ Check authorization on every request (complete mediation)
- [ ] ✅ Verify repo ownership (auth.did == query.repo)
- [ ] ✅ Use middleware for centralized enforcement
- [ ] ✅ Fail secure (deny by default on errors)
- [ ] ✅ Log all authorization failures
- [ ] ✅ Return clear error messages (without leaking sensitive info)

#### OAuth Clients
- [ ] ✅ Require PKCE for public clients (browsers, mobile)
- [ ] ✅ Support both confidential and public clients
- [ ] ✅ Implement client verification (domain ownership)
- [ ] ✅ Display verified badge on consent screen
- [ ] ✅ Allow user revocation of client access
- [ ] ✅ Show active sessions/devices to user

#### Cross-PDS
- [ ] ✅ Implement service auth JWT (<60 seconds)
- [ ] ✅ Verify JWT via DID document (no callback)
- [ ] ✅ Implement nonce-based replay prevention
- [ ] ✅ Apply 10x stricter rate limits
- [ ] ✅ Define trust boundaries (trusted, known, unknown, blocked)
- [ ] ✅ Log all cross-PDS requests

#### Admin
- [ ] ✅ Implement role-based access control (SuperAdmin, Admin, Moderator)
- [ ] ✅ Log all admin actions (audit trail)
- [ ] ✅ Separate admin authentication (JWT, not session)
- [ ] ✅ Whitelist admin DIDs in config
- [ ] ✅ Require multi-factor authentication for admin (future)

---

### 7.2 Aurora Locus Recommendations

**High Priority (P0)**:
1. ✅ **Hierarchical scopes** - Already implemented
2. ✅ **Automatic lexicon mapping** - Already implemented
3. ✅ **10x cross-PDS rate limiting** - Already implemented
4. ⚠️ **Token hashing** - TODO (production blocker)
5. ⚠️ **Nonce store scalability** - TODO (Redis migration)

**Medium Priority (P1)**:
1. ⚠️ **Client verification** - Add domain ownership verification
2. ⚠️ **OAuth phishing protection** - Add verified client badges
3. ⚠️ **Circuit breaker** - Add for federated calls
4. ⚠️ **Admin MFA** - Add multi-factor authentication

---

### 7.3 Bluesky PDS Recommendations

**High Priority (P0)**:
1. ❌ **Cross-PDS rate limiting** - Add 10x stricter limits (URGENT)
2. ⚠️ **Token hashing** - Verify if implemented, add if missing
3. ⚠️ **Fine-grained scopes** - Add more than 6 scopes

**Medium Priority (P1)**:
1. ⚠️ **DPoP support** - Consider adding token binding
2. ⚠️ **Client verification** - Add domain ownership verification
3. ⚠️ **Automatic lexicon mapping** - Add scope inference

---

## Conclusion

**Key Takeaways**:

1. **Aurora Locus** has implemented **industry-leading authorization patterns**:
   - ✅ Hierarchical, namespace-based, action-based scopes
   - ✅ Automatic lexicon → scope mapping
   - ✅ 10x cross-PDS rate limiting
   - ✅ Fine-grained admin RBAC

2. **Both implementations** should prioritize:
   - ⚠️ Token hashing (production blocker)
   - ⚠️ OAuth client verification (phishing protection)
   - ⚠️ Cross-PDS rate limiting (Bluesky urgent)

3. **Long-term improvements**:
   - Multi-factor authentication for admins
   - Dynamic client registration (RFC 7591)
   - Circuit breaker for federated calls
   - Per-instance reputation scoring

---

**Last Updated**: 2025-11-05
**Phase**: 6.8 - Authorization Pattern Recommendations
**Status**: Complete
**Next**: Security Hardening Roadmap
