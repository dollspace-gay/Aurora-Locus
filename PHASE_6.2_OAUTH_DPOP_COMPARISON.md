# Phase 6.2: OAuth & DPoP Implementation Comparison

**Date**: 2025-11-05
**Status**: In Analysis
**Priority**: P1
**BD Issue**: Aurora-Locus-449

---

## Executive Summary

Bluesky PDS implements a **full OAuth 2.1 authorization server** with DPoP token binding using the official `@atproto/oauth-provider` library, while Aurora Locus currently has **basic JWT session tokens** with a placeholder DPoP implementation. This represents a significant compliance gap for ATProto federation.

### Critical Gaps Identified:
- ❌ **No OAuth 2.1 Authorization Server** (Aurora Locus uses simple JWT tokens)
- ❌ **No PKCE Flow Support**
- ❌ **No Device Registration/Management**
- ⚠️ **DPoP Implementation Incomplete** (placeholder with TODOs)
- ❌ **No Scope-based Permissions**
- ❌ **No Client Registration/Management**
- ❌ **No Authorization Request Flow**

---

## 1. Architecture Comparison

### 1.1 Bluesky PDS OAuth Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                      Bluesky PDS OAuth Stack                      │
├──────────────────────────────────────────────────────────────────┤
│                                                                   │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │          @atproto/oauth-provider (Official Library)        │  │
│  │                                                             │  │
│  │  • OAuth 2.1 Authorization Server                          │  │
│  │  • PKCE Support (RFC 7636)                                 │  │
│  │  • DPoP Token Binding (RFC 9449)                           │  │
│  │  • Client Registration (Dynamic)                           │  │
│  │  • Scope Management                                        │  │
│  │  • Authorization Code Flow                                 │  │
│  └────────────────────────────────────────────────────────────┘  │
│                             ↓↑                                    │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │                    OAuthStore Adapter                       │  │
│  │                                                             │  │
│  │  Implements: AccountStore, RequestStore, DeviceStore,      │  │
│  │              TokenStore, LexiconStore                       │  │
│  │                                                             │  │
│  │  • Account Creation & Authentication                        │  │
│  │  • Device Management                                        │  │
│  │  • Authorization Request Handling                           │  │
│  │  • Token Lifecycle (create, rotate, revoke)                │  │
│  │  • Refresh Token Rotation                                  │  │
│  │  • Authorized Client Tracking                              │  │
│  └────────────────────────────────────────────────────────────┘  │
│                             ↓↑                                    │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │                   Database Schema                           │  │
│  │                                                             │  │
│  │  Tables:                                                    │  │
│  │  • device (session tracking)                               │  │
│  │  • account_device (device-account binding)                 │  │
│  │  • authorization_request (pending auth flows)              │  │
│  │  • token (OAuth tokens with DPoP binding)                  │  │
│  │  • used_refresh_token (rotation tracking)                  │  │
│  │  • authorized_client (client permissions)                  │  │
│  │  • lexicon (schema caching)                                │  │
│  └────────────────────────────────────────────────────────────┘  │
│                                                                   │
└──────────────────────────────────────────────────────────────────┘
```

### 1.2 Aurora Locus Current Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                  Aurora Locus Authentication Stack                │
├──────────────────────────────────────────────────────────────────┤
│                                                                   │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │              Simple JWT Session Tokens                       │  │
│  │                                                             │  │
│  │  • Access Token (JWT, 1 hour expiry)                        │  │
│  │  • Refresh Token (JWT, 180 days expiry)                     │  │
│  │  • App Passwords (long-lived credentials)                   │  │
│  │  • Basic Token Rotation (mark as "used")                    │  │
│  └────────────────────────────────────────────────────────────┘  │
│                             ↓↑                                    │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │                  AccountManager                             │  │
│  │                                                             │  │
│  │  • create_session() - Login → tokens                        │  │
│  │  • validate_access_token() - Verify JWT                     │  │
│  │  • refresh_session() - Exchange refresh token               │  │
│  │  • cleanup_expired_sessions()                               │  │
│  └────────────────────────────────────────────────────────────┘  │
│                             ↓↑                                    │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │              DPoP Module (PLACEHOLDER)                       │  │
│  │                                                             │  │
│  │  ⚠️ TODO: Proper JWK-to-PEM conversion for EC keys          │  │
│  │  ⚠️ Currently uses placeholder implementation                │  │
│  │                                                             │  │
│  │  • DPopNonceStore (nonce tracking - WORKS)                  │  │
│  │  • DPopVerifier (verification - INCOMPLETE)                 │  │
│  │  • jwk_to_decoding_key() - ⚠️ PLACEHOLDER                    │  │
│  └────────────────────────────────────────────────────────────┘  │
│                             ↓↑                                    │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │                   Database Schema                           │  │
│  │                                                             │  │
│  │  Tables:                                                    │  │
│  │  • session (access + refresh tokens)                        │  │
│  │  • refresh_token (separate tracking)                        │  │
│  │  • email_token (verification/reset)                         │  │
│  │  • app_password (long-lived auth)                           │  │
│  │                                                             │  │
│  │  ❌ Missing:                                                 │  │
│  │  • device                                                   │  │
│  │  • authorization_request                                    │  │
│  │  • authorized_client                                        │  │
│  └────────────────────────────────────────────────────────────┘  │
│                                                                   │
└──────────────────────────────────────────────────────────────────┘
```

---

## 2. Feature-by-Feature Comparison

### 2.1 OAuth Authorization Server

| Feature | Bluesky PDS | Aurora Locus | Gap |
|---------|-------------|--------------|-----|
| **OAuth 2.1 Server** | ✅ Full implementation via `@atproto/oauth-provider` | ❌ None | **CRITICAL** |
| **Authorization Code Flow** | ✅ With PKCE | ❌ N/A | **CRITICAL** |
| **Client Registration** | ✅ Dynamic registration supported | ❌ None | HIGH |
| **Redirect URI Validation** | ✅ Built-in | ❌ N/A | HIGH |
| **State Parameter** | ✅ CSRF protection | ❌ N/A | HIGH |
| **Authorization Request** | ✅ Stored in `authorization_request` table | ❌ No request tracking | HIGH |

**Bluesky Implementation:**
- Uses official `@atproto/oauth-provider` library
- Full OAuth 2.1 authorization server
- Handles authorization requests, consent, token exchange
- Supports multiple grant types

**Aurora Locus Current State:**
- No OAuth server
- Direct username/password login → JWT tokens
- No authorization flow or consent screen

---

### 2.2 PKCE (Proof Key for Code Exchange)

| Feature | Bluesky PDS | Aurora Locus | Gap |
|---------|-------------|--------------|-----|
| **PKCE Support** | ✅ Required for all flows | ❌ None | **CRITICAL** |
| **Code Challenge** | ✅ S256 hashing | ❌ N/A | **CRITICAL** |
| **Code Verifier** | ✅ Validated on token exchange | ❌ N/A | **CRITICAL** |

**Why This Matters:**
- PKCE prevents authorization code interception attacks
- **Required for ATProto compliance**
- Essential for mobile/native apps

---

### 2.3 DPoP (Demonstrating Proof-of-Possession)

| Feature | Bluesky PDS | Aurora Locus | Gap |
|---------|-------------|--------------|-----|
| **DPoP Token Binding** | ✅ Full support | ⚠️ Placeholder | **CRITICAL** |
| **JWK Extraction** | ✅ From DPoP proof header | ⚠️ Implemented but not used | MEDIUM |
| **Signature Verification** | ✅ ES256 (ECDSA P-256) | ⚠️ **PLACEHOLDER** (uses secret instead of public key) | **CRITICAL** |
| **Thumbprint Computation** | ✅ RFC 7638 | ✅ Implemented correctly | OK |
| **Nonce Management** | ✅ Via OAuth provider | ✅ Implemented in `DPopNonceStore` | OK |
| **Token Binding Storage** | ✅ In `token` table | ❌ No storage | HIGH |

**Critical Issue in Aurora Locus:**

```rust
// src/federation/dpop.rs:254-256
// TODO: Implement proper JWK-to-PEM conversion for EC keys
// This is a placeholder implementation that needs to be replaced in production
// with proper EC public key parsing from JWK format
Ok(DecodingKey::from_secret(jwk_str.as_bytes()))  // ⚠️ WRONG!
```

**What Bluesky Does:**
```typescript
// Uses @atproto/oauth-provider's built-in DPoP verification
// Properly parses EC public key from JWK
// Verifies JWT signature using that public key
```

**What Needs to Happen:**
1. Parse EC public key from JWK format (x, y coordinates)
2. Convert to proper EC public key format
3. Verify JWT signature using `DecodingKey::from_ec_pem()` or similar
4. Consider using `jsonwebtoken-jwk` crate or similar

---

### 2.4 Device Management

| Feature | Bluesky PDS | Aurora Locus | Gap |
|---------|-------------|--------------|-----|
| **Device Registration** | ✅ `device` table | ❌ None | **CRITICAL** |
| **Device-Account Binding** | ✅ `account_device` table | ❌ None | **CRITICAL** |
| **Device Tracking** | ✅ sessionId, userAgent, ipAddress, lastSeenAt | ❌ None | HIGH |
| **Device Revocation** | ✅ Cascade delete to tokens | ❌ N/A | HIGH |
| **Multi-Device Support** | ✅ Full support | ❌ Session-based only | HIGH |

**Bluesky Device Schema:**
```sql
CREATE TABLE device (
    id TEXT PRIMARY KEY,             -- Device ID (UUID)
    sessionId TEXT NOT NULL,         -- Browser/app session
    userAgent TEXT,                  -- Client info
    ipAddress TEXT,                  -- Last IP
    lastSeenAt DATETIME NOT NULL,    -- Activity tracking
    createdAt DATETIME NOT NULL,
    updatedAt DATETIME NOT NULL
);

CREATE TABLE account_device (
    did TEXT NOT NULL,               -- Account DID
    deviceId TEXT NOT NULL,          -- Device reference
    createdAt DATETIME NOT NULL,
    updatedAt DATETIME NOT NULL,
    PRIMARY KEY (did, deviceId),
    FOREIGN KEY (deviceId) REFERENCES device(id) ON DELETE CASCADE
);
```

**Aurora Locus:**
- No device tracking
- Sessions are not bound to devices
- No way to revoke access per-device

---

### 2.5 Token Management

| Feature | Bluesky PDS | Aurora Locus | Gap |
|---------|-------------|--------------|-----|
| **Token Storage** | ✅ `token` table | ✅ `session` + `refresh_token` tables | OK |
| **Refresh Token Rotation** | ✅ Full rotation with `used_refresh_token` tracking | ⚠️ Mark as "used" but no rotation | MEDIUM |
| **Token Revocation** | ✅ Per-token revocation | ⚠️ Delete session | MEDIUM |
| **Token Binding (DPoP)** | ✅ Stored with token | ❌ No binding | **CRITICAL** |
| **Scope Tracking** | ✅ Per-token scopes | ❌ No scopes | HIGH |
| **Client Tracking** | ✅ clientId, clientAuth | ❌ No client tracking | HIGH |
| **Code Challenge** | ✅ Stored for PKCE | ❌ N/A | **CRITICAL** |

**Bluesky Token Schema:**
```sql
CREATE TABLE token (
    id INTEGER PRIMARY KEY,
    tokenId TEXT UNIQUE NOT NULL,          -- UUID
    createdAt DATETIME NOT NULL,
    expiresAt DATETIME NOT NULL,
    updatedAt DATETIME NOT NULL,
    clientId TEXT NOT NULL,                -- OAuth client
    clientAuth JSON,                       -- Client authentication data
    deviceId TEXT,                         -- Device binding
    did TEXT NOT NULL,                     -- Account DID
    parameters JSON NOT NULL,              -- Authorization parameters
    details JSON,                          -- Additional metadata
    code TEXT UNIQUE,                      -- Authorization code
    currentRefreshToken TEXT UNIQUE,       -- Active refresh token
    scope TEXT NOT NULL,                   -- Granted scopes
    FOREIGN KEY (deviceId) REFERENCES device(id) ON DELETE CASCADE,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE TABLE used_refresh_token (
    id INTEGER PRIMARY KEY,
    tokenId INTEGER NOT NULL,              -- Reference to token
    refreshToken TEXT UNIQUE NOT NULL,     -- Used refresh token
    createdAt DATETIME NOT NULL,
    FOREIGN KEY (tokenId) REFERENCES token(id) ON DELETE CASCADE
);
```

**Key Insight:**
Bluesky tracks **all previously used refresh tokens** to detect replay attacks. If a used refresh token is presented, the entire token is revoked.

**Aurora Locus Schema:**
```sql
CREATE TABLE session (
    id TEXT PRIMARY KEY,
    did TEXT NOT NULL,
    access_token TEXT UNIQUE NOT NULL,     -- JWT
    refresh_token TEXT UNIQUE NOT NULL,    -- JWT
    created_at DATETIME NOT NULL,
    expires_at DATETIME NOT NULL,
    app_password_name TEXT                 -- For app passwords
);

CREATE TABLE refresh_token (
    id TEXT PRIMARY KEY,
    did TEXT NOT NULL,
    token TEXT UNIQUE NOT NULL,            -- JWT
    created_at DATETIME NOT NULL,
    expires_at DATETIME NOT NULL,
    used BOOLEAN NOT NULL,                 -- Simple flag
    used_at DATETIME
);
```

**Gaps:**
- No device binding
- No client tracking
- No scope tracking
- Refresh rotation incomplete (no used token tracking)

---

### 2.6 Scope Management

| Feature | Bluesky PDS | Aurora Locus | Gap |
|---------|-------------|--------------|-----|
| **Scope Definition** | ✅ Per-client scopes | ❌ None | **CRITICAL** |
| **Scope Validation** | ✅ On authorization | ❌ N/A | **CRITICAL** |
| **Scope Enforcement** | ✅ Per-endpoint checks | ❌ No granular permissions | HIGH |
| **Scope Storage** | ✅ In `token` table | ❌ N/A | HIGH |

**ATProto Scopes:**
- `atproto` - Full ATProto API access
- `transition:generic` - Generic transitional scope
- Custom scopes per app

**Aurora Locus:**
- No concept of scopes
- All authenticated users have full access
- App passwords have same permissions as main account

---

### 2.7 Authorization Request Flow

| Feature | Bluesky PDS | Aurora Locus | Gap |
|---------|-------------|--------------|-----|
| **Request Storage** | ✅ `authorization_request` table | ❌ None | **CRITICAL** |
| **Request Expiration** | ✅ Automatic cleanup | ❌ N/A | HIGH |
| **Code Generation** | ✅ Cryptographically secure | ❌ N/A | **CRITICAL** |
| **Code Consumption** | ✅ One-time use enforced | ❌ N/A | **CRITICAL** |
| **State Tracking** | ✅ PKCE + state parameter | ❌ N/A | **CRITICAL** |

**Bluesky Authorization Request Schema:**
```sql
CREATE TABLE authorization_request (
    id TEXT PRIMARY KEY,              -- Request ID (UUID)
    did TEXT,                         -- Authenticated user (nullable until auth)
    deviceId TEXT,                    -- Device (nullable until auth)
    clientId TEXT NOT NULL,           -- OAuth client
    clientAuth JSON NOT NULL,         -- Client authentication method
    parameters JSON NOT NULL,         -- Authorization parameters (PKCE, scopes, etc.)
    expiresAt DATETIME NOT NULL,      -- Request expiration
    code TEXT UNIQUE,                 -- Authorization code (after consent)
    createdAt DATETIME NOT NULL,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE,
    FOREIGN KEY (deviceId) REFERENCES device(id) ON DELETE CASCADE
);
```

**Flow:**
1. Client initiates authorization request
2. Request stored with PKCE challenge
3. User authenticates and grants consent
4. Authorization code generated and bound to request
5. Client exchanges code for tokens (verifying PKCE)
6. Request deleted after use

---

### 2.8 Authorized Client Tracking

| Feature | Bluesky PDS | Aurora Locus | Gap |
|---------|-------------|--------------|-----|
| **Client Permissions** | ✅ `authorized_client` table | ❌ None | HIGH |
| **Granted Scopes** | ✅ Per-client scopes | ❌ None | HIGH |
| **Client Revocation** | ✅ Revoke all client tokens | ❌ N/A | HIGH |
| **Last Used Tracking** | ✅ updatedAt timestamp | ❌ N/A | MEDIUM |

**Bluesky Schema:**
```sql
CREATE TABLE authorized_client (
    did TEXT NOT NULL,
    clientId TEXT NOT NULL,
    clientAuth JSON NOT NULL,
    createdAt DATETIME NOT NULL,
    updatedAt DATETIME NOT NULL,
    PRIMARY KEY (did, clientId),
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);
```

**Purpose:**
- Track which OAuth clients have been authorized by each user
- Allow users to see and revoke app permissions
- Persist consent across sessions

---

## 3. OAuth Flow Diagrams

### 3.1 Bluesky PDS OAuth 2.1 + PKCE + DPoP Flow

```
┌─────────┐                                  ┌────────────┐                           ┌──────────────┐
│  Client │                                  │ Bluesky PDS│                           │ User Browser │
│  (App)  │                                  │            │                           │              │
└────┬────┘                                  └──────┬─────┘                           └───────┬──────┘
     │                                              │                                        │
     │  1. Generate code_verifier                  │                                        │
     │     code_challenge = SHA256(verifier)       │                                        │
     │                                              │                                        │
     │  2. Authorization Request                   │                                        │
     │  GET /oauth/authorize?                      │                                        │
     │    response_type=code                       │                                        │
     │    client_id=...                            │                                        │
     │    redirect_uri=...                         │                                        │
     │    scope=atproto                            │                                        │
     │    code_challenge=...                       │                                        │
     │    code_challenge_method=S256               │                                        │
     │    state=...                                │                                        │
     │ ────────────────────────────────────────────>                                        │
     │                                              │                                        │
     │                                              │  3. Redirect to login                  │
     │                                              │ ──────────────────────────────────────>│
     │                                              │                                        │
     │                                              │  4. User authenticates                 │
     │                                              │ <──────────────────────────────────────│
     │                                              │                                        │
     │                                              │  5. Store authorization_request        │
     │                                              │     (with code_challenge, device)      │
     │                                              │                                        │
     │                                              │  6. Show consent screen                │
     │                                              │ ──────────────────────────────────────>│
     │                                              │                                        │
     │                                              │  7. User grants consent                │
     │                                              │ <──────────────────────────────────────│
     │                                              │                                        │
     │                                              │  8. Generate authorization code        │
     │                                              │     Update authorization_request       │
     │                                              │     with code, did, deviceId           │
     │                                              │                                        │
     │  9. Redirect with code                      │                                        │
     │  http://client/callback?                    │                                        │
     │    code=xxx&state=...                       │                                        │
     │ <────────────────────────────────────────────                                        │
     │                                              │                                        │
     │  10. Generate DPoP proof (private key)      │                                        │
     │      dpop_proof = JWT signed with ES256     │                                        │
     │      Header: {typ:"dpop+jwt", jwk:{...}}    │                                        │
     │      Claims: {jti, htm, htu, iat, exp}      │                                        │
     │                                              │                                        │
     │  11. Token Request                          │                                        │
     │  POST /oauth/token                          │                                        │
     │  Headers:                                   │                                        │
     │    DPoP: {dpop_proof}                       │                                        │
     │  Body:                                      │                                        │
     │    grant_type=authorization_code            │                                        │
     │    code=xxx                                 │                                        │
     │    redirect_uri=...                         │                                        │
     │    code_verifier=...                        │                                        │
     │    client_id=...                            │                                        │
     │ ────────────────────────────────────────────>                                        │
     │                                              │                                        │
     │                                              │  12. Verify DPoP proof                 │
     │                                              │      - Extract JWK from header         │
     │                                              │      - Verify signature                │
     │                                              │      - Compute thumbprint              │
     │                                              │                                        │
     │                                              │  13. Verify PKCE                       │
     │                                              │      SHA256(code_verifier) ==          │
     │                                              │      code_challenge                    │
     │                                              │                                        │
     │                                              │  14. Create token bound to DPoP        │
     │                                              │      thumbprint + device               │
     │                                              │      INSERT INTO token (...)           │
     │                                              │      INSERT INTO authorized_client     │
     │                                              │      DELETE authorization_request      │
     │                                              │                                        │
     │  15. Token Response                         │                                        │
     │  {                                          │                                        │
     │    access_token: "...",                     │                                        │
     │    refresh_token: "...",                    │                                        │
     │    token_type: "DPoP",                      │                                        │
     │    expires_in: 3600,                        │                                        │
     │    scope: "atproto"                         │                                        │
     │  }                                          │                                        │
     │ <────────────────────────────────────────────                                        │
     │                                              │                                        │
     │  16. API Request with DPoP                  │                                        │
     │  POST /xrpc/com.atproto.repo.createRecord   │                                        │
     │  Headers:                                   │                                        │
     │    Authorization: DPoP {access_token}       │                                        │
     │    DPoP: {fresh_dpop_proof}                 │                                        │
     │ ────────────────────────────────────────────>                                        │
     │                                              │                                        │
     │                                              │  17. Verify DPoP proof                 │
     │                                              │      - Same JWK thumbprint?            │
     │                                              │      - Fresh nonce?                    │
     │                                              │      - Correct htm/htu?                │
     │                                              │                                        │
     │  18. Success Response                       │                                        │
     │ <────────────────────────────────────────────                                        │
     │                                              │                                        │
```

### 3.2 Aurora Locus Current Flow (Simple JWT)

```
┌─────────┐                                  ┌────────────────┐
│  Client │                                  │  Aurora Locus  │
│  (App)  │                                  │      PDS       │
└────┬────┘                                  └────────┬───────┘
     │                                                │
     │  1. Login Request                             │
     │  POST /xrpc/com.atproto.server.createSession  │
     │  {                                            │
     │    identifier: "user@example.com",            │
     │    password: "..."                            │
     │  }                                            │
     │ ──────────────────────────────────────────────>
     │                                                │
     │                                                │  2. Validate credentials
     │                                                │     Lookup user in database
     │                                                │     Verify password hash
     │                                                │
     │                                                │  3. Generate JWT tokens
     │                                                │     access_token = JWT{did, sid, exp=1h}
     │                                                │     refresh_token = JWT{did, sid, exp=180d}
     │                                                │
     │                                                │  4. Store session
     │                                                │     INSERT INTO session (...)
     │                                                │     INSERT INTO refresh_token (...)
     │                                                │
     │  5. Session Response                          │
     │  {                                            │
     │    accessJwt: "...",                          │
     │    refreshJwt: "...",                         │
     │    did: "did:plc:...",                        │
     │    handle: "user.example.com"                 │
     │  }                                            │
     │ <──────────────────────────────────────────────
     │                                                │
     │  6. API Request                               │
     │  POST /xrpc/com.atproto.repo.createRecord     │
     │  Headers:                                     │
     │    Authorization: Bearer {accessJwt}          │
     │ ──────────────────────────────────────────────>
     │                                                │
     │                                                │  7. Validate JWT
     │                                                │     Verify signature
     │                                                │     Check expiration
     │                                                │     Lookup session
     │                                                │
     │  8. Success Response                          │
     │ <──────────────────────────────────────────────
     │                                                │
     │                                                │
     │  ❌ NO: OAuth flow                             │
     │  ❌ NO: PKCE                                   │
     │  ❌ NO: DPoP binding                           │
     │  ❌ NO: Device tracking                        │
     │  ❌ NO: Scopes                                 │
     │  ❌ NO: Client registration                    │
     │                                                │
```

---

## 4. Database Schema Comparison

### 4.1 Bluesky PDS OAuth Tables

**Core OAuth Tables:**

```sql
-- 1. Device tracking
CREATE TABLE device (
    id TEXT PRIMARY KEY,
    sessionId TEXT NOT NULL,
    userAgent TEXT,
    ipAddress TEXT,
    lastSeenAt DATETIME NOT NULL,
    createdAt DATETIME NOT NULL,
    updatedAt DATETIME NOT NULL
);

-- 2. Device-Account binding
CREATE TABLE account_device (
    did TEXT NOT NULL,
    deviceId TEXT NOT NULL,
    createdAt DATETIME NOT NULL,
    updatedAt DATETIME NOT NULL,
    PRIMARY KEY (did, deviceId),
    FOREIGN KEY (deviceId) REFERENCES device(id) ON DELETE CASCADE
);

-- 3. Authorization requests (pending flows)
CREATE TABLE authorization_request (
    id TEXT PRIMARY KEY,
    did TEXT,
    deviceId TEXT,
    clientId TEXT NOT NULL,
    clientAuth JSON NOT NULL,
    parameters JSON NOT NULL,
    expiresAt DATETIME NOT NULL,
    code TEXT UNIQUE,
    createdAt DATETIME NOT NULL,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE,
    FOREIGN KEY (deviceId) REFERENCES device(id) ON DELETE CASCADE
);
CREATE INDEX authorization_request_code_idx ON authorization_request(code) WHERE code IS NOT NULL;
CREATE INDEX authorization_request_expires_at_idx ON authorization_request(expiresAt);

-- 4. OAuth tokens
CREATE TABLE token (
    id INTEGER PRIMARY KEY,
    tokenId TEXT UNIQUE NOT NULL,
    createdAt DATETIME NOT NULL,
    expiresAt DATETIME NOT NULL,
    updatedAt DATETIME NOT NULL,
    clientId TEXT NOT NULL,
    clientAuth JSON,
    deviceId TEXT,
    did TEXT NOT NULL,
    parameters JSON NOT NULL,
    details JSON,
    code TEXT UNIQUE,
    currentRefreshToken TEXT UNIQUE,
    scope TEXT NOT NULL,
    FOREIGN KEY (deviceId) REFERENCES device(id) ON DELETE CASCADE,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);
CREATE INDEX token_did_idx ON token(did);
CREATE INDEX token_code_idx ON token(code) WHERE code IS NOT NULL;
CREATE INDEX token_token_id_idx ON token(tokenId);
CREATE UNIQUE INDEX token_refresh_token_unique_idx ON token(currentRefreshToken) WHERE currentRefreshToken IS NOT NULL;

-- 5. Used refresh tokens (rotation tracking)
CREATE TABLE used_refresh_token (
    id INTEGER PRIMARY KEY,
    tokenId INTEGER NOT NULL,
    refreshToken TEXT UNIQUE NOT NULL,
    createdAt DATETIME NOT NULL,
    FOREIGN KEY (tokenId) REFERENCES token(id) ON DELETE CASCADE
);
CREATE INDEX used_refresh_token_refresh_token_idx ON used_refresh_token(refreshToken);

-- 6. Authorized clients
CREATE TABLE authorized_client (
    did TEXT NOT NULL,
    clientId TEXT NOT NULL,
    clientAuth JSON NOT NULL,
    createdAt DATETIME NOT NULL,
    updatedAt DATETIME NOT NULL,
    PRIMARY KEY (did, clientId),
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);

-- 7. Lexicon schema cache
CREATE TABLE lexicon (
    nsid TEXT PRIMARY KEY,
    json JSON NOT NULL,
    createdAt DATETIME NOT NULL
);
```

### 4.2 Aurora Locus Current Tables

```sql
-- 1. Sessions (simple JWT)
CREATE TABLE session (
    id TEXT PRIMARY KEY,
    did TEXT NOT NULL,
    access_token TEXT UNIQUE NOT NULL,
    refresh_token TEXT UNIQUE NOT NULL,
    created_at DATETIME NOT NULL,
    expires_at DATETIME NOT NULL,
    app_password_name TEXT,
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE
);

-- 2. Refresh tokens
CREATE TABLE refresh_token (
    id TEXT PRIMARY KEY,
    did TEXT NOT NULL,
    token TEXT UNIQUE NOT NULL,
    created_at DATETIME NOT NULL,
    expires_at DATETIME NOT NULL,
    used BOOLEAN NOT NULL DEFAULT 0,
    used_at DATETIME,
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE
);

-- 3. Email tokens (verification/reset)
CREATE TABLE email_token (
    token TEXT PRIMARY KEY,
    did TEXT NOT NULL,
    purpose TEXT NOT NULL,
    created_at DATETIME NOT NULL,
    expires_at DATETIME NOT NULL,
    used BOOLEAN NOT NULL DEFAULT 0,
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE
);

-- 4. App passwords
CREATE TABLE app_password (
    name TEXT NOT NULL,
    did TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    created_at DATETIME NOT NULL,
    privileged BOOLEAN NOT NULL DEFAULT 0,
    PRIMARY KEY (did, name),
    FOREIGN KEY (did) REFERENCES account(did) ON DELETE CASCADE
);
```

### 4.3 Missing Tables in Aurora Locus

```sql
-- ❌ Missing: Device tracking
-- ❌ Missing: Device-account binding
-- ❌ Missing: Authorization request flow
-- ❌ Missing: OAuth token storage (with DPoP binding)
-- ❌ Missing: Used refresh token tracking
-- ❌ Missing: Authorized client permissions
-- ❌ Missing: Lexicon schema cache
```

---

## 5. Critical Code Gaps

### 5.1 DPoP JWK Verification (CRITICAL)

**Aurora Locus Current (BROKEN):**
```rust
// src/federation/dpop.rs:243-256
fn jwk_to_decoding_key(jwk: &Value) -> PdsResult<DecodingKey> {
    let jwk_str = serde_json::to_string(jwk)
        .map_err(|e| PdsError::Internal(format!("Failed to serialize JWK: {}", e)))?;

    // TODO: Implement proper JWK-to-PEM conversion for EC keys
    // This is a placeholder implementation that needs to be replaced in production
    // with proper EC public key parsing from JWK format

    // ⚠️ THIS IS WRONG - treats JWK as a symmetric secret!
    Ok(DecodingKey::from_secret(jwk_str.as_bytes()))
}
```

**What It Should Do:**
```rust
fn jwk_to_decoding_key(jwk: &Value) -> PdsResult<DecodingKey> {
    // Extract EC public key components
    let kty = jwk["kty"].as_str().ok_or(...)?;
    let crv = jwk["crv"].as_str().ok_or(...)?;
    let x = jwk["x"].as_str().ok_or(...)?;
    let y = jwk["y"].as_str().ok_or(...)?;

    if kty != "EC" || crv != "P-256" {
        return Err(PdsError::Authentication("Unsupported JWK type".to_string()));
    }

    // Decode base64url coordinates
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let x_bytes = URL_SAFE_NO_PAD.decode(x)?;
    let y_bytes = URL_SAFE_NO_PAD.decode(y)?;

    // Construct uncompressed EC public key (0x04 prefix + x + y)
    let mut key_bytes = vec![0x04];
    key_bytes.extend_from_slice(&x_bytes);
    key_bytes.extend_from_slice(&y_bytes);

    // Convert to PEM or use jsonwebtoken-jwk crate
    // Option 1: Use jsonwebtoken-jwk crate
    // Option 2: Manual PEM construction
    // Option 3: Use p256 crate for EC key handling

    DecodingKey::from_ec_pem(&pem_bytes)?
}
```

**Recommended Solution:**
Use `jsonwebtoken-jwk` crate or `p256` crate for proper EC key handling.

---

### 5.2 Refresh Token Rotation (INCOMPLETE)

**Bluesky Implementation:**
```typescript
// bluesky-pds/src/account-manager/helpers/token.ts:531-563
async rotateToken(
  tokenId: TokenId,
  newTokenId: TokenId,
  newRefreshToken: RefreshToken,
  newData: NewTokenData,
): Promise<void> {
  const err = await this.db.transaction(async (dbTxn) => {
    // Get current token
    const { id, currentRefreshToken } = await tokenHelper
      .forRotateQB(dbTxn, tokenId)
      .executeTakeFirstOrThrow()

    // Store old refresh token in used_refresh_token table
    if (currentRefreshToken) {
      await usedRefreshTokenHelper
        .insertQB(dbTxn, id, currentRefreshToken)
        .execute()
    }

    // Check if new refresh token already used (replay attack!)
    const { count } = await usedRefreshTokenHelper
      .countQB(dbTxn, newRefreshToken)
      .executeTakeFirstOrThrow()

    if (count > 0) {
      // IMPORTANT: Don't throw - we don't want rollback
      return new Error('New refresh token already in use')
    }

    // Update token with new refresh token
    await tokenHelper
      .rotateQB(dbTxn, id, newTokenId, newRefreshToken, newData)
      .execute()
  })

  if (err) throw err
}
```

**Aurora Locus Current:**
```rust
// src/account/manager.rs:235-268
pub async fn refresh_session(&self, refresh_token: &str) -> PdsResult<Session> {
    // Find refresh token
    let row = sqlx::query("SELECT id, did, expires_at, used FROM refresh_token WHERE token = ?1")
        .bind(refresh_token)
        .fetch_optional(&self.db)
        .await?
        .ok_or_else(|| PdsError::Authentication("Invalid refresh token".to_string()))?;

    let token_id: String = row.get("id");
    let did: String = row.get("did");
    let used: bool = row.get("used");

    // Check if already used
    if used {
        return Err(PdsError::Authentication("Refresh token already used".to_string()));
    }

    // Mark old refresh token as used
    sqlx::query("UPDATE refresh_token SET used = TRUE, used_at = ?1 WHERE id = ?2")
        .bind(Utc::now())
        .bind(&token_id)
        .execute(&self.db)
        .await?;

    // ⚠️ Creates NEW session but doesn't track rotation history
    self.create_session(&did, None).await
}
```

**Gap:**
- ❌ Doesn't track **which** refresh tokens were used
- ❌ Can't detect if old refresh token is replayed after rotation
- ❌ No protection against refresh token replay attacks

**Fix Required:**
1. Add `used_refresh_token` table
2. Store old refresh token before rotation
3. Check if new token was previously used (replay detection)
4. Update existing token record instead of creating new session

---

### 5.3 Authorization Flow (MISSING)

**Required Components:**

```rust
// 1. Authorization request handler
async fn handle_authorization_request(
    query: AuthorizationParams,
) -> PdsResult<Redirect> {
    // Validate client_id, redirect_uri, scope
    // Store authorization request with PKCE challenge
    // Redirect to login if not authenticated
    // Show consent screen if authenticated
}

// 2. Consent handler
async fn handle_consent(
    request_id: String,
    approved: bool,
) -> PdsResult<Redirect> {
    // If approved: generate authorization code
    // Bind to device and user
    // Redirect to client with code
}

// 3. Token exchange handler
async fn exchange_authorization_code(
    request: TokenRequest,
    dpop_proof: Option<String>,
) -> PdsResult<TokenResponse> {
    // Verify authorization code
    // Verify PKCE code_verifier
    // Verify DPoP proof
    // Create token bound to DPoP thumbprint
    // Return access + refresh tokens
}
```

**Current State:**
- ❌ No authorization endpoint
- ❌ No consent screen
- ❌ No code exchange
- ❌ No PKCE verification

---

## 6. Migration Path to Full OAuth

### 6.1 Phase 1: Foundation (Estimated: 3-4 weeks)

**Objective**: Add database schema and basic OAuth infrastructure

**Tasks:**
1. **Database Migration** (1 week)
   - [ ] Add `device` table
   - [ ] Add `account_device` table
   - [ ] Add `authorization_request` table
   - [ ] Modify `session`/`refresh_token` → `token` table (OAuth format)
   - [ ] Add `used_refresh_token` table
   - [ ] Add `authorized_client` table
   - [ ] Add `lexicon` table
   - [ ] Create migration scripts

2. **DPoP Fixes** (1 week)
   - [ ] Fix `jwk_to_decoding_key()` - proper EC key parsing
   - [ ] Test with real DPoP proofs
   - [ ] Add integration tests
   - [ ] Consider using `jsonwebtoken-jwk` or `p256` crate

3. **Device Management** (1 week)
   - [ ] Device registration API
   - [ ] Device listing/revocation API
   - [ ] Device-account binding logic
   - [ ] Device activity tracking

4. **Refresh Token Rotation** (1 week)
   - [ ] Implement `used_refresh_token` tracking
   - [ ] Update refresh logic to detect replay
   - [ ] Add rotation tests

**Dependencies:**
- `jsonwebtoken-jwk` or `p256` for EC key handling
- Consider `oauth2` crate for client-side OAuth (if needed)

---

### 6.2 Phase 2: OAuth Authorization Server (Estimated: 4-5 weeks)

**Objective**: Implement full OAuth 2.1 authorization server with PKCE

**Options:**

#### Option A: Use Existing Rust OAuth Library
**Recommended**: `oxide-auth` or `ory/hydra` integration

**Pros:**
- Battle-tested OAuth implementation
- PKCE support built-in
- Reduced development time
- Security best practices included

**Cons:**
- Learning curve
- May need customization for ATProto specifics
- Dependency management

**Estimated Time**: 3-4 weeks

#### Option B: Port `@atproto/oauth-provider` to Rust
**Note**: Significant effort, not recommended unless necessary

**Pros:**
- Perfect ATProto compatibility
- Control over implementation

**Cons:**
- 6-8 weeks of development
- Maintenance burden
- Error-prone translation

**Estimated Time**: 6-8 weeks

#### Option C: Build Custom OAuth Server
**Recommended for Phase 2**

**Tasks:**
1. **Authorization Endpoint** (1 week)
   - [ ] `/oauth/authorize` handler
   - [ ] PKCE challenge storage
   - [ ] Redirect URI validation
   - [ ] State parameter handling

2. **Consent Screen** (1 week)
   - [ ] User consent UI
   - [ ] Scope display
   - [ ] Grant/deny logic
   - [ ] Authorization code generation

3. **Token Endpoint** (1 week)
   - [ ] `/oauth/token` handler
   - [ ] Authorization code exchange
   - [ ] PKCE verification
   - [ ] DPoP proof verification
   - [ ] Token binding
   - [ ] Refresh token flow

4. **Client Management** (1 week)
   - [ ] Client registration (static for now)
   - [ ] Client authentication
   - [ ] Authorized client tracking

5. **Scope System** (1 week)
   - [ ] Define ATProto scopes
   - [ ] Scope validation
   - [ ] Scope enforcement middleware
   - [ ] Per-endpoint scope checks

**Dependencies:**
- `uuid` for request/token IDs
- `sha2` for PKCE challenge hashing
- `base64` for encoding
- `serde_json` for JSON storage

---

### 6.3 Phase 3: Integration & Testing (Estimated: 2-3 weeks)

**Objective**: Integrate OAuth with existing endpoints and test thoroughly

**Tasks:**
1. **Endpoint Migration** (1 week)
   - [ ] Update all XRPC endpoints to accept DPoP tokens
   - [ ] Add scope checks to endpoints
   - [ ] Maintain backward compatibility (temporary)

2. **Client SDK Updates** (1 week)
   - [ ] Update client authentication flow
   - [ ] Add DPoP proof generation
   - [ ] Handle OAuth redirect flow

3. **Testing** (1 week)
   - [ ] Unit tests for all OAuth flows
   - [ ] Integration tests with real clients
   - [ ] Security testing (OWASP OAuth)
   - [ ] Performance testing

**Test Coverage:**
- [ ] Authorization code flow
- [ ] PKCE verification
- [ ] DPoP binding
- [ ] Refresh token rotation
- [ ] Refresh token replay detection
- [ ] Device revocation
- [ ] Scope enforcement
- [ ] Multi-device scenarios

---

### 6.4 Phase 4: Production Deployment (Estimated: 1-2 weeks)

**Objective**: Deploy OAuth system to production with monitoring

**Tasks:**
1. **Backward Compatibility** (3 days)
   - [ ] Support legacy JWT tokens during transition
   - [ ] Deprecation timeline
   - [ ] Migration tooling for existing users

2. **Monitoring & Metrics** (2 days)
   - [ ] OAuth flow metrics (Prometheus)
   - [ ] DPoP verification failures
   - [ ] Token rotation metrics
   - [ ] Device tracking

3. **Documentation** (2 days)
   - [ ] OAuth integration guide
   - [ ] Client SDK documentation
   - [ ] Migration guide for developers
   - [ ] Security best practices

4. **Deployment** (3 days)
   - [ ] Staged rollout
   - [ ] Rollback plan
   - [ ] Post-deployment verification

---

## 7. Scope Management Design

### 7.1 ATProto Scopes

**Standard Scopes:**
```rust
pub enum AtProtoScope {
    /// Full ATProto API access (default)
    AtProto,

    /// Transitional scope for legacy clients
    TransitionGeneric,

    /// Read-only access to user's data
    ReadOnly,

    /// Write access to user's data
    Write,

    /// Account management (handle changes, etc.)
    AccountManagement,

    /// OAuth client management
    OAuthManagement,
}
```

**Scope Format:**
- `atproto` - Full access
- `atproto:read` - Read-only
- `atproto:write` - Write operations
- `atproto:account` - Account management
- `atproto:oauth` - OAuth management

### 7.2 Scope Enforcement

**Middleware:**
```rust
pub async fn require_scope(
    auth: AuthResult,
    required: Vec<AtProtoScope>,
) -> PdsResult<()> {
    let token_scopes = get_token_scopes(&auth.token_id).await?;

    for scope in required {
        if !token_scopes.contains(&scope) {
            return Err(PdsError::Authorization(
                format!("Missing required scope: {:?}", scope)
            ));
        }
    }

    Ok(())
}
```

**Usage:**
```rust
async fn create_record(
    auth: AuthResult,
    req: CreateRecordRequest,
) -> PdsResult<CreateRecordResponse> {
    require_scope(auth.clone(), vec![AtProtoScope::Write]).await?;

    // ... endpoint logic
}
```

### 7.3 Scope Storage

**In Token Table:**
```sql
ALTER TABLE token ADD COLUMN scope TEXT NOT NULL DEFAULT 'atproto';
```

**Scope Parsing:**
```rust
impl AtProtoScope {
    pub fn parse(s: &str) -> Vec<Self> {
        s.split_whitespace()
            .filter_map(|scope| {
                match scope {
                    "atproto" => Some(AtProtoScope::AtProto),
                    "atproto:read" => Some(AtProtoScope::ReadOnly),
                    "atproto:write" => Some(AtProtoScope::Write),
                    "atproto:account" => Some(AtProtoScope::AccountManagement),
                    "atproto:oauth" => Some(AtProtoScope::OAuthManagement),
                    _ => None,
                }
            })
            .collect()
    }
}
```

---

## 8. Recommendations

### 8.1 Immediate Actions (P0)

1. **Fix DPoP JWK Verification** (1 week)
   - Replace placeholder with proper EC key parsing
   - Use `jsonwebtoken-jwk` or `p256` crate
   - Add comprehensive tests

2. **Fix Refresh Token Rotation** (3 days)
   - Add `used_refresh_token` table
   - Implement replay detection
   - Update refresh logic

3. **Add Device Tracking** (1 week)
   - Add `device` and `account_device` tables
   - Implement device registration API
   - Track device activity

### 8.2 Short-term (P1) - Next 1-2 Months

4. **Implement Basic OAuth Flow** (4-5 weeks)
   - Authorization endpoint
   - Consent screen
   - Token exchange with PKCE
   - DPoP binding

5. **Add Scope System** (1 week)
   - Define ATProto scopes
   - Implement scope enforcement
   - Update endpoints

### 8.3 Medium-term (P2) - Next 3-6 Months

6. **Client Management** (2 weeks)
   - Client registration
   - Authorized client tracking
   - Revocation API

7. **Migration from Legacy Auth** (2 weeks)
   - Backward compatibility layer
   - Migration tooling
   - Deprecation timeline

8. **Comprehensive Testing** (2 weeks)
   - Security testing
   - Performance testing
   - Integration testing

---

## 9. Success Criteria

### 9.1 Functional Requirements

- [x] ✅ DPoP proof verification working with real EC keys
- [ ] OAuth 2.1 authorization flow complete
- [ ] PKCE verification working
- [ ] Device management functional
- [ ] Refresh token rotation with replay detection
- [ ] Scope-based permissions enforced
- [ ] Client registration and tracking

### 9.2 ATProto Compliance

- [ ] Passes ATProto OAuth compliance tests
- [ ] Compatible with official ATProto clients
- [ ] Supports cross-PDS authentication
- [ ] DPoP token binding enforced

### 9.3 Security Requirements

- [ ] No refresh token replay possible
- [ ] PKCE protects against code interception
- [ ] DPoP prevents token theft
- [ ] Scope isolation enforced
- [ ] Device revocation cascades properly

---

## 10. Conclusion

**Current State:** Aurora Locus has a **basic JWT authentication system** that is **not compliant with ATProto's OAuth 2.1 + DPoP requirements**.

**Critical Gaps:**
1. ❌ **No OAuth authorization server** (required for ATProto federation)
2. ❌ **DPoP verification broken** (placeholder implementation)
3. ❌ **No PKCE support** (required for mobile apps)
4. ❌ **No device management** (required for multi-device support)
5. ❌ **Incomplete refresh token rotation** (security risk)

**Estimated Effort:** 10-12 weeks to full ATProto OAuth compliance

**Recommended Path:**
1. **Phase 1** (3-4 weeks): Fix DPoP, add database schema, device management
2. **Phase 2** (4-5 weeks): Build OAuth authorization server with PKCE
3. **Phase 3** (2-3 weeks): Integration, testing, scope enforcement
4. **Phase 4** (1-2 weeks): Production deployment, migration

**Priority:** **P1 (HIGH)** - Required for production federation and ATProto compliance

---

**Next Steps:**
1. Review this analysis with the team
2. Decide on OAuth library vs custom implementation
3. Create implementation BD issues for each phase
4. Begin Phase 1 work (DPoP fixes, database migration)

---

**Document Status:** Complete
**Last Updated:** 2025-11-05
**Author:** Claude (Aurora Locus Analysis)
