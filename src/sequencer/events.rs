/// Event type definitions for the sequencer
use crate::error::{PdsError, PdsResult};
use proto_blue::lex_data::Cid;
use proto_blue::repo::{BlockMap, CommitData, blocks_to_car};
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

    /// Arc 15 §8.3.1 — invariant-enforcing builder used at handler
    /// sites where the status is hardcoded by handler semantics
    /// (no row to read; "Pattern A" per §8.3.4 selection rule).
    ///
    /// Sets `active = false` for any non-Active status; sets
    /// `status = None` (wire-absent) for Active. Direct construction
    /// of `AccountEvent` with mismatched `active` + `status` is still
    /// permitted at sites that derive both from a row read (Pattern
    /// B), but `from_status` is the canonical path for hardcoded sites
    /// (`delete_account`, `create_account`).
    pub fn from_status(did: String, status: AccountStatus) -> Self {
        Self {
            did,
            active: false,
            status: Some(status),
        }
    }

    /// Arc 15 §8.3.1 — `active = true`, `status = None` (wire-absent).
    /// Used at `create_account` per §8.3.8.
    pub fn active(did: String) -> Self {
        Self {
            did,
            active: true,
            status: None,
        }
    }
}

/// Arc 15 §8.2.1 + §8.3.9 — minimal projection of a `CommitData`
/// suitable for the `#sync` wire frame. Carries the current commit
/// CID, the revision string, and a minimal `BlockMap` slice (commit
/// block + its MST root block). Distinct from `CommitEvent` which
/// carries the full per-write block set.
#[derive(Debug, Clone)]
pub struct SyncEvtData {
    pub cid: Cid,
    pub rev: String,
    pub blocks: BlockMap,
}

impl SyncEvent {
    /// Arc 15 §8.3.9 — formatter from the recon-resolved
    /// `SyncEvtData` to a wire-encodable `SyncEvent`. CAR-encodes the
    /// minimal block slice with the commit CID as root.
    pub fn from_sync_data(did: String, data: SyncEvtData) -> PdsResult<Self> {
        let blocks = blocks_to_car(Some(&data.cid), &data.blocks)
            .map_err(|e| PdsError::Internal(format!("sync CAR export failed: {}", e)))?;
        Ok(Self {
            did,
            rev: data.rev,
            blocks,
        })
    }
}

/// Arc 15 §8.3.9 — projection helper: distill proto-blue's
/// `CommitData` (which carries every new block for the commit) down
/// to the minimal slice the `#sync` frame needs: the signed commit
/// block + its MST root block. Errors if either expected block is
/// missing from the `CommitData.blocks` map (shouldn't happen for a
/// well-formed `CommitData` returned by `Repo::apply_writes` or
/// `Repo::create`).
pub fn sync_evt_data_from_commit(commit_data: &CommitData) -> PdsResult<SyncEvtData> {
    let mut minimal = BlockMap::new();

    let commit_bytes = commit_data
        .blocks
        .get(&commit_data.commit_cid)
        .ok_or_else(|| {
            PdsError::Internal(format!(
                "commit block CID {} missing from CommitData.blocks",
                commit_data.commit_cid
            ))
        })?
        .to_vec();
    minimal.set(commit_data.commit_cid.clone(), commit_bytes);

    let mst_root = commit_data.commit.data.clone();
    let mst_bytes = commit_data
        .blocks
        .get(&mst_root)
        .ok_or_else(|| {
            PdsError::Internal(format!(
                "MST root block CID {} missing from CommitData.blocks",
                mst_root
            ))
        })?
        .to_vec();
    minimal.set(mst_root, mst_bytes);

    Ok(SyncEvtData {
        cid: commit_data.commit_cid.clone(),
        rev: commit_data.commit.rev.clone(),
        blocks: minimal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Arc 15 §8.3.1 / Step 1.1: `from_status` enforces
    /// `active = false` + `status = Some(s)`.
    #[test]
    fn account_event_from_status_enforces_invariant() {
        let e = AccountEvent::from_status("did:plc:abc".to_string(), AccountStatus::Deleted);
        assert!(!e.active);
        assert_eq!(e.status, Some(AccountStatus::Deleted));
        assert_eq!(e.did, "did:plc:abc");

        let e2 = AccountEvent::from_status("did:plc:xyz".to_string(), AccountStatus::Takendown);
        assert!(!e2.active);
        assert_eq!(e2.status, Some(AccountStatus::Takendown));
    }

    /// Arc 15 §8.3.1 / Step 1.1: `active()` enforces
    /// `active = true` + `status = None` (wire-absent).
    #[test]
    fn account_event_active_enforces_invariant() {
        let e = AccountEvent::active("did:plc:abc".to_string());
        assert!(e.active);
        assert_eq!(e.status, None);
    }
}
