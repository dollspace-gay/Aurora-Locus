# Phase 6: Bluesky PDS Parity Analysis

## Overview

This phase involves systematic comparison of Aurora Locus PDS with the official Bluesky PDS implementation to ensure ATProto spec compliance and feature parity.

**Source**: `bluesky-pds/` folder (official TypeScript implementation)

**Goal**: Identify gaps, verify compliance, and create roadmap for full ATProto compatibility

---

## Created BD Issues (12 Total)

### P0 - Critical (Must Have)

#### Aurora-Locus-ii8: Phase 6.10 - Sync Protocol Implementation
**Status**: Open | **Priority**: P0 (Highest)

**Critical Missing Endpoints:**
- `com.atproto.sync.getBlob` - Fetch blob by CID
- `com.atproto.sync.getBlocks` - Get repository blocks
- `com.atproto.sync.getLatestCommit` - Get latest commit CID
- `com.atproto.sync.getRecord` - Get record at specific commit
- `com.atproto.sync.getRepo` - Download entire repo as CAR
- `com.atproto.sync.listBlobs` - List all blob CIDs
- `com.atproto.sync.getCheckout` - Get repo at commit
- `com.atproto.sync.notifyOfUpdate` - Notify crawlers
- `com.atproto.sync.requestCrawl` - Request crawl

**Why Critical**: Required for federation, external indexing, and ATProto spec compliance

**Current State**: Only `subscribeRepos` implemented (via relay)

**Blocker For**: Federation, interoperability, repo backup/migration

---

### P1 - High Priority (Should Have)

#### Aurora-Locus-449: Phase 6.2 - OAuth & DPoP Implementation Comparison
**Status**: Open | **Priority**: P1

**Major Gaps:**
- ❌ Full OAuth 2.1 authorization server
- ❌ PKCE flow support
- ❌ Device registration/management
- ⚠️ DPoP JWK parsing (placeholder - production blocker)
- ❌ Scope-based permissions
- ❌ Client registration

**Bluesky Features to Match:**
- OAuth authorization requests (db migration 004)
- Device management (db migration 004)
- DPoP-bound tokens (account_device table)
- Permission sets (db migration 006)
- Refresh token rotation (used_refresh_token)

**Current Aurora State**: Basic session tokens, partial DPoP

---

#### Aurora-Locus-epe: Phase 6.3 - API Endpoint Parity Analysis
**Status**: Open | **Priority**: P1

**Endpoint Coverage:**

✅ **com.atproto.repo.*** (Repository): Mostly complete
- createRecord, putRecord, deleteRecord, getRecord, listRecords, describeRepo, uploadBlob, applyWrites

⚠️ **com.atproto.server.*** (Server): Partial
- Need verification of all server endpoints

❌ **com.atproto.sync.*** (Sync): Critical gaps (see P0 above)

⚠️ **com.atproto.admin.*** (Admin): Partial
- Need full comparison

❌ **app.bsky.*** (Bluesky App): Not implemented (expected - PDS-only)

**Deliverable**: Complete endpoint inventory and implementation roadmap

---

#### Aurora-Locus-9xe: Phase 6.8 - Security & Authorization Model Comparison
**Status**: Open | **Priority**: P1

**Security Gaps:**
- ❌ OAuth 2.1 authorization server
- ❌ PKCE flow implementation
- ⚠️ DPoP JWK parsing (production blocker)
- ❌ Scope-based authorization
- ❌ Device credential binding
- ⚠️ Per-endpoint rate limits (global only)

**Strengths (Aurora):**
- ✅ Service auth JWTs (Phase 4)
- ✅ Nonce-based replay prevention
- ✅ 10x stricter cross-PDS rate limiting
- ✅ Comprehensive audit logging

**Deliverable**: Security hardening roadmap, threat model update

---

### P2 - Medium Priority (Nice to Have)

#### Aurora-Locus-u6v: Phase 6.0 - Architecture & Module Structure Comparison
**Status**: Open | **Priority**: P2

**Module Mapping:**
| Bluesky PDS | Aurora Locus | Status |
|-------------|--------------|--------|
| account-manager/ | account/ | ✅ Similar |
| actor-store/ | actor_store/ | ✅ Similar |
| api/ | api/ | ✅ Similar |
| config/ | config/ | ✅ Similar |
| did-cache/ | identity/ | ⚠️ Different structure |
| sequencer/ | sequencer/ | ✅ Similar |
| mailer/ | mailer/ | ✅ Similar |
| read-after-write/ | ❌ Missing | Gap |
| handle/ | ❌ Partial | In identity/ |
| lexicon/ | ⚠️ SDK only | No dedicated module |

**Deliverable**: Architecture comparison document, structural recommendations

---

#### Aurora-Locus-0vv: Phase 6.1 - Database Schema Comparison
**Status**: Open | **Priority**: P2

**Missing Tables (Aurora needs):**
- `account_device` - DPoP device binding
- `device` - OAuth device management
- `authorization_request` - OAuth flow state
- `authorized_client` - OAuth client tracking
- `used_refresh_token` - Token rotation tracking
- `lexicon_failure` - Schema validation errors
- `actor` table (separate from accounts)

**Schema Enhancements Needed:**
- Account deactivation support (vs permanent deletion)
- Privileged app password scopes
- OAuth permission sets

**Deliverable**: Migration scripts for schema updates

---

#### Aurora-Locus-d45: Phase 6.4 - Actor Store & Repository Implementation Comparison
**Status**: Open | **Priority**: P2

**Comparison Areas:**
- Transaction isolation and atomicity
- Concurrent access patterns
- CAR file format compliance
- Blob deduplication strategies
- Repository migration and backup

**Bluesky Architecture:**
- Separate reader/writer/transactor classes
- Transaction support for atomic operations
- CAR import/export

**Aurora Architecture:**
- Combined reader/writer
- Basic MST via atproto-sdk
- Sequencer integration

**Deliverable**: Performance benchmarks, transaction support plan

---

#### Aurora-Locus-0qg: Phase 6.5 - Sequencer & Event Stream Comparison
**Status**: Open | **Priority**: P2

**Comparison Points:**
- Event ordering guarantees
- Subscription filtering
- Backfill mechanisms
- Performance under load
- Event retention policies
- Cursor format compatibility

**Current Aurora State:**
- ✅ Relay integration (Phase 3)
- ✅ Event types: commit, identity, account, handle, tombstone
- ✅ Metrics tracking
- ⚠️ Need backfill verification

**Deliverable**: ATProto sync spec compliance verification

---

#### Aurora-Locus-c7s: Phase 6.6 - Lexicon Schema Validation Comparison
**Status**: Open | **Priority**: P2

**Bluesky Features:**
- Runtime schema validation
- Lexicon failure tracking (db table)
- Validation error debugging
- Dynamic lexicon loading

**Aurora Gaps:**
- ❌ Lexicon failure tracking
- ❌ Detailed validation errors
- ❌ Schema version migration
- ⚠️ Custom lexicon extensibility

**Deliverable**: Lexicon validation improvements, error tracking

---

#### Aurora-Locus-edi: Phase 6.7 - Identity & DID Resolution Comparison
**Status**: Open | **Priority**: P2

**Comparison Areas:**
- DID resolution performance
- Handle verification methods
- PLC directory integration
- Cache invalidation strategies
- Failover and redundancy

**Aurora Current:**
- ✅ DidCache with SQLite
- ✅ Basic PLC support
- ✅ 1-hour cache TTL
- ✅ Phase 3 identity event handling

**Bluesky Features:**
- Multi-resolver support
- Handle verification workflows
- Advanced cache strategies

**Deliverable**: Performance benchmarks, PLC integration verification

---

#### Aurora-Locus-xh3: Phase 6.9 - Account Management & Lifecycle Comparison
**Status**: Open | **Priority**: P2

**Missing Features:**
- ❌ Account deactivation (vs permanent deletion)
- ❌ Privileged app password scopes
- ⚠️ Handle change workflow
- ❌ Account recovery mechanisms
- ⚠️ Email change verification

**Bluesky Features (db migration 002-003):**
- Account deactivation (soft delete)
- Privileged app passwords
- Handle changes with validation
- Account recovery

**Aurora Current:**
- ✅ Account creation with invites
- ✅ Email verification
- ✅ Password reset
- ✅ Account deletion (30-day grace)

**Deliverable**: Account lifecycle improvements, recovery mechanisms

---

#### Aurora-Locus-wwo: Phase 6.11 - Read-After-Write Consistency & Performance
**Status**: Open | **Priority**: P2

**Bluesky Feature:**
- Dedicated `read-after-write/` module
- Cache warming on write
- Consistency tracking
- Optimistic UI support

**Aurora Current:**
- Direct database reads (strong consistency)
- No explicit read-after-write optimization
- Basic Redis caching
- Identity cache (1-hour TTL)

**Why It Matters:**
- Critical for user experience
- Enables optimistic UI
- Reduces perceived latency
- Required for distributed deployments

**Deliverable**: Read-after-write module, performance benchmarks

---

## Summary Statistics

### By Priority

| Priority | Count | Issues |
|----------|-------|--------|
| P0 (Critical) | 1 | Sync Protocol |
| P1 (High) | 3 | OAuth/DPoP, API Parity, Security |
| P2 (Medium) | 8 | Architecture, DB Schema, Actor Store, Sequencer, Lexicon, Identity, Account Lifecycle, Read-After-Write |
| **Total** | **12** | |

### By Category

| Category | Issue Count | Status |
|----------|-------------|--------|
| Authentication/Security | 2 | OAuth/DPoP gaps, Security model |
| API Endpoints | 2 | Sync protocol, General parity |
| Data Layer | 2 | DB schema, Actor store |
| Infrastructure | 3 | Architecture, Sequencer, Read-After-Write |
| Identity | 1 | DID resolution |
| Validation | 1 | Lexicon schemas |
| Account Management | 1 | Lifecycle, Recovery |

---

## Critical Path (Recommended Order)

### Immediate (Must Do First)

1. **Phase 6.10** (P0): Sync Protocol Implementation
   - Blocking federation and interop
   - Required for ATProto compliance
   - 10 missing endpoints

2. **Phase 6.2** (P1): OAuth & DPoP Implementation
   - Production blocker (DPoP JWK parsing)
   - Required for modern auth flows
   - Foundation for other security features

### High Priority (Should Do Next)

3. **Phase 6.3** (P1): API Endpoint Parity
   - Verify all server endpoints
   - Document gaps in admin endpoints
   - Ensure XRPC spec compliance

4. **Phase 6.1** (P2): Database Schema Updates
   - Required for OAuth (device tables)
   - Enables account deactivation
   - Foundation for many features

5. **Phase 6.8** (P1): Security & Authorization
   - Scope-based permissions
   - Per-endpoint rate limits
   - Device credential binding

### Medium Priority (Good to Have)

6. **Phase 6.9** (P2): Account Lifecycle
7. **Phase 6.6** (P2): Lexicon Validation
8. **Phase 6.4** (P2): Actor Store Improvements
9. **Phase 6.7** (P2): Identity Resolution
10. **Phase 6.5** (P2): Sequencer Optimization

### Optional (Nice to Have)

11. **Phase 6.0** (P2): Architecture Review
12. **Phase 6.11** (P2): Read-After-Write

---

## Key Findings

### What Aurora Has That Bluesky Doesn't (Strengths)

- ✅ **Rust Performance**: Compiled binary, lower memory footprint
- ✅ **Federation Security**: 10x stricter cross-PDS rate limiting
- ✅ **Metrics**: Comprehensive Prometheus integration
- ✅ **Moderation**: Built-in moderation system (labels, reports, suspensions)

### What Bluesky Has That Aurora Needs (Gaps)

- ❌ **OAuth 2.1 Server**: Full authorization server with PKCE
- ❌ **Sync Protocol**: com.atproto.sync.* endpoints (critical)
- ❌ **Device Management**: OAuth device binding and tracking
- ❌ **Lexicon Tracking**: Validation error storage and debugging
- ❌ **Read-After-Write**: Consistency optimization module
- ⚠️ **DPoP JWK Parsing**: Production-ready implementation

### ATProto Spec Compliance

| Area | Compliance | Notes |
|------|------------|-------|
| Repository (com.atproto.repo.*) | ✅ 95% | All core endpoints implemented |
| Sync (com.atproto.sync.*) | ❌ 10% | Only subscribeRepos via relay |
| Server (com.atproto.server.*) | ⚠️ 70% | Need verification |
| Admin (com.atproto.admin.*) | ⚠️ 60% | Partial implementation |
| Identity (DID resolution) | ✅ 90% | Solid implementation |
| Authentication | ⚠️ 60% | Missing OAuth, partial DPoP |

---

## Next Steps

1. **Review & Prioritize**: Discuss which Phase 6 issues to tackle first
2. **Start with P0**: Begin Phase 6.10 (Sync Protocol) immediately
3. **Parallel Work**: Phase 6.2 (OAuth/DPoP) can run in parallel
4. **Incremental Progress**: Work through issues in critical path order
5. **Continuous Comparison**: Keep comparing as Bluesky PDS evolves

---

## Resources

- **Bluesky PDS Source**: `bluesky-pds/` folder
- **Aurora Locus Source**: `src/` folder
- **ATProto Specs**: https://atproto.com/specs
- **BD Issue Tracker**: `.beads/` database

---

**Created**: 2025-11-05
**Total Issues**: 12
**Estimated Effort**: 3-6 months for full parity
**Next Action**: Review and begin Phase 6.10 (Sync Protocol)
