/// Blob Storage System
///
/// Handles binary file storage for images, videos, and other media.
/// Supports multiple backend implementations (disk, S3, etc.)

pub mod disk;
pub mod models;
pub mod store;

pub use models::*;
pub use store::{BlobStore, BlobStoreConfig};

use crate::error::PdsResult;
use async_trait::async_trait;
use std::path::PathBuf;

/// Blob storage backend trait
///
/// Implementations handle the actual storage and retrieval of blob data.
#[async_trait]
pub trait BlobBackend: Send + Sync {
    /// Store a blob and return its CID
    async fn put(&self, cid: &str, data: Vec<u8>, mime_type: &str) -> PdsResult<()>;

    /// Retrieve a blob by CID
    async fn get(&self, cid: &str) -> PdsResult<Option<Vec<u8>>>;

    /// Delete a blob by CID
    async fn delete(&self, cid: &str) -> PdsResult<()>;

    /// Check if a blob exists
    async fn exists(&self, cid: &str) -> PdsResult<bool>;

    /// Get the size of a blob in bytes
    async fn size(&self, cid: &str) -> PdsResult<Option<u64>>;
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
    Disk {
        location: PathBuf,
    },

    /// Store blobs in S3-compatible storage
    #[allow(dead_code)]
    S3 {
        bucket: String,
        region: String,
        endpoint: Option<String>,
    },
}
