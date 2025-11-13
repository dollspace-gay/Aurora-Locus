# Actor Store & Account Management Parity Analysis

**Issue**: Aurora-Locus-rbl
**Date**: 2025-11-13
**Comparison**: Aurora-Locus vs Bluesky PDS Official Implementation

---

## Executive Summary

This document provides a detailed comparison between Aurora-Locus's account management system and the official Bluesky PDS implementation to ensure ATProto federation compatibility.

### Overall Assessment

**Coverage**: ~75% feature parity
**Critical Gaps**: 3 major areas identified
**Priority**: P0 - Required for full federation compatibility

---

## 1. Account Creation & Registration Flow

### ✅ Implemented in Aurora-Locus

| Feature | Aurora-Locus | Bluesky PDS | Status |
|---------|--------------|-------------|--------|
| Create account with handle, email, password | ✅ `AccountManager::create_account()` | ✅ `AccountManager.createAccount()` | **PARITY** |
| Handle validation | ✅ `validate_handle()` | ✅ `normalizeAndValidateHandle()` | **PARTIAL** |
| Email validation | ✅ `validate_email()` | ✅ Email normalized to lowercase | **PARTIAL** |
| Password hashing (Argon2id) | ✅ atproto SDK | ✅ scrypt | **DIFFERENT** |
| DID generation & PLC registration | ✅ `generate_plc_did()` | ✅ Separate module | **PARITY** |
| Duplicate handle/email prevention | ✅ Database checks | ✅ Database unique constraints | **PARITY** |
| Invite code validation | ⚠️ Deferred to API layer | ✅ Built into AccountManager | **GAP** |

### 🔴 Gaps Identified

#### 1.1 Invite Code System
**Bluesky PDS**:
- `ensureInviteIsAvailable()` - validates invite code during account creation
- `recordInviteUse()` - tracks usage transactionally
- `createInviteCodes()` - creates invite codes with use count limits
- `getAccountInvitesCodes()` - retrieves invite history
- `disableInviteCodes()` - admin management

**Aurora-Locus**:
- ❌ No invite system implementation
- Config has `invites.required` but no enforcement

**Impact**: Cannot fully replicate Bluesky's account growth control

#### 1.2 Handle Validation Differences
**Bluesky PDS**:
- `baseNormalizeAndValidate()` - normalization rules
- `hasExplicitSlur()` - slur checking
- `isValidTld()` - TLD whitelist/blacklist
- `ensureHandleServiceConstraints()` - service domain rules
- External domain verification via DID resolution

**Aurora-Locus**:
- Basic character validation only (alphanumeric, `-`, `.`)
- Length constraints (3-253 chars)
- ❌ No slur checking
- ❌ No TLD validation
- ❌ No external domain verification

**Impact**: May accept invalid handles rejected by other PDS instances

#### 1.3 Account + Actor Separation
**Bluesky PDS**:
- Two tables: `actor` (public identity) + `account` (private auth data)
- Actor can exist without account (imported identities)
- Supports account-less actors for federation

**Aurora-Locus**:
- Single `account` table combines both
- All accounts require auth credentials
- Actor store is separate per-user repository

**Impact**: Cannot represent external actors without local accounts

---

## 2. DID Document Generation & Storage

### ✅ Implemented in Aurora-Locus

| Feature | Aurora-Locus | Bluesky PDS | Status |
|---------|--------------|-------------|--------|
| PLC DID generation | ✅ `generate_plc_did()` | ✅ External module | **PARITY** |
| DID:PLC registration with directory | ✅ HTTP POST to PLC server | ✅ Similar | **PARITY** |
| Signing key generation (K256) | ✅ 32-byte random key | ✅ Similar | **PARITY** |
| DID document structure | ✅ Multikey, service endpoints | ✅ Similar | **PARITY** |
| Rotation key storage | ✅ `plc_rotation_key` field | ✅ Not in AccountManager | **DIFFERENT** |
| Fallback to DID:Web | ✅ On PLC failure | ⚠️ Not observed | **EXTRA** |
| Operation CID tracking | ✅ `plc_last_operation_cid` | ❌ Not in account table | **EXTRA** |

### 🟡 Minor Differences

#### 2.1 Key Storage Location
**Bluesky PDS**: Rotation keys likely stored separately (not in account table)
**Aurora-Locus**: Stores `plc_rotation_key`, `plc_rotation_key_public`, and `plc_last_operation_cid` in account table

**Impact**: Minimal - both approaches work, Aurora approach more convenient

#### 2.2 CID Generation
**Bluesky PDS**: Uses proper CID generation via `multiformats/cid`
**Aurora-Locus**: Simplified hash-based CID generation

**Impact**: Minor - should use proper CID library for compatibility

---

## 3. Handle Reservation & Validation

### ✅ Implemented in Aurora-Locus

| Feature | Aurora-Locus | Bluesky PDS | Status |
|---------|--------------|-------------|--------|
| Handle uniqueness check | ✅ `handle_exists()` | ✅ Unique constraint | **PARITY** |
| Handle update | ✅ `update_handle()` | ✅ `updateHandle()` | **PARITY** |
| Handle format validation | ⚠️ Basic | ✅ Comprehensive | **GAP** |
| Reserved handles | ❌ Not implemented | ⚠️ Not in AccountManager | **UNKNOWN** |

### 🔴 Gaps Identified

#### 3.1 Reserved/Protected Handles
**Aurora-Locus**: Has `src/identity/reserved_handles.rs` but not integrated
**Bluesky PDS**: Not directly visible in AccountManager (may be in handle module)

**Action Required**: Verify if bluesky-pds has reserved handle checking and integrate Aurora's module

#### 3.2 Service Domain Constraints
**Bluesky PDS**: `ensureHandleServiceConstraints()` enforces rules like:
- No uppercase letters
- No leading/trailing hyphens
- Specific length limits for service domains
- Subdomain structure validation

**Aurora-Locus**: No service domain-specific rules

**Impact**: Service handles may not follow ATProto conventions

---

## 4. Account Status Management

### ✅ Implemented in Aurora-Locus

| Feature | Aurora-Locus | Bluesky PDS | Status |
|---------|--------------|-------------|--------|
| Account deactivation | ✅ `deactivated_at` field | ✅ `deactivatedAt` | **PARITY** |
| Account takedown | ✅ `taken_down` boolean | ✅ `takedownRef` string | **DIFFERENT** |
| Soft deletion / grace period | ⚠️ 30-day via `deactivated_at` | ✅ 3-day `deleteAfter` | **DIFFERENT** |
| Account activation | ⚠️ Manual `SET deactivated_at = NULL` | ✅ `activateAccount()` | **GAP** |

### 🟡 Differences

#### 4.1 Takedown Tracking
**Bluesky PDS**:
- `takedownRef` (string) - references moderation action
- Allows tracking *why* account was taken down
- `getAccountAdminStatus()` returns takedown info

**Aurora-Locus**:
- `taken_down` (boolean) - simple flag
- `takedown_ref` exists in records, not account table
- Less audit trail

**Impact**: Cannot track takedown reasons or reference moderation actions

#### 4.2 Deletion Grace Period
**Bluesky PDS**:
- `deactivatedAt` + `deleteAfter` (3 days)
- Immediate deactivation, delayed deletion
- Account marked for deletion quickly

**Aurora-Locus**:
- `deactivated_at` set to future date (30 days)
- `request_account_deletion()` for soft delete
- Longer grace period

**Impact**: Different user experience, Bluesky more aggressive

---

## 5. Email Verification Flow

### ✅ Implemented in Aurora-Locus

| Feature | Aurora-Locus | Bluesky PDS | Status |
|---------|--------------|-------------|--------|
| Email token generation | ✅ UUID-based, 24hr expiry | ✅ Similar | **PARITY** |
| Email confirmation | ✅ `confirm_email()` | ✅ `confirmEmail()` | **PARITY** |
| Email confirmed tracking | ✅ `email_confirmed_at` | ✅ `emailConfirmedAt` | **PARITY** |
| Token cleanup | ✅ On confirm/reset | ✅ `deleteAllEmailTokens()` | **PARITY** |
| Email update | ✅ `update_email()` (not shown) | ✅ `updateEmail()` + token cleanup | **PARITY** |

**Assessment**: Email verification is feature-complete ✅

---

## 6. Password Reset Functionality

### ✅ Implemented in Aurora-Locus

| Feature | Aurora-Locus | Bluesky PDS | Status |
|---------|--------------|-------------|--------|
| Reset token generation | ✅ UUID, 1hr expiry | ✅ Similar | **PARITY** |
| Password reset flow | ✅ `reset_password()` | ✅ `resetPassword()` | **PARITY** |
| Session invalidation on reset | ✅ Delete all sessions/tokens | ✅ `revokeRefreshTokensByDid()` | **PARITY** |
| Token purpose tracking | ✅ `reset_password` purpose | ✅ `EmailTokenPurpose` enum | **PARITY** |

**Assessment**: Password reset is feature-complete ✅

---

## 7. Authentication Mechanisms

### ✅ Implemented in Aurora-Locus

| Feature | Aurora-Locus | Bluesky PDS | Status |
|---------|--------------|-------------|--------|
| Password login | ✅ `login()` | ✅ `login()` | **PARITY** |
| App password authentication | ✅ `login_with_app_password()` | ✅ `verifyAppPassword()` | **PARITY** |
| JWT session tokens | ✅ Access (1hr) + Refresh (180d) | ✅ Similar expiry | **PARITY** |
| Session creation | ✅ `create_session()` | ✅ `createSession()` | **PARITY** |
| Session validation | ✅ `validate_access_token()` | ⚠️ Implicit in middleware | **PARITY** |
| Timing attack mitigation | ❌ Not implemented | ✅ 350ms minimum delay | **GAP** |

### 🔴 Security Gap

#### 7.1 Timing Attack Protection
**Bluesky PDS**:
```typescript
finally {
  // Mitigate timing attacks
  await wait(350 - (Date.now() - start))
}
```

**Aurora-Locus**: No timing protection

**Impact**: Potential vulnerability to timing-based username enumeration

**Fix Required**: Add constant-time delay to login function

---

## 8. Session Management

### ✅ Implemented in Aurora-Locus

| Feature | Aurora-Locus | Bluesky PDS | Status |
|---------|--------------|-------------|--------|
| Refresh token storage | ✅ `refresh_token` table | ✅ `refreshToken` table | **PARITY** |
| Token rotation | ✅ Mark old as used, create new | ✅ Grace period + rotation | **DIFFERENT** |
| Session expiration | ✅ 1hr access, 180d refresh | ✅ Similar | **PARITY** |
| Session cleanup | ✅ `cleanup_expired_sessions()` | ✅ `deleteExpiredRefreshTokens()` | **PARITY** |
| Logout (session deletion) | ✅ `delete_session()` | ✅ `revokeRefreshToken()` | **PARITY** |

### 🟡 Token Rotation Differences

#### 8.1 Refresh Token Grace Period
**Bluesky PDS**:
- 2-hour grace period (`REFRESH_GRACE_MS`)
- `addRefreshGracePeriod()` - allows old token to remain valid
- `nextId` tracking for concurrent refresh handling
- Handles refresh token reuse with same ID

**Aurora-Locus**:
- Immediate invalidation of old refresh token
- Mark as `used=true`, create new
- No grace period or concurrent handling

**Impact**: Aurora may reject valid refresh attempts during race conditions

---

## 9. Service Auth Tokens

### ❌ NOT Implemented in Aurora-Locus

**Bluesky PDS**: Likely has service-to-service auth tokens (not visible in AccountManager)
**Aurora-Locus**: No service auth token system

**Impact**: Cannot authenticate PDS-to-PDS or PDS-to-AppView communications

**Required for Federation**: YES - Critical for relay and AppView communication

---

## 10. App Passwords

### ✅ Implemented in Aurora-Locus

| Feature | Aurora-Locus | Bluesky PDS | Status |
|---------|--------------|-------------|--------|
| Create app password | ✅ `create_app_password()` | ✅ `createAppPassword()` | **PARITY** |
| App password format | ✅ 8x4 char groups with hyphens | ✅ Similar | **PARITY** |
| List app passwords | ✅ `list_app_passwords()` | ✅ `listAppPasswords()` | **PARITY** |
| Revoke app password | ✅ `revoke_app_password()` | ✅ `revokeAppPassword()` | **PARITY** |
| Privileged flag | ✅ Supported | ✅ `privileged` field | **PARITY** |
| Argon2id hashing | ✅ Via atproto SDK | ✅ scrypt | **DIFFERENT** |
| Session tracking | ✅ `app_password_name` in session | ✅ `appPassword` object | **PARITY** |

**Assessment**: App passwords are feature-complete ✅

---

## 11. Account Repository Setup

### ✅ Implemented in Aurora-Locus

| Feature | Aurora-Locus | Bluesky PDS | Status |
|---------|--------------|-------------|--------|
| Per-user repository creation | ✅ `ActorStore::create()` | ✅ Separate ActorStore | **PARITY** |
| Repository root tracking | ✅ `repo_root` table | ✅ `repoRoot` table | **PARITY** |
| Initial empty MST | ✅ Hardcoded empty root CID | ✅ Similar | **PARITY** |
| Repository initialization | ✅ On account creation | ✅ Via `repoCid`/`repoRev` params | **PARITY** |

**Assessment**: Repository setup is adequate ✅

---

## 12. Takedown & Suspension Logic

### ⚠️ Partially Implemented

| Feature | Aurora-Locus | Bluesky PDS | Status |
|---------|--------------|-------------|--------|
| Takedown account | ✅ `taken_down` boolean | ✅ `takedownAccount()` | **PARTIAL** |
| Revoke tokens on takedown | ❌ Not automatic | ✅ Transactional | **GAP** |
| Takedown reference tracking | ⚠️ In records only | ✅ `takedownRef` in actor | **GAP** |
| Soft-deleted account restrictions | ❌ Not enforced | ✅ Limited sessions | **GAP** |

### 🔴 Critical Gaps

#### 12.1 Takedown Enforcement
**Bluesky PDS**:
```typescript
await this.db.transaction(async (dbTxn) =>
  Promise.all([
    account.updateAccountTakedownStatus(dbTxn, did, takedown),
    auth.revokeRefreshTokensByDid(dbTxn, did),
    token.removeByDidQB(dbTxn, did).execute(),
  ]),
)
```

**Aurora-Locus**: No automatic token revocation on takedown

**Fix Required**: Add transactional token revocation when setting `taken_down=true`

#### 12.2 Soft Deletion Handling
**Bluesky PDS**: Soft-deleted accounts can login with limited session (no refresh token stored)
**Aurora-Locus**: No soft-deletion concept

---

## Summary of Critical Gaps

### P0 - Must Implement

1. **Timing Attack Protection** (lines 363-402 in bluesky account-manager.ts)
   - Add 350ms minimum delay to login functions
   - Prevents username enumeration

2. **Actor/Account Separation**
   - Split `account` table into `actor` (public) + `account` (private)
   - Support account-less actors for federation

3. **Takedown Token Revocation**
   - Automatically revoke all tokens when account taken down
   - Transactional consistency

4. **Service Auth Tokens**
   - Implement PDS-to-PDS authentication
   - Required for relay/AppView communication

### P1 - Should Implement

5. **Invite Code System**
   - Full invite code lifecycle management
   - Validation, tracking, admin tools

6. **Comprehensive Handle Validation**
   - Slur checking
   - TLD validation
   - Service domain constraints
   - External domain verification

7. **Refresh Token Grace Period**
   - 2-hour grace for old tokens
   - Concurrent refresh handling

### P2 - Nice to Have

8. **Takedown Reference Tracking**
   - Store moderation action IDs
   - Better audit trail

9. **Proper CID Generation**
   - Use `multiformats/cid` library
   - Full spec compliance

---

## Database Schema Comparison

### Bluesky PDS Tables

```sql
-- Actor (public identity)
CREATE TABLE actor (
  did TEXT PRIMARY KEY,
  handle TEXT,
  createdAt TEXT NOT NULL,
  takedownRef TEXT,
  deactivatedAt TEXT,
  deleteAfter TEXT
);

-- Account (private auth)
CREATE TABLE account (
  did TEXT PRIMARY KEY,
  email TEXT UNIQUE,
  passwordScrypt TEXT,
  emailConfirmedAt TEXT,
  invitesDisabled INTEGER DEFAULT 0,
  FOREIGN KEY (did) REFERENCES actor(did)
);

-- App Passwords
CREATE TABLE app_password (
  did TEXT NOT NULL,
  name TEXT NOT NULL,
  passwordScrypt TEXT NOT NULL,
  createdAt TEXT NOT NULL,
  privileged INTEGER DEFAULT 0,
  PRIMARY KEY (did, name)
);

-- Refresh Tokens
CREATE TABLE refresh_token (
  id TEXT PRIMARY KEY,
  did TEXT NOT NULL,
  expiresAt TEXT NOT NULL,
  nextId TEXT,
  appPasswordName TEXT,
  FOREIGN KEY (did) REFERENCES actor(did)
);

-- Email Tokens
CREATE TABLE email_token (
  purpose TEXT NOT NULL,
  did TEXT NOT NULL,
  token TEXT NOT NULL,
  requestedAt TEXT NOT NULL,
  PRIMARY KEY (purpose, did)
);

-- Invite Codes
CREATE TABLE invite_code (
  code TEXT PRIMARY KEY,
  availableUses INTEGER NOT NULL,
  disabled INTEGER DEFAULT 0,
  forAccount TEXT NOT NULL,
  createdBy TEXT NOT NULL,
  createdAt TEXT NOT NULL
);

-- Repo Root
CREATE TABLE repo_root (
  did TEXT PRIMARY KEY,
  cid TEXT NOT NULL,
  rev TEXT NOT NULL,
  indexedAt TEXT NOT NULL
);
```

### Aurora-Locus Tables

```sql
-- Account (combines actor + account)
CREATE TABLE account (
  did TEXT PRIMARY KEY,
  handle TEXT UNIQUE NOT NULL,
  email TEXT UNIQUE,
  password_hash TEXT NOT NULL,
  created_at DATETIME NOT NULL,
  email_confirmed BOOLEAN DEFAULT 0,
  email_confirmed_at DATETIME,
  deactivated_at DATETIME,
  taken_down BOOLEAN DEFAULT 0,
  plc_rotation_key TEXT,
  plc_rotation_key_public TEXT,
  plc_last_operation_cid TEXT
);

-- Session
CREATE TABLE session (
  id TEXT PRIMARY KEY,
  did TEXT NOT NULL,
  access_token TEXT UNIQUE NOT NULL,
  refresh_token TEXT UNIQUE NOT NULL,
  created_at DATETIME NOT NULL,
  expires_at DATETIME NOT NULL,
  app_password_name TEXT,
  FOREIGN KEY (did) REFERENCES account(did)
);

-- Refresh Token
CREATE TABLE refresh_token (
  id TEXT PRIMARY KEY,
  did TEXT NOT NULL,
  token TEXT UNIQUE NOT NULL,
  created_at DATETIME NOT NULL,
  expires_at DATETIME NOT NULL,
  used BOOLEAN DEFAULT 0,
  used_at DATETIME,
  FOREIGN KEY (did) REFERENCES account(did)
);

-- App Password
CREATE TABLE app_password (
  did TEXT NOT NULL,
  name TEXT NOT NULL,
  password_hash TEXT NOT NULL,
  created_at DATETIME NOT NULL,
  privileged BOOLEAN DEFAULT 0,
  PRIMARY KEY (did, name),
  FOREIGN KEY (did) REFERENCES account(did)
);

-- Email Token
CREATE TABLE email_token (
  token TEXT PRIMARY KEY,
  did TEXT NOT NULL,
  purpose TEXT NOT NULL,
  created_at DATETIME NOT NULL,
  expires_at DATETIME NOT NULL,
  used BOOLEAN DEFAULT 0
);

-- (No invite code table)

-- Actor Store per-user database (separate)
-- Each user has: repo_root, repo_block, record tables
```

### Key Differences

1. **Actor/Account Split**: Bluesky separates, Aurora combines
2. **Invite Codes**: Bluesky has table, Aurora doesn't
3. **Refresh Token Grace**: Bluesky has `nextId`, Aurora doesn't
4. **Takedown Tracking**: Bluesky has `takedownRef`, Aurora has boolean
5. **Deletion Schedule**: Bluesky has `deleteAfter`, Aurora uses `deactivated_at` for both

---

## Implementation Priority

### Phase 1: Security (Immediate)
1. Add timing attack protection to login
2. Implement takedown token revocation
3. Add actor/account table separation

### Phase 2: Federation (Week 1)
4. Implement service auth tokens
5. Add comprehensive handle validation
6. Implement proper takedown reference tracking

### Phase 3: Polish (Week 2)
7. Add invite code system
8. Implement refresh token grace period
9. Switch to proper CID generation

---

## Conclusion

Aurora-Locus has a solid account management foundation with ~75% feature parity to Bluesky PDS. The core authentication, password management, and session handling are well-implemented.

**Critical gaps** requiring immediate attention:
- Timing attack protection (security vulnerability)
- Actor/Account separation (federation requirement)
- Service auth tokens (federation requirement)
- Takedown token revocation (moderation requirement)

Once these P0 gaps are addressed, Aurora-Locus will be ready for ATProto network federation with respect to account management.
