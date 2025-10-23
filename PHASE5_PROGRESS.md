# Phase 5: Synchronization - IN PROGRESS 🚧

**Started**: October 22, 2025
**Status**: Core sequencer implemented, sync endpoints in progress

## Completed So Far ✅

### 1. Sequencer Database Schema
**File**: `migrations/20250104000001_sequencer.sql`

Created `repo_seq` table with:
- `seq` - Auto-incrementing primary key for global ordering
- `did` - Repository DID
- `event_type` - 'commit', 'identity', or 'account'
- `event` - CBOR-encoded event data (binary)
- `invalidated` - Flag for deleted/invalidated events
- `sequenced_at` - ISO 8601 timestamp

**Indexes**:
- `idx_repo_seq_did` - Filter by DID
- `idx_repo_seq_event_type` - Filter by event type
- `idx_repo_seq_sequenced_at` - Time-based queries
- `idx_repo_seq_seq_invalidated` - Efficient cursor queries

### 2. Event Type Definitions
**File**: `src/sequencer/events.rs`

Implemented ATProto event types:

#### CommitEvent
```rust
pub struct CommitEvent {
    pub rebase: bool,
    pub too_big: bool,
    pub repo: String,       // DID
    pub commit: String,     // CID of commit
    pub rev: String,        // Revision TID
    pub since: Option<String>, // Previous commit CID
    pub blocks: Vec<u8>,    // CAR file bytes
    pub ops: Vec<CommitOp>,
    pub blobs: Vec<String>,
    pub prev: Option<String>,
}
```

#### IdentityEvent
```rust
pub struct IdentityEvent {
    pub did: String,
    pub handle: Option<String>,
}
```

#### AccountEvent
```rust
pub struct AccountEvent {
    pub did: String,
    pub active: bool,
    pub status: Option<AccountStatus>, // Takendown, Suspended, Deleted, Deactivated
}
```

### 3. Sequencer Core Implementation
**File**: `src/sequencer/sequencer.rs`

**Key Features**:
- ✅ CBOR encoding of events for storage
- ✅ Insert events with monotonic sequence numbers
- ✅ Query current maximum sequence
- ✅ Get next event after cursor
- ✅ Request events in sequence range
- ✅ Filter events by DID
- ✅ Configurable query limits
- ✅ Comprehensive unit tests

**Methods**:
```rust
pub async fn sequence_commit(&self, evt: CommitEvent) -> PdsResult<i64>
pub async fn sequence_identity(&self, evt: IdentityEvent) -> PdsResult<i64>
pub async fn sequence_account(&self, evt: AccountEvent) -> PdsResult<i64>
pub async fn current_seq(&self) -> PdsResult<Option<i64>>
pub async fn next_event(&self, cursor: i64) -> PdsResult<Option<SeqRow>>
pub async fn request_seq_range(&self, earliest_seq, latest_seq, limit) -> PdsResult<Vec<SeqEvent>>
```

## Remaining Work 📋

### 1. Integration Tasks
- [ ] Add `sequencer` module to `main.rs`
- [ ] Initialize Sequencer in `AppContext`
- [ ] Integrate with RepositoryManager to record commits
- [ ] Generate CAR files from MST blocks for events

### 2. Sync Endpoints (com.atproto.sync.*)
- [ ] `getRepo` - Export full repository as CAR file
- [ ] `getLatestCommit` - Get latest commit info
- [ ] `getBlob` - Serve blob data by CID
- [ ] Create `src/api/sync.rs` module

### 3. WebSocket Firehose (Future)
- [ ] WebSocket handler setup
- [ ] `subscribeRepos` endpoint
- [ ] Outbox implementation (3-phase: backfill, cutover, streaming)
- [ ] Cursor validation and resume
- [ ] Backpressure handling

## Architecture Overview

```
┌─────────────────────────────────────────────────────────┐
│              Repository Operations                      │
│    (createRecord, putRecord, deleteRecord, etc.)       │
└────────────────────┬────────────────────────────────────┘
                     │
                     │ On every commit:
                     ▼
┌─────────────────────────────────────────────────────────┐
│              RepositoryManager                          │
│  1. Apply writes to MST                                │
│  2. Create signed commit                               │
│  3. Store blocks in repo_block                         │
│  4. Update repo_root                                   │
│  5. Generate CommitEvent ✨                            │
└────────────────────┬────────────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────────────┐
│                  Sequencer                              │
│  • CBOR-encode event                                   │
│  • INSERT into repo_seq                                │
│  • Return monotonic seq number                         │
│  • Notify listeners (future)                           │
└────────────────────┬────────────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────────────┐
│            Sequencer Database                           │
│              (repo_seq table)                           │
│                                                         │
│  seq | did | event_type | event (CBOR) | sequenced_at │
│  ────────────────────────────────────────────────────  │
│   1  | did1 | commit    | [binary]     | 2025-...    │
│   2  | did2 | commit    | [binary]     | 2025-...    │
│   3  | did1 | identity  | [binary]     | 2025-...    │
│   4  | did3 | commit    | [binary]     | 2025-...    │
│  ...                                                   │
└─────────────────────────────────────────────────────────┘
                     │
                     │ Consumed by:
                     ▼
┌─────────────────────────────────────────────────────────┐
│              Sync Endpoints                             │
│                                                         │
│  GET /xrpc/com.atproto.sync.getRepo                   │
│    → Returns CAR file for full repository             │
│                                                         │
│  GET /xrpc/com.atproto.sync.getLatestCommit           │
│    → Returns latest commit info                       │
│                                                         │
│  WS  /xrpc/com.atproto.sync.subscribeRepos            │
│    → Real-time event stream (firehose)                │
└─────────────────────────────────────────────────────────┘
```

## Key Design Decisions

### 1. CBOR Encoding
Events are CBOR-encoded before storage for:
- **Efficiency**: Binary format smaller than JSON
- **Type Safety**: Preserves type information
- **Compatibility**: Standard format in ATProto ecosystem

### 2. Global Sequence Numbers
- Single auto-incrementing sequence across ALL events
- Enables total ordering for federation
- Simple cursor-based resumption

### 3. Event Immutability
- Events are never updated, only invalidated
- `invalidated` flag marks deleted events
- Preserves audit trail

### 4. Time Indexing
- `sequenced_at` indexed for time-range queries
- ISO 8601 format for compatibility
- Not strictly ordered (multiple events can share timestamp)

## Testing Status

✅ Unit tests implemented for:
- Event sequencing (commit, identity, account)
- Current seq tracking
- Sequence range queries
- Multiple event insertion

## Dependencies

All required dependencies already present:
- ✅ `serde_cbor` - For CBOR encoding (from SDK)
- ✅ `sqlx` - Database operations
- ✅ `chrono` - Timestamp handling
- ✅ `tokio` - Async runtime

## Next Immediate Steps

1. **Add sequencer to main.rs**:
   ```rust
   mod sequencer;
   ```

2. **Initialize in AppContext**:
   ```rust
   let sequencer = Arc::new(Sequencer::new(account_db.clone(), SequencerConfig::default()));
   ```

3. **Integrate with RepositoryManager**:
   - After successful commit, create CommitEvent
   - Extract CAR file from MST blocks
   - Call `sequencer.sequence_commit(event)`

4. **Create sync endpoints**:
   - Implement `getRepo` (CAR export)
   - Implement `getLatestCommit` (commit info)
   - Implement `getBlob` (blob serving)

## Notes

- WebSocket firehose (`subscribeRepos`) is complex and can be deferred
- Focus on HTTP sync endpoints first for basic federation
- CAR file generation will reuse SDK's existing CAR writer
- Blob serving is straightforward (already have BlobStore)

## Session Context

This work is being done in a continuing session. The Aurora Locus PDS has:
- ✅ Phase 1: Foundation (HTTP server, config)
- ✅ Phase 2: Account System (auth, sessions)
- ✅ Phase 3: Repository Operations (records, MST)
- ✅ Phase 4: Blob Storage (media uploads)
- 🚧 Phase 5: Synchronization (federation) - IN PROGRESS

Total endpoints so far: 14
Expected after Phase 5: 17+ (3 new sync endpoints)

Server is currently running on PID 23340 (multiple background processes).
