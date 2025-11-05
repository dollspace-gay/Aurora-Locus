# Phase 6.10: Sync Protocol Implementation Architecture

**Issue**: Aurora-Locus-ii8
**Priority**: P0 (Critical)
**Status**: Design Phase

---

## Executive Summary

This document outlines the architecture for implementing ATProto's sync protocol endpoints in Aurora Locus PDS. The sync protocol is **critical for federation**, enabling external services (AppViews, relays, crawlers) to synchronize repository data.

### Scope

Implement 10 missing `com.atproto.sync.*` endpoints to achieve ATProto spec compliance:

1. ✅ `subscribeRepos` - Already implemented (Phase 3)
2. ❌ `getLatestCommit` - Get latest commit CID/rev
3. ❌ `getBlob` - Fetch blob by CID
4. ❌ `getBlocks` - Get repository blocks
5. ❌ `getRepo` - Export full repo as CAR
6. ❌ `getRecord` - Get record at specific commit
7. ❌ `listBlobs` - List all blob CIDs
8. ❌ `listRepos` - List all repositories on PDS
9. ❌ `getRepoStatus` - Get repo availability status
10. 🗑️ `getCheckout` - Deprecated (Bluesky PDS has it marked)
11. 🗑️ `getHead` - Deprecated (replaced by getLatestCommit)

**Estimated Effort**: 3-5 days
**Blockers**: None (all infrastructure in place)

---

## Bluesky PDS Analysis

### Implementation Patterns Observed

All Bluesky sync endpoints follow consistent patterns:

#### 1. Authorization Pattern
```typescript
auth: ctx.authVerifier.authorizationOrAdminTokenOptional({
  additional: [AuthScope.Takendown],  // Optional
  authorize: () => {
    // always allow (sync endpoints are public)
  },
})
```

**Key Insight**: Sync endpoints are **publicly accessible** but may include takendown content for admins/owners.

#### 2. Repository Availability Check
```typescript
await assertRepoAvailability(ctx, did, isUserOrAdmin(auth, did))
```

This helper function:
- Checks if account exists
- Verifies account is not takendown (unless admin/owner)
- Verifies account is not deactivated (unless admin/owner)
- Returns error codes: `RepoNotFound`, `RepoTakendown`, `RepoDeactivated`

#### 3. Actor Store Access Pattern
```typescript
const result = await ctx.actorStore.read(did, (store) => {
  return store.repo.storage.getSomething()
})
```

**Aurora Equivalent**: Our `ActorStore` already supports this pattern.

#### 4. CAR Streaming Pattern
```typescript
// For blocks
const car = blocksToCarStream(null, got.blocks)
return {
  encoding: 'application/vnd.ipld.car',
  body: byteIterableToStream(car),
}

// For full repo
const carStream = byteIterableToStream(await storage.getCarStream(since))
return {
  encoding: 'application/vnd.ipld.car',
  body: carStream,
}
```

**Challenge**: We need CAR export functionality.

---

## Aurora Locus Current State

### ✅ What We Have

#### Actor Store Infrastructure ([src/actor_store/](src/actor_store/))

```rust
// Already implemented:
pub async fn get_repo_root(&self, did: &str) -> PdsResult<RepoRoot>
pub async fn get_block(&self, did: &str, cid: &str) -> PdsResult<Option<Vec<u8>>>
pub async fn get_blocks_by_cids(&self, did: &str, cids: &[String]) -> PdsResult<Vec<(String, Vec<u8>)>>
pub async fn get_all_blocks(&self, did: &str) -> PdsResult<Vec<(String, Vec<u8>)>>
```

**Database Schema** (per-actor SQLite):
```sql
CREATE TABLE repo_root (
    did TEXT PRIMARY KEY,
    cid TEXT NOT NULL,        -- Latest commit CID
    rev TEXT NOT NULL,        -- Revision (TID)
    indexed_at DATETIME
);

CREATE TABLE repo_block (
    cid TEXT PRIMARY KEY,
    content BLOB NOT NULL,    -- Raw block data
    indexed_at DATETIME
);
```

#### Blob Store Infrastructure ([src/blob_store/](src/blob_store/))

**Database Schema** (global SQLite):
```sql
CREATE TABLE blob_metadata (
    cid TEXT PRIMARY KEY,
    mime_type TEXT NOT NULL,
    size INTEGER NOT NULL,
    creator_did TEXT NOT NULL,  -- ✅ Can query by DID!
    created_at DATETIME,
    width INTEGER,
    height INTEGER,
    thumbnail_cid TEXT
);
```

#### SDK Integration
- ✅ `atproto::repo::Repository` - In-memory MST
- ✅ Block storage/retrieval
- ✅ CID handling

### ❌ What We're Missing

1. **CAR Export Functionality**
   - Need to convert blocks to CAR format
   - Need streaming CAR generation
   - SDK likely has utilities we can use

2. **Blob Listing by DID**
   - Have the data (`creator_did` in blob_metadata)
   - Need query method: `list_blobs_by_did()`

3. **Sync API Module**
   - No `src/api/sync.rs` exists yet
   - Need to create from scratch

4. **Repository Availability Helper**
   - Need Rust equivalent of `assertRepoAvailability`
   - Check account status, deactivation, takedown

---

## Architecture Design

### Module Structure

```
src/api/sync.rs                 # New sync endpoints module
src/actor_store/car.rs          # New CAR export utilities
src/api/sync_helpers.rs         # Shared helpers (availability checks)
```

### 1. CAR Export Module (`src/actor_store/car.rs`)

**Purpose**: Convert repository blocks to CAR (Content Addressable aRchive) format.

**Key Functions**:

```rust
/// Export all blocks as CAR stream
pub async fn export_repo_to_car(
    store: &ActorStore,
    did: &str,
    since: Option<&str>,  // For incremental sync
) -> PdsResult<Vec<u8>>;

/// Export specific blocks as CAR
pub async fn blocks_to_car(
    blocks: Vec<(String, Vec<u8>)>,
    root: Option<&str>,
) -> PdsResult<Vec<u8>>;

/// Export single record as CAR
pub async fn export_record_to_car(
    store: &ActorStore,
    did: &str,
    collection: &str,
    rkey: &str,
) -> PdsResult<Vec<u8>>;
```

**Implementation Strategy**:
- Check if `atproto` crate has CAR utilities (likely in `atproto::car` module)
- If not, use `iroh-car` or `rs-car` crate
- CAR format: Simple header + sequence of (CID, Block) pairs

**CAR Format Reference**:
```
CARv1 Header: {roots: [CID], version: 1}
Block 1: varint(cid_len) + CID + varint(block_len) + Block
Block 2: ...
```

### 2. Blob Listing Extension (`src/blob_store/store.rs`)

Add method to existing `BlobStore`:

```rust
impl BlobStore {
    /// List blobs for a specific DID
    pub async fn list_blobs_by_did(
        &self,
        did: &str,
        since: Option<&str>,
        limit: i64,
        cursor: Option<&str>,
    ) -> PdsResult<Vec<String>> {
        let query = if let Some(cursor) = cursor {
            sqlx::query_scalar(
                "SELECT cid FROM blob_metadata
                 WHERE creator_did = ?1 AND cid > ?2
                 ORDER BY cid ASC
                 LIMIT ?3"
            )
            .bind(did)
            .bind(cursor)
            .bind(limit)
        } else {
            sqlx::query_scalar(
                "SELECT cid FROM blob_metadata
                 WHERE creator_did = ?1
                 ORDER BY cid ASC
                 LIMIT ?2"
            )
            .bind(did)
            .bind(limit)
        };

        Ok(query.fetch_all(&self.db).await?)
    }
}
```

### 3. Repository Availability Helper (`src/api/sync_helpers.rs`)

```rust
use crate::{
    account::AccountManager,
    error::{PdsError, PdsResult},
};

/// Check if repository is available for sync access
///
/// - Returns Ok(()) if repo is accessible
/// - Returns Err(RepoNotFound) if account doesn't exist
/// - Returns Err(RepoTakendown) if takendown (unless is_admin_or_self)
/// - Returns Err(RepoDeactivated) if deactivated (unless is_admin_or_self)
pub async fn assert_repo_availability(
    account_manager: &AccountManager,
    did: &str,
    is_admin_or_self: bool,
) -> PdsResult<()> {
    let account = account_manager
        .get_account_by_did(did)
        .await?
        .ok_or_else(|| {
            PdsError::NotFound(format!("Could not find repo for DID: {}", did))
        })?;

    // Admins and owners can access any repo
    if is_admin_or_self {
        return Ok(());
    }

    // Check if account is takendown
    if account.takedown_ref.is_some() {
        return Err(PdsError::Validation(format!(
            "Repo has been takendown: {}",
            did
        )));
    }

    // Check if account is deactivated
    if account.deactivated_at.is_some() {
        return Err(PdsError::Validation(format!(
            "Repo has been deactivated: {}",
            did
        )));
    }

    Ok(())
}
```

### 4. Sync Endpoints Module (`src/api/sync.rs`)

**Structure**:

```rust
use crate::{
    actor_store::{car, ActorStore},
    api::sync_helpers::assert_repo_availability,
    auth::AuthResult,
    context::AppContext,
    error::{PdsError, PdsResult},
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};

/// Mount sync routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        .route("/xrpc/com.atproto.sync.getLatestCommit", get(get_latest_commit))
        .route("/xrpc/com.atproto.sync.getBlob", get(get_blob))
        .route("/xrpc/com.atproto.sync.getBlocks", get(get_blocks))
        .route("/xrpc/com.atproto.sync.getRepo", get(get_repo))
        .route("/xrpc/com.atproto.sync.getRecord", get(get_record))
        .route("/xrpc/com.atproto.sync.listBlobs", get(list_blobs))
        .route("/xrpc/com.atproto.sync.listRepos", get(list_repos))
        .route("/xrpc/com.atproto.sync.getRepoStatus", get(get_repo_status))
}

// Individual endpoint implementations below...
```

---

## Endpoint Implementation Details

### 1. `getLatestCommit` - EASY ✅

**Request**: `GET /xrpc/com.atproto.sync.getLatestCommit?did={did}`

**Response**:
```json
{
  "cid": "bafyreihk5ztsfapt6g2cnxbxgbxb7dltipq5pufb4jtwmqrxrxqaygceyq",
  "rev": "3jzfcijpj2z2a"
}
```

**Implementation**:
```rust
#[derive(Deserialize)]
struct GetLatestCommitParams {
    did: String,
}

#[derive(Serialize)]
struct GetLatestCommitResponse {
    cid: String,
    rev: String,
}

async fn get_latest_commit(
    State(ctx): State<AppContext>,
    Query(params): Query<GetLatestCommitParams>,
    auth: AuthResult,
) -> PdsResult<Json<GetLatestCommitResponse>> {
    let is_admin_or_self = auth.is_admin() || auth.did() == Some(&params.did);
    assert_repo_availability(&ctx.account_manager, &params.did, is_admin_or_self).await?;

    let root = ctx.actor_store.get_repo_root(&params.did).await?;

    Ok(Json(GetLatestCommitResponse {
        cid: root.cid,
        rev: root.rev,
    }))
}
```

**Effort**: 30 minutes

---

### 2. `getBlob` - EASY ✅

**Request**: `GET /xrpc/com.atproto.sync.getBlob?did={did}&cid={cid}`

**Response**: Raw blob bytes with headers

**Implementation**:
```rust
#[derive(Deserialize)]
struct GetBlobParams {
    did: String,
    cid: String,
}

async fn get_blob(
    State(ctx): State<AppContext>,
    Query(params): Query<GetBlobParams>,
    auth: AuthResult,
) -> PdsResult<Response> {
    let is_admin_or_self = auth.is_admin() || auth.did() == Some(&params.did);
    assert_repo_availability(&ctx.account_manager, &params.did, is_admin_or_self).await?;

    // Get blob from blob store
    let (data, mime_type) = ctx
        .blob_store
        .get(&params.cid)
        .await?
        .ok_or_else(|| PdsError::NotFound("Blob not found".to_string()))?;

    // Security headers (mimic Bluesky)
    Ok((
        StatusCode::OK,
        [
            ("content-type", mime_type.as_str()),
            ("content-length", &data.len().to_string()),
            ("x-content-type-options", "nosniff"),
            ("content-security-policy", "default-src 'none'; sandbox"),
        ],
        data,
    )
        .into_response())
}
```

**Effort**: 30 minutes

---

### 3. `getBlocks` - MEDIUM 🔶

**Request**: `GET /xrpc/com.atproto.sync.getBlocks?did={did}&cids={cid1}&cids={cid2}`

**Response**: CAR file (application/vnd.ipld.car)

**Implementation**:
```rust
#[derive(Deserialize)]
struct GetBlocksParams {
    did: String,
    cids: Vec<String>,  // Multiple cids in query params
}

async fn get_blocks(
    State(ctx): State<AppContext>,
    Query(params): Query<GetBlocksParams>,
    auth: AuthResult,
) -> PdsResult<Response> {
    let is_admin_or_self = auth.is_admin() || auth.did() == Some(&params.did);
    assert_repo_availability(&ctx.account_manager, &params.did, is_admin_or_self).await?;

    // Get blocks
    let blocks = ctx.actor_store.get_blocks_by_cids(&params.did, &params.cids).await?;

    // Check if any blocks are missing
    if blocks.len() != params.cids.len() {
        let found_cids: Vec<&str> = blocks.iter().map(|(cid, _)| cid.as_str()).collect();
        let missing: Vec<&str> = params.cids.iter()
            .filter(|cid| !found_cids.contains(&cid.as_str()))
            .map(|s| s.as_str())
            .collect();
        return Err(PdsError::NotFound(format!("Could not find cids: {:?}", missing)));
    }

    // Convert to CAR
    let car_bytes = car::blocks_to_car(blocks, None).await?;

    Ok((
        StatusCode::OK,
        [("content-type", "application/vnd.ipld.car")],
        car_bytes,
    )
        .into_response())
}
```

**Effort**: 2 hours (includes CAR utility implementation)

---

### 4. `getRepo` - HARD 🔴

**Request**: `GET /xrpc/com.atproto.sync.getRepo?did={did}&since={rev}`

**Response**: CAR file with full repository (or incremental if `since` provided)

**Implementation**:
```rust
#[derive(Deserialize)]
struct GetRepoParams {
    did: String,
    since: Option<String>,  // For incremental sync
}

async fn get_repo(
    State(ctx): State<AppContext>,
    Query(params): Query<GetRepoParams>,
    auth: AuthResult,
) -> PdsResult<Response> {
    let is_admin_or_self = auth.is_admin() || auth.did() == Some(&params.did);
    assert_repo_availability(&ctx.account_manager, &params.did, is_admin_or_self).await?;

    // Export full repo as CAR
    let car_bytes = car::export_repo_to_car(
        &ctx.actor_store,
        &params.did,
        params.since.as_deref(),
    )
    .await?;

    Ok((
        StatusCode::OK,
        [("content-type", "application/vnd.ipld.car")],
        car_bytes,
    )
        .into_response())
}
```

**Challenge**: Incremental sync (if `since` is provided, only export blocks newer than that revision).

**Effort**: 3-4 hours

---

### 5. `getRecord` - HARD 🔴

**Request**: `GET /xrpc/com.atproto.sync.getRecord?did={did}&collection={collection}&rkey={rkey}`

**Response**: CAR file with specific record

**Implementation**:
```rust
#[derive(Deserialize)]
struct GetRecordParams {
    did: String,
    collection: String,
    rkey: String,
}

async fn get_record(
    State(ctx): State<AppContext>,
    Query(params): Query<GetRecordParams>,
    auth: AuthResult,
) -> PdsResult<Response> {
    let is_admin_or_self = auth.is_admin() || auth.did() == Some(&params.did);
    assert_repo_availability(&ctx.account_manager, &params.did, is_admin_or_self).await?;

    // Export specific record as CAR
    let car_bytes = car::export_record_to_car(
        &ctx.actor_store,
        &params.did,
        &params.collection,
        &params.rkey,
    )
    .await?;

    Ok((
        StatusCode::OK,
        [("content-type", "application/vnd.ipld.car")],
        car_bytes,
    )
        .into_response())
}
```

**Challenge**: Need to traverse MST to find record blocks.

**Effort**: 3-4 hours

---

### 6. `listBlobs` - EASY ✅

**Request**: `GET /xrpc/com.atproto.sync.listBlobs?did={did}&limit={limit}&cursor={cursor}`

**Response**:
```json
{
  "cursor": "bafyreiabc...",
  "cids": [
    "bafyreiabc...",
    "bafyreidef..."
  ]
}
```

**Implementation**:
```rust
#[derive(Deserialize)]
struct ListBlobsParams {
    did: String,
    since: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
}

#[derive(Serialize)]
struct ListBlobsResponse {
    cursor: Option<String>,
    cids: Vec<String>,
}

async fn list_blobs(
    State(ctx): State<AppContext>,
    Query(params): Query<ListBlobsParams>,
    auth: AuthResult,
) -> PdsResult<Json<ListBlobsResponse>> {
    let is_admin_or_self = auth.is_admin() || auth.did() == Some(&params.did);
    assert_repo_availability(&ctx.account_manager, &params.did, is_admin_or_self).await?;

    let limit = params.limit.unwrap_or(100).min(1000);

    let cids = ctx
        .blob_store
        .list_blobs_by_did(&params.did, params.since.as_deref(), limit, params.cursor.as_deref())
        .await?;

    let cursor = cids.last().cloned();

    Ok(Json(ListBlobsResponse { cursor, cids }))
}
```

**Effort**: 1 hour (includes blob listing method)

---

### 7. `listRepos` - MEDIUM 🔶

**Request**: `GET /xrpc/com.atproto.sync.listRepos?limit={limit}&cursor={cursor}`

**Response**:
```json
{
  "cursor": "...",
  "repos": [
    {
      "did": "did:plc:abc123",
      "head": "bafyreihk5...",
      "rev": "3jzfcijpj2z2a",
      "active": true,
      "status": null
    }
  ]
}
```

**Implementation**:
```rust
#[derive(Deserialize)]
struct ListReposParams {
    limit: Option<i64>,
    cursor: Option<String>,
}

#[derive(Serialize)]
struct RepoInfo {
    did: String,
    head: String,
    rev: String,
    active: bool,
    status: Option<String>,
}

#[derive(Serialize)]
struct ListReposResponse {
    cursor: Option<String>,
    repos: Vec<RepoInfo>,
}

async fn list_repos(
    State(ctx): State<AppContext>,
    Query(params): Query<ListReposParams>,
) -> PdsResult<Json<ListReposResponse>> {
    let limit = params.limit.unwrap_or(100).min(1000);

    // Query accounts with repo roots
    let accounts = ctx
        .account_manager
        .list_accounts(limit, params.cursor.as_deref())
        .await?;

    let mut repos = Vec::new();
    for account in accounts {
        // Get repo root for each account
        if let Ok(root) = ctx.actor_store.get_repo_root(&account.did).await {
            let active = account.deactivated_at.is_none() && account.takedown_ref.is_none();
            let status = if account.takedown_ref.is_some() {
                Some("takendown".to_string())
            } else if account.deactivated_at.is_some() {
                Some("deactivated".to_string())
            } else {
                None
            };

            repos.push(RepoInfo {
                did: account.did.clone(),
                head: root.cid,
                rev: root.rev,
                active,
                status,
            });
        }
    }

    let cursor = repos.last().map(|r| r.did.clone());

    Ok(Json(ListReposResponse { cursor, repos }))
}
```

**Effort**: 2 hours

---

### 8. `getRepoStatus` - EASY ✅

**Request**: `GET /xrpc/com.atproto.sync.getRepoStatus?did={did}`

**Response**:
```json
{
  "did": "did:plc:abc123",
  "active": true,
  "status": null,
  "rev": "3jzfcijpj2z2a"
}
```

**Implementation**:
```rust
#[derive(Deserialize)]
struct GetRepoStatusParams {
    did: String,
}

#[derive(Serialize)]
struct GetRepoStatusResponse {
    did: String,
    active: bool,
    status: Option<String>,
    rev: Option<String>,
}

async fn get_repo_status(
    State(ctx): State<AppContext>,
    Query(params): Query<GetRepoStatusParams>,
) -> PdsResult<Json<GetRepoStatusResponse>> {
    // No auth required - this is public info about repo availability

    let account = ctx
        .account_manager
        .get_account_by_did(&params.did)
        .await?
        .ok_or_else(|| PdsError::NotFound(format!("Could not find repo for DID: {}", params.did)))?;

    let active = account.deactivated_at.is_none() && account.takedown_ref.is_none();
    let status = if account.takedown_ref.is_some() {
        Some("takendown".to_string())
    } else if account.deactivated_at.is_some() {
        Some("deactivated".to_string())
    } else {
        None
    };

    let rev = if active {
        ctx.actor_store
            .get_repo_root(&params.did)
            .await
            .ok()
            .map(|root| root.rev)
    } else {
        None
    };

    Ok(Json(GetRepoStatusResponse {
        did: params.did,
        active,
        status,
        rev,
    }))
}
```

**Effort**: 30 minutes

---

## Dependencies & Crate Research

### CAR Format Support

**Option 1: `atproto` crate** (PREFERRED)
- Check if `atproto::car` module exists
- Likely has `write_car`, `read_car` functions
- Already integrated in our project

**Option 2: `iroh-car` crate**
```toml
[dependencies]
iroh-car = "0.4"
```
- Mature CAR implementation
- Used by IPFS Rust ecosystem

**Option 3: Manual Implementation**
- CAR format is relatively simple
- Could implement ourselves if needed
- ~200 lines of code

**Decision**: Research `atproto` crate first, fall back to `iroh-car`.

---

## Testing Strategy

### Unit Tests

For each endpoint:
1. Test successful case
2. Test repo not found
3. Test takendown repo (should fail for non-admins)
4. Test deactivated repo (should fail for non-admins)
5. Test admin override

### Integration Tests

1. **CAR Export Validation**
   - Export repo as CAR
   - Import into clean repo
   - Verify identical state

2. **Incremental Sync**
   - Export repo at commit A
   - Make changes
   - Export with `since=A`
   - Verify only new blocks included

3. **Blob Listing**
   - Upload blobs for DID
   - List blobs
   - Verify all CIDs returned

### External Validation

Test with actual ATProto tools:
- `@atproto/api` client library
- PDS crawlers
- AppView indexers

---

## Implementation Plan

### Phase 1: Foundation (Day 1)
- [ ] Research CAR support in `atproto` crate
- [ ] Implement `src/actor_store/car.rs` module
  - [ ] `blocks_to_car()` - basic CAR generation
  - [ ] Unit tests for CAR format
- [ ] Add `list_blobs_by_did()` to `BlobStore`
- [ ] Create `src/api/sync_helpers.rs`
  - [ ] `assert_repo_availability()`

### Phase 2: Easy Endpoints (Day 1-2)
- [ ] Implement `getLatestCommit`
- [ ] Implement `getBlob`
- [ ] Implement `getRepoStatus`
- [ ] Implement `listBlobs`
- [ ] Write unit tests

### Phase 3: Medium Endpoints (Day 2-3)
- [ ] Implement `getBlocks`
- [ ] Implement `listRepos`
- [ ] Implement full CAR export in `car.rs`
  - [ ] `export_repo_to_car()`
- [ ] Write integration tests

### Phase 4: Hard Endpoints (Day 3-4)
- [ ] Implement `getRepo`
  - [ ] Support incremental sync (`since` parameter)
- [ ] Implement `getRecord`
  - [ ] MST traversal for specific record
- [ ] Write comprehensive tests

### Phase 5: Integration & Testing (Day 5)
- [ ] Wire up all routes in `src/api/mod.rs`
- [ ] Test with external ATProto clients
- [ ] Test with PDS crawler tools
- [ ] Update API documentation
- [ ] Update PHASE_6_COMPARISON_PLAN.md

---

## Success Criteria

### Functional Requirements
- ✅ All 8 sync endpoints implemented (excluding deprecated ones)
- ✅ CAR export working correctly
- ✅ Blob listing by DID functional
- ✅ Repository availability checks working
- ✅ Authorization working (public + admin override)

### Quality Requirements
- ✅ 0 compilation errors
- ✅ All unit tests passing
- ✅ Integration tests with external tools passing
- ✅ CAR files validated by ATProto tools
- ✅ Performance: repo export <1s for small repos (<10MB)

### Documentation Requirements
- ✅ API documentation updated
- ✅ Inline code comments
- ✅ Architecture document (this file)
- ✅ Update SECURITY.md if needed
- ✅ Close BD issue Aurora-Locus-ii8

---

## Risk Assessment

### Low Risk ✅
- Basic endpoints (`getLatestCommit`, `getBlob`, `listBlobs`, `getRepoStatus`)
- Existing infrastructure sufficient

### Medium Risk 🔶
- CAR export implementation
- **Mitigation**: Use well-tested library (`atproto` or `iroh-car`)

### High Risk 🔴
- Incremental sync (`since` parameter in `getRepo`)
- Record-specific export (`getRecord`)
- **Mitigation**: Phase these for last, test thoroughly

---

## Open Questions

1. **CAR Streaming**: Should we stream CAR files or buffer in memory?
   - **Decision**: Buffer for MVP, optimize later if needed

2. **Blob Access**: Should sync endpoints bypass blob authorization?
   - **Decision**: Yes (follow Bluesky pattern - repos are public)

3. **Rate Limiting**: Should sync endpoints have special rate limits?
   - **Decision**: Yes - use cross-PDS rate limits (10 req/s)

4. **Caching**: Should we cache CAR exports?
   - **Decision**: No for MVP - implement in Phase 6.11 (Read-After-Write)

---

## References

- **ATProto Sync Spec**: https://atproto.com/specs/sync
- **Bluesky PDS Implementation**: `bluesky-pds/src/api/com/atproto/sync/`
- **CAR Format Spec**: https://ipld.io/specs/transport/car/
- **Phase 6 Comparison Plan**: [PHASE_6_COMPARISON_PLAN.md](PHASE_6_COMPARISON_PLAN.md)

---

**Status**: Ready for implementation
**Next Step**: Begin Phase 1 (Foundation) - CAR utilities research
**Estimated Completion**: 3-5 days from start
