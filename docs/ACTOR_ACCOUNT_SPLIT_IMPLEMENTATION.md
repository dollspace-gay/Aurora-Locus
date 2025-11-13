# Actor/Account Table Split Implementation Plan

**Issue**: Aurora-Locus-3lw
**Priority**: P0 - Required for Federation
**Status**: In Progress
**Date**: 2025-11-13

---

## Overview

This document tracks the implementation of splitting the combined `account` table into separate `actor` (public identity) and `account` (private authentication) tables to match the ATProto federation model and bluesky-pds schema.

### Why This Change Is Critical

1. **Federation Requirement**: Enables representation of federated actors from other PDS instances without requiring local authentication credentials
2. **Data Separation**: Separates public identity data from private authentication data
3. **ATProto Compliance**: Matches the official bluesky-pds schema for proper network interoperability
4. **Security**: Isolates authentication credentials from public actor information

---

## Phase 1: Schema Design ✅ COMPLETE

### 1.1 New Data Models

**Files Modified**:
- [src/db/account.rs](src/db/account.rs)

**New Structs Created**:

```rust
/// Actor - Public identity (lines 11-25)
pub struct Actor {
    pub did: String,
    pub handle: Option<String>,
    pub created_at: DateTime<Utc>,
    pub takedown_ref: Option<String>,
    pub deactivated_at: Option<DateTime<Utc>>,
    pub delete_after: Option<DateTime<Utc>>,
}

/// Account - Private authentication (lines 32-44)
pub struct Account {
    pub did: String,  // FK to actor.did
    pub email: Option<String>,
    pub password_hash: String,
    pub email_confirmed_at: Option<DateTime<Utc>>,
    pub invites_disabled: bool,
}

/// ActorAccount - Combined for convenience (lines 50-65)
pub struct ActorAccount {
    // Contains all fields from both Actor and Account
    // Used for queries that need both public and private data
}

/// PlcKeys - Cryptographic material (lines 71-81)
pub struct PlcKeys {
    pub did: String,
    pub rotation_key: String,
    pub rotation_key_public: String,
    pub last_operation_cid: Option<String>,
}
```

**Key Changes**:
- ✅ Separated public identity from private auth
- ✅ Changed `taken_down: bool` to `takedown_ref: Option<String>` for audit trail
- ✅ Added `delete_after` field for scheduled deletion
- ✅ Made `handle` optional (for unclaimed actors)
- ✅ Moved PLC keys to separate table for isolation
- ✅ Added `ActorAccount` convenience struct for joined queries

### 1.2 Database Migration

**Files Created**:
- [migrations/20251113000000_split_actor_account.sql](migrations/20251113000000_split_actor_account.sql)

**Migration Steps**:
1. ✅ Create `actor` table with indexes
2. ✅ Create new `account` table with foreign key to actor
3. ✅ Create `plc_keys` table
4. ✅ Migrate data from old `account` table to new tables
5. ✅ Rename old `account` table to `account_old` (backup)
6. ✅ Rename `account_new` to `account`

**Data Transformation**:
- `taken_down = 1` → `takedown_ref = 'manual_takedown'`
- `deactivated_at` + 3 days → `delete_after`
- `email_confirmed = 1` → `email_confirmed_at` populated
- PLC keys migrated to separate table

---

## Phase 2: AccountManager Updates ⚠️ IN PROGRESS

### 2.1 Update Core Account Operations

**Files to Modify**:
- [src/account/manager.rs](src/account/manager.rs)

**Methods Requiring Updates**:

#### High Priority (Breaking Changes)

| Method | Current Return | New Return | Status |
|--------|---------------|------------|--------|
| `create_account()` | `Account` | `ActorAccount` | ⚠️ TODO |
| `get_account()` | `Account` | `ActorAccount` | ⚠️ TODO |
| `get_account_by_identifier()` | `Account` | `ActorAccount` | ⚠️ TODO |
| `login()` | `(Account, Session)` | `(ActorAccount, Session)` | ⚠️ TODO |
| `login_with_app_password()` | `(Account, Session, String)` | `(ActorAccount, Session, String)` | ⚠️ TODO |
| `list_accounts()` | `Vec<Account>` | `Vec<ActorAccount>` | ⚠️ TODO |

#### Query Updates Required

**create_account() - Lines 29-100**:
```sql
-- Current: Single INSERT into account
INSERT INTO account (did, handle, email, password_hash, created_at, ...)

-- New: Two INSERTs with transaction
BEGIN TRANSACTION;
INSERT INTO actor (did, handle, created_at) VALUES (?, ?, ?);
INSERT INTO account (did, email, password_hash, email_confirmed_at, invites_disabled)
    VALUES (?, ?, ?, ?, ?);
INSERT INTO plc_keys (did, rotation_key, rotation_key_public, last_operation_cid)
    VALUES (?, ?, ?, ?);
COMMIT;
```

**get_account() - Find in manager.rs**:
```sql
-- Current: Single SELECT from account
SELECT * FROM account WHERE did = ? OR handle = ?

-- New: JOIN actor and account
SELECT
    a.did, a.handle, a.created_at, a.takedown_ref, a.deactivated_at, a.delete_after,
    ac.email, ac.password_hash, ac.email_confirmed_at, ac.invites_disabled
FROM actor a
LEFT JOIN account ac ON a.did = ac.did
WHERE a.did = ? OR a.handle = ?
```

**get_account_by_identifier() - Lines 650-680**:
```sql
-- Current: SELECT from account with handle OR email
SELECT * FROM account WHERE handle = ? OR email = ?

-- New: JOIN with OR conditions
SELECT
    a.did, a.handle, a.created_at, a.takedown_ref, a.deactivated_at, a.delete_after,
    ac.email, ac.password_hash, ac.email_confirmed_at, ac.invites_disabled
FROM actor a
LEFT JOIN account ac ON a.did = ac.did
WHERE a.handle = ? OR ac.email = ?
```

### 2.2 Update Takedown/Moderation

**Methods to Update**:
- `takedown_account()` - Update to set `takedown_ref` instead of `taken_down` boolean
- `activate_account()` - Update to clear `deactivated_at` and `delete_after`
- `deactivate_account()` - Update to set `delete_after` = `deactivated_at` + 3 days

### 2.3 Update Handle Operations

**Methods to Update**:
- `update_handle()` - Update actor table, not account
- `handle_exists()` - Query actor table
- `validate_handle()` - No changes needed

---

## Phase 3: Update API Endpoints ⚠️ TODO

### 3.1 Files Requiring Updates

**API Route Files**:
- [src/api/server.rs](src/api/server.rs) - createAccount, createSession
- [src/api/repo.rs](src/api/repo.rs) - Any account lookups
- [src/api/admin.rs](src/api/admin.rs) - Admin account management

**Changes Required**:
1. Update all `Account` types to `ActorAccount`
2. Update response serialization (some fields renamed)
3. Update error handling for actor vs account not found cases

### 3.2 Response Format Changes

**createAccount Response**:
```rust
// Old
CreateAccountResponse {
    did,
    handle,  // String
    access_jwt,
    refresh_jwt,
}

// New (handle is now Option<String>)
CreateAccountResponse {
    did,
    handle,  // Option<String> - may be None for new actors
    access_jwt,
    refresh_jwt,
}
```

---

## Phase 4: Update Tests ⚠️ TODO

### 4.1 Test Files Requiring Updates

- [tests/timing_attack_protection_test.rs](tests/timing_attack_protection_test.rs)
- [src/account/manager.rs](src/account/manager.rs) - inline tests

**Required Changes**:
1. Update table creation SQL in test helpers
2. Add actor table creation
3. Update account table schema
4. Add plc_keys table creation
5. Update test assertions for new field names

### 4.2 Example Test Update

**Current (timing_attack_protection_test.rs:24-42)**:
```rust
CREATE TABLE account (
    did TEXT PRIMARY KEY,
    handle TEXT UNIQUE NOT NULL,
    email TEXT UNIQUE,
    password_hash TEXT NOT NULL,
    created_at DATETIME NOT NULL,
    ...
)
```

**New**:
```rust
CREATE TABLE actor (
    did TEXT PRIMARY KEY,
    handle TEXT,
    created_at DATETIME NOT NULL,
    takedown_ref TEXT,
    deactivated_at DATETIME,
    delete_after DATETIME
);

CREATE TABLE account (
    did TEXT PRIMARY KEY,
    email TEXT UNIQUE,
    password_hash TEXT NOT NULL,
    email_confirmed_at DATETIME,
    invites_disabled BOOLEAN NOT NULL DEFAULT 0,
    FOREIGN KEY (did) REFERENCES actor(did)
);

CREATE TABLE plc_keys (
    did TEXT PRIMARY KEY,
    rotation_key TEXT NOT NULL,
    rotation_key_public TEXT NOT NULL,
    last_operation_cid TEXT,
    FOREIGN KEY (did) REFERENCES actor(did)
);
```

---

## Phase 5: Database Initialization ⚠️ TODO

### 5.1 Update Schema Creation

**Location**: Search for "CREATE TABLE account" in codebase

**Files to Update**:
- Any initialization code that creates tables
- Test setup code
- Development database setup scripts

**Required Changes**:
1. Create actor table before account table
2. Create plc_keys table
3. Update foreign key references
4. Update indexes

---

## Phase 6: Migration Execution Plan ⚠️ TODO

### 6.1 Manual Migration Steps

For existing deployments:

```bash
# 1. Backup existing database
cp data/pds.db data/pds.db.backup-$(date +%Y%m%d)

# 2. Run migration
sqlite3 data/pds.db < migrations/20251113000000_split_actor_account.sql

# 3. Verify migration
sqlite3 data/pds.db "SELECT COUNT(*) FROM account_old;"
sqlite3 data/pds.db "SELECT COUNT(*) FROM actor;"
sqlite3 data/pds.db "SELECT COUNT(*) FROM account;"

# 4. Test application
cargo run

# 5. If successful, can drop backup table
sqlite3 data/pds.db "DROP TABLE IF EXISTS account_old;"
```

### 6.2 Rollback Plan

If migration fails:

```sql
-- Drop new tables
DROP TABLE IF EXISTS plc_keys;
DROP TABLE IF EXISTS account;
DROP TABLE IF EXISTS actor;

-- Restore old table
ALTER TABLE account_old RENAME TO account;
```

---

## Phase 7: Code Quality ⚠️ TODO

### 7.1 Update Documentation

- [ ] Update architecture documentation
- [ ] Update API documentation
- [ ] Update README if schema is mentioned
- [ ] Add migration guide for developers

### 7.2 Add Helper Functions

Recommended additions to `src/db/account.rs`:

```rust
impl ActorAccount {
    /// Check if actor has a local account
    pub fn has_local_account(&self) -> bool {
        self.password_hash.is_some()
    }

    /// Check if actor is federated (no local account)
    pub fn is_federated(&self) -> bool {
        !self.has_local_account()
    }

    /// Check if actor is taken down
    pub fn is_taken_down(&self) -> bool {
        self.takedown_ref.is_some()
    }

    /// Check if actor is scheduled for deletion
    pub fn is_scheduled_for_deletion(&self) -> bool {
        self.delete_after.is_some()
    }
}
```

---

## Impact Analysis

### Breaking Changes

| Component | Impact | Migration Required |
|-----------|--------|-------------------|
| AccountManager API | HIGH | All methods return ActorAccount instead of Account |
| Database Schema | HIGH | Migration script required |
| API Responses | MEDIUM | Handle is now Option<String> |
| Tests | HIGH | All test schemas need updating |

### Non-Breaking Changes

| Component | Impact | Notes |
|-----------|--------|-------|
| Session Management | LOW | Sessions still reference DID |
| OAuth Flow | LOW | OAuth tables reference DID |
| App Passwords | LOW | Reference DID, not affected |
| Email Tokens | LOW | Reference DID, not affected |

---

## Testing Checklist

### Unit Tests
- [ ] Account creation with actor/account split
- [ ] Account lookup by DID
- [ ] Account lookup by handle
- [ ] Account lookup by email
- [ ] Login with password (actor+account join)
- [ ] Login with app password
- [ ] Handle updates (actor table)
- [ ] Takedown with ref (not boolean)
- [ ] Deactivation with delete_after
- [ ] PLC key storage and retrieval

### Integration Tests
- [ ] Full registration flow
- [ ] Full login flow
- [ ] Session management
- [ ] Account deactivation
- [ ] Account deletion
- [ ] Handle changes
- [ ] Email confirmation

### Migration Tests
- [ ] Test migration on database with existing data
- [ ] Verify all data migrated correctly
- [ ] Verify foreign key relationships maintained
- [ ] Verify indexes created
- [ ] Rollback and restore works

---

## Current Status Summary

### ✅ Completed

1. **Schema Design** - New Actor, Account, PlcKeys, ActorAccount structs defined
2. **Migration Script** - Complete SQL migration with data transformation
3. **Documentation** - This implementation plan document

### ⚠️ In Progress

4. **AccountManager Updates** - Need to update all query methods

### ⚠️ Blocked/Waiting

5. **API Updates** - Blocked on AccountManager completion
6. **Test Updates** - Blocked on AccountManager completion
7. **Migration Execution** - Blocked on code updates
8. **Production Deployment** - Blocked on testing

---

## Estimated Effort

| Phase | Complexity | Time Estimate |
|-------|-----------|---------------|
| Schema Design | Medium | 2 hours ✅ |
| AccountManager Updates | High | 4-6 hours ⚠️ |
| API Updates | Medium | 2-3 hours |
| Test Updates | Medium | 2-3 hours |
| Testing & QA | High | 3-4 hours |
| Documentation | Low | 1 hour |
| **Total** | **High** | **14-19 hours** |

---

## Next Steps

1. **Immediate**: Update AccountManager query methods to use actor+account joins
2. **Then**: Update all method return types from Account to ActorAccount
3. **Then**: Update API endpoints to handle ActorAccount
4. **Then**: Update all test files
5. **Finally**: Run migration and test on development database

---

## Notes

- This is a significant refactoring that touches many parts of the codebase
- Thorough testing is critical before deployment
- Consider feature flag for gradual rollout
- Keep account_old table for at least one release cycle as safety backup
- Monitor for any foreign key constraint violations after deployment

---

## References

- [Actor Store & Account Management Parity Analysis](ACTOR_STORE_ACCOUNT_PARITY_ANALYSIS.md)
- [Bluesky PDS Schema](../bluesky-pds/src/account-manager/db/schema/)
- [ATProto Specification](https://atproto.com/specs/did)
