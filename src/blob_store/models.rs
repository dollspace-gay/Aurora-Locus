/// Blob storage data models
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Blob metadata stored in database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobMetadata {
    pub cid: String,
    pub mime_type: String,
    pub size: i64,
    pub created_at: DateTime<Utc>,
    pub creator_did: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub alt_text: Option<String>,
    pub thumbnail_cid: Option<String>,
}

/// Image dimensions
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ImageDimensions {
    pub width: u32,
    pub height: u32,
}

/// Blob upload response
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobUploadResponse {
    pub blob: BlobRef,
}

/// Blob reference (ATProto standard format)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobRef {
    #[serde(rename = "$type")]
    pub blob_type: String,
    #[serde(rename = "ref")]
    pub r#ref: CidLink,
    pub mime_type: String,
    pub size: i64,
}

impl BlobRef {
    /// Create a new blob reference
    pub fn new(cid: String, mime_type: String, size: i64) -> Self {
        Self {
            blob_type: "blob".to_string(),
            r#ref: CidLink { link: cid },
            mime_type,
            size,
        }
    }
}

/// CID link format
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CidLink {
    #[serde(rename = "$link")]
    pub link: String,
}

/// Temporary blob for uploads (two-phase upload)
#[derive(Debug, Clone)]
pub struct TempBlob {
    pub cid: String,
    pub mime_type: String,
    pub size: i64,
    pub creator_did: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub width: Option<i64>,
    pub height: Option<i64>,
}
