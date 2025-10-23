/// Actor store database models
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Repository root metadata
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RepoRoot {
    pub did: String,
    pub cid: String,
    pub rev: String,
    pub indexed_at: DateTime<Utc>,
}

/// MST block
#[derive(Debug, Clone, FromRow)]
pub struct RepoBlock {
    pub cid: String,
    pub repo_rev: String,
    pub size: i64,
    pub content: Vec<u8>,
}

/// Record metadata and index
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Record {
    pub uri: String,
    pub cid: String,
    pub collection: String,
    pub rkey: String,
    pub repo_rev: String,
    pub indexed_at: DateTime<Utc>,
    pub takedown_ref: Option<String>,
}

/// Blob metadata
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Blob {
    pub cid: String,
    pub mime_type: String,
    pub size: i64,
    pub temp_key: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub takedown_ref: Option<String>,
}

/// Record-blob association
#[derive(Debug, Clone, FromRow)]
pub struct RecordBlob {
    pub blob_cid: String,
    pub record_uri: String,
}

/// Backlink (record link tracking)
#[derive(Debug, Clone, FromRow)]
pub struct Backlink {
    pub uri: String,
    pub path: String,
    pub link_to: String,
}

/// User preference
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AccountPref {
    pub id: i64,
    pub name: String,
    pub value_json: String,
}

/// Commit data for repository operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitData {
    pub cid: String,
    pub rev: String,
    pub since: Option<String>,
    pub prev: Option<String>,
}

/// Record operation types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteOpAction {
    Create,
    Update,
    Delete,
}

/// Prepared write operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedWrite {
    pub action: WriteOpAction,
    pub collection: String,
    pub rkey: String,
    pub record: Option<serde_json::Value>,
    pub swap_cid: Option<String>,
    pub validate: Option<bool>,
}
