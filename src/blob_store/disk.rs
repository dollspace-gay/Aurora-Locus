/// Disk-based blob storage backend
use crate::{
    blob_store::BlobBackend,
    error::{PdsError, PdsResult},
};
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::fs;

/// Disk storage backend
///
/// Stores blobs on the local filesystem with directory sharding
/// based on CID prefixes to prevent too many files in one directory.
#[derive(Clone)]
pub struct DiskBlobBackend {
    base_path: PathBuf,
}

impl DiskBlobBackend {
    /// Create a new disk storage backend
    pub fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    /// Get the file path for a CID
    ///
    /// Uses directory sharding: {base}/{first2chars}/{cid}
    /// For example, CID "bafyreiabc..." -> {base}/ba/bafyreiabc...
    fn get_blob_path(&self, cid: &str) -> PathBuf {
        if cid.len() >= 2 {
            let shard = &cid[0..2];
            self.base_path.join(shard).join(cid)
        } else {
            self.base_path.join("_").join(cid)
        }
    }

    /// Ensure the directory for a blob exists
    async fn ensure_blob_dir(&self, cid: &str) -> PdsResult<PathBuf> {
        let blob_path = self.get_blob_path(cid);
        if let Some(parent) = blob_path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                PdsError::BlobStorage(format!("Failed to create blob directory: {}", e))
            })?;
        }
        Ok(blob_path)
    }
}

#[async_trait]
impl BlobBackend for DiskBlobBackend {
    async fn put(&self, cid: &str, data: Vec<u8>, _mime_type: &str) -> PdsResult<()> {
        let blob_path = self.ensure_blob_dir(cid).await?;

        fs::write(&blob_path, data).await.map_err(|e| {
            PdsError::BlobStorage(format!("Failed to write blob {}: {}", cid, e))
        })?;

        Ok(())
    }

    async fn get(&self, cid: &str) -> PdsResult<Option<Vec<u8>>> {
        let blob_path = self.get_blob_path(cid);

        match fs::read(&blob_path).await {
            Ok(data) => Ok(Some(data)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(PdsError::BlobStorage(format!(
                "Failed to read blob {}: {}",
                cid, e
            ))),
        }
    }

    async fn delete(&self, cid: &str) -> PdsResult<()> {
        let blob_path = self.get_blob_path(cid);

        match fs::remove_file(&blob_path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(PdsError::BlobStorage(format!(
                "Failed to delete blob {}: {}",
                cid, e
            ))),
        }
    }

    async fn exists(&self, cid: &str) -> PdsResult<bool> {
        let blob_path = self.get_blob_path(cid);
        Ok(blob_path.exists())
    }

    async fn size(&self, cid: &str) -> PdsResult<Option<u64>> {
        let blob_path = self.get_blob_path(cid);

        match fs::metadata(&blob_path).await {
            Ok(metadata) => Ok(Some(metadata.len())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(PdsError::BlobStorage(format!(
                "Failed to get blob size {}: {}",
                cid, e
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_put_and_get_blob() {
        let dir = tempdir().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());

        let cid = "bafyreiabc123";
        let data = b"test blob data".to_vec();

        // Put blob
        backend.put(cid, data.clone(), "image/png").await.unwrap();

        // Get blob
        let retrieved = backend.get(cid).await.unwrap();
        assert_eq!(retrieved, Some(data));
    }

    #[tokio::test]
    async fn test_get_nonexistent_blob() {
        let dir = tempdir().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());

        let result = backend.get("nonexistent").await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_delete_blob() {
        let dir = tempdir().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());

        let cid = "bafyreidelete123";
        let data = b"to be deleted".to_vec();

        // Put and then delete
        backend.put(cid, data, "image/png").await.unwrap();
        assert!(backend.exists(cid).await.unwrap());

        backend.delete(cid).await.unwrap();
        assert!(!backend.exists(cid).await.unwrap());
    }

    #[tokio::test]
    async fn test_blob_size() {
        let dir = tempdir().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());

        let cid = "bafyreisize123";
        let data = b"12345".to_vec();

        backend.put(cid, data.clone(), "text/plain").await.unwrap();

        let size = backend.size(cid).await.unwrap();
        assert_eq!(size, Some(5));
    }

    #[tokio::test]
    async fn test_directory_sharding() {
        let dir = tempdir().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());

        let cid = "bafyreiabc123";
        let path = backend.get_blob_path(cid);

        // Should be in a subdirectory based on first 2 chars
        assert!(path.to_string_lossy().contains("/ba/"));
    }
}
