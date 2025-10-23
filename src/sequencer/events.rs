/// Event type definitions for the sequencer
use serde::{Deserialize, Serialize};

/// Commit event - emitted when repository data changes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitEvent {
    pub rebase: bool,
    pub too_big: bool,
    pub repo: String,       // DID
    pub commit: String,     // CID of commit
    pub rev: String,        // Revision TID
    pub since: Option<String>, // Previous commit CID
    pub blocks: Vec<u8>,    // CAR file bytes
    pub ops: Vec<CommitOp>,
    pub blobs: Vec<String>, // CIDs of blobs (deprecated but included)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev: Option<String>, // Previous commit CID for reconstruction
}

/// Operation within a commit
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitOp {
    pub action: OpAction,
    pub path: String,       // collection/rkey
    pub cid: Option<String>, // CID of record (null for delete)
}

/// Operation action type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OpAction {
    Create,
    Update,
    Delete,
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

/// Account status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AccountStatus {
    Takendown,
    Suspended,
    Deleted,
    Deactivated,
}

impl CommitEvent {
    /// Create a new commit event
    pub fn new(
        repo: String,
        commit: String,
        rev: String,
        since: Option<String>,
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
            prev: None,
        }
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
    pub fn new(did: String, active: bool, status: Option<AccountStatus>) -> Self {
        Self { did, active, status }
    }
}
