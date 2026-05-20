//! Blob Storage System
//!
//! Handles binary file storage for images, videos, and other media.
//! Supports multiple backend implementations (disk, S3, etc.)

pub mod disk;
pub mod gc;
pub mod mime;
pub mod models;
pub mod quarantine;
pub mod s3;
pub mod store;

pub use models::*;
// Phase 2 (#72) will wire AppContext to construct S3BlobBackend; until
// then the re-export is unused at bin-scope. The allow lifts in Phase 2.
#[allow(unused_imports)]
pub use s3::{S3BlobBackend, S3Config};
pub use store::{BlobStore, BlobStoreConfig, UnreferenceOutcome};

use crate::error::PdsResult;
use async_trait::async_trait;
use std::path::PathBuf;

/// A page of blobs from a paginated storage walk.
///
/// Returned by [`BlobBackend::list_all_blobs`]. The
/// `next_cursor` is an opaque continuation token suitable for
/// passing back into a subsequent call; `None` means the walk
/// is complete.
#[derive(Debug, Clone)]
pub struct BlobListPage {
    pub entries: Vec<BlobListEntry>,
    pub next_cursor: Option<String>,
}

/// A single blob entry from a storage walk.
#[derive(Debug, Clone)]
pub struct BlobListEntry {
    /// The blob CID, with any backend-specific prefix or
    /// sharding stripped. Round-trips through `get` / `put` /
    /// `delete` without further normalisation.
    pub cid: String,
    /// Storage-side last-modified timestamp. Used by Arc 10's
    /// GC sweep to apply the belt-and-braces freshness threshold
    /// for orphan classification (the authoritative in-flight
    /// signal is the `temp_blob_metadata` table; this timestamp
    /// is a secondary check for blobs that escaped the upload
    /// tracking surface entirely).
    pub last_modified: chrono::DateTime<chrono::Utc>,
}

/// Blob storage backend trait
///
/// Implementations handle the actual storage and retrieval of blob data.
#[async_trait]
pub trait BlobBackend: Send + Sync {
    /// Store a blob and return its CID
    #[allow(dead_code)] // Trait methods for future blob backends
    async fn put(&self, cid: &str, data: Vec<u8>, mime_type: &str) -> PdsResult<()>;

    /// Retrieve a blob by CID
    async fn get(&self, cid: &str) -> PdsResult<Option<Vec<u8>>>;

    /// Delete a blob by CID
    async fn delete(&self, cid: &str) -> PdsResult<()>;

    /// Check if a blob exists
    #[allow(dead_code)] // Trait method for future blob backends
    async fn exists(&self, cid: &str) -> PdsResult<bool>;

    /// Get the size of a blob in bytes
    #[allow(dead_code)] // Trait method for future blob backends
    async fn size(&self, cid: &str) -> PdsResult<Option<u64>>;

    /// List blobs in storage, paginated.
    ///
    /// `cursor` is an opaque continuation token from a previous
    /// call's response, or `None` to start from the beginning.
    /// `page_size` is a hint; backends may return fewer entries.
    ///
    /// Returns the next page of CIDs paired with storage-side
    /// last-modified timestamps. `next_cursor` is `None` when
    /// iteration is complete.
    ///
    /// Added in Arc 10 (chainlink #57) to support the GC sweep
    /// for orphaned blob storage. Backend-specific cursor
    /// semantics:
    ///
    /// - `DiskBlobBackend`: cursor is a synthesised
    ///   `"{shard}/{filename}"` string. Walks shards
    ///   lexicographically; within each shard, files
    ///   lexicographically.
    /// - `S3BlobBackend`: cursor is the S3 `ContinuationToken`
    ///   pass-through; `ListObjectsV2` is the underlying call.
    ///
    /// Both backends consistently propagate `last_modified` from
    /// their respective storage metadata (FS `metadata().modified()`,
    /// S3 `Object::last_modified`).
    async fn list_all_blobs(
        &self,
        cursor: Option<String>,
        page_size: usize,
    ) -> PdsResult<BlobListPage>;

    /// Arc 16c §9.3.3.2 step 3 — establish bytes-durability at the
    /// CID-derived final position. Called by `BlobStore::commit_blob`
    /// AFTER `put()` succeeds, BEFORE the metadata transaction opens.
    ///
    /// - **Disk backend**: open file at CID path, `sync_all()` (file
    ///   data + metadata); open containing directory, `sync_all()`.
    ///   "Both absent" disposition per Arc 16c Step 0.2 recon — disk
    ///   backend had no fsync today; Arc 16c adds both in canonical
    ///   order (file then directory).
    /// - **S3 backend**: no-op (durability was confirmed by the 2xx
    ///   PUT response inside `put()`; no further sync needed).
    ///
    /// Default impl: no-op. Backends that need fsync override.
    async fn fsync(&self, _cid: &str) -> PdsResult<()> {
        Ok(())
    }
}

/// Configuration for blob storage
#[derive(Debug, Clone)]
pub struct BlobStorageConfig {
    /// Backend type
    pub backend: BlobBackendType,

    /// Maximum blob size in bytes (default: 5MB)
    pub max_blob_size: usize,

    /// Temporary upload directory
    pub temp_dir: PathBuf,
}

impl Default for BlobStorageConfig {
    fn default() -> Self {
        Self {
            backend: BlobBackendType::Disk {
                location: PathBuf::from("./data/blobs"),
            },
            max_blob_size: 5 * 1024 * 1024, // 5MB
            temp_dir: PathBuf::from("./data/tmp"),
        }
    }
}

/// Backend types for blob storage
#[derive(Debug, Clone)]
pub enum BlobBackendType {
    /// Store blobs on local disk
    Disk { location: PathBuf },

    /// Store blobs in S3-compatible storage. Phase 2 (#72) added the
    /// credential and prefix fields; Phase 3 (#73) added `force_path_style`
    /// and `upload_timeout_ms` for parity with bsky-PDS env-var conventions.
    S3 {
        bucket: String,
        region: String,
        endpoint: Option<String>,
        access_key_id: String,
        secret_access_key: String,
        /// S3 object key prefix (default `"blobs/"`).
        prefix: String,
        /// Path-style addressing toggle (default `false`; set `true` for
        /// MinIO and other S3-compatible providers without virtual-host
        /// support).
        force_path_style: bool,
        /// Upload operation timeout in milliseconds (default `20000`).
        upload_timeout_ms: u64,
    },
}
