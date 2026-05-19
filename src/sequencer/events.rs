/// Event type definitions for the sequencer
use serde::{Deserialize, Serialize};

/// Commit event - emitted when repository data changes.
///
/// Arc 14 §7.3.2: `prev_data` is the prior commit's MST root CID
/// (extracted from the prior signed commit's `data` field). It is
/// the inductive-firehose linkage and is `None` only for genesis
/// commits (the first commit for a repo).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitEvent {
    pub rebase: bool,
    pub too_big: bool,
    pub repo: String,          // DID
    pub commit: String,        // CID of commit
    pub rev: String,           // Revision TID
    pub since: Option<String>, // Previous commit CID
    pub blocks: Vec<u8>,       // CAR file bytes
    pub ops: Vec<CommitOp>,
    pub blobs: Vec<String>, // CIDs of blobs (deprecated but included)
    /// Prior commit's MST root CID. `None` for genesis commits.
    /// Per Arc 14 §7.3.2, omitted from the wire when None.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_data: Option<String>,
}

/// Operation within a commit.
///
/// Arc 14 §7.3.2: `prev` is the prior record version's CID. `None`
/// for `create` ops (no prior version exists). For delete ops, `cid`
/// is `None` AND wire-emitted as CBOR null (lexicon `nullable`
/// discipline) — see [`crate::api::firehose_encoder::commit_op_to_lex_value`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitOp {
    pub action: OpAction,
    pub path: String,        // collection/rkey
    pub cid: Option<String>, // CID of record (null for delete)
    /// Prior record version CID. `None` for `create` ops.
    /// Per Arc 14 §7.3.2, omitted from the wire when None.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev: Option<String>,
}

/// Operation action type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OpAction {
    Create,
    Update,
    Delete,
}

/// Sync event - lightweight repo state sync (for account creation/activation)
///
/// This is a simpler alternative to CommitEvent used when there's no meaningful
/// diff to show (e.g., account creation with empty repo, account activation after
/// deactivation). Contains only the commit block, not all repo operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncEvent {
    pub did: String,
    pub rev: String,     // Revision TID
    pub blocks: Vec<u8>, // CAR file with only commit block
}

/// Identity event - emitted when handle changes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityEvent {
    pub did: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
}

/// Account event - emitted when account status changes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountEvent {
    pub did: String,
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<AccountStatus>,
}

/// Account status — the 6-value lexicon `knownValues` vocabulary
/// per Arc 14 §7.3.7. Wire-emission segregated per §7.1.2:
///
/// - **Wire-emittable from production sources in v0.5**: `Takendown`,
///   `Deactivated` (2).
/// - **Wire-emittable from test-affordance direct DB writes in v0.5**:
///   above + `Suspended`, `Desynchronized` (4).
/// - **Enum-extended but not wire-emittable in v0.5**: `Throttled`
///   (no column source); `Deleted` (Arc 15 producer scope).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AccountStatus {
    Takendown,
    Suspended,
    Deleted,
    Deactivated,
    Desynchronized,
    Throttled,
}

impl CommitEvent {
    /// Create a new commit event.
    ///
    /// Arc 14 §7.3.2: `prev_data` is the prior commit's MST root CID
    /// (`None` for genesis commits). Per-op `prev` (prior record
    /// version CID) is set on the `CommitOp` values directly.
    pub fn new(
        repo: String,
        commit: String,
        rev: String,
        since: Option<String>,
        prev_data: Option<String>,
        blocks: Vec<u8>,
        ops: Vec<CommitOp>,
    ) -> Self {
        Self {
            rebase: false,
            too_big: false,
            repo,
            commit,
            rev,
            since,
            blocks,
            ops,
            blobs: Vec::new(),
            prev_data,
        }
    }
}

impl SyncEvent {
    /// Create a new sync event
    #[allow(dead_code)] // Will be used when implementing sync events
    pub fn new(did: String, rev: String, blocks: Vec<u8>) -> Self {
        Self { did, rev, blocks }
    }
}

impl IdentityEvent {
    /// Create a new identity event
    pub fn new(did: String, handle: Option<String>) -> Self {
        Self { did, handle }
    }
}

impl AccountEvent {
    /// Create a new account event
    #[allow(dead_code)] // Will be used when implementing account events
    pub fn new(did: String, active: bool, status: Option<AccountStatus>) -> Self {
        Self {
            did,
            active,
            status,
        }
    }
}
