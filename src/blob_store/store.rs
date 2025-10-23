/// Blob Store Manager
///
/// Coordinates blob storage backends with database metadata tracking
use crate::{
    blob_store::{disk::DiskBlobBackend, BlobBackend, BlobBackendType, BlobMetadata, BlobRef, BlobStorageConfig, ImageDimensions, TempBlob},
    error::{PdsError, PdsResult},
};
use chrono::Utc;
use image::ImageFormat;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use std::sync::Arc;
use tokio::fs;

/// Blob store configuration
#[derive(Debug, Clone)]
pub struct BlobStoreConfig {
    pub storage: BlobStorageConfig,
}

impl Default for BlobStoreConfig {
    fn default() -> Self {
        Self {
            storage: BlobStorageConfig::default(),
        }
    }
}

/// Main blob store manager
#[derive(Clone)]
pub struct BlobStore {
    config: BlobStoreConfig,
    backend: Arc<dyn BlobBackend>,
    db: SqlitePool,
}

impl BlobStore {
    /// Create a new blob store
    pub fn new(config: BlobStoreConfig, db: SqlitePool) -> PdsResult<Self> {
        let backend: Arc<dyn BlobBackend> = match &config.storage.backend {
            BlobBackendType::Disk { location } => {
                Arc::new(DiskBlobBackend::new(location.clone()))
            }
            BlobBackendType::S3 { .. } => {
                return Err(PdsError::Internal("S3 backend not yet implemented".to_string()));
            }
        };

        Ok(Self { config, backend, db })
    }

    /// Extract image dimensions from data
    fn extract_image_dimensions(data: &[u8], mime_type: &str) -> Option<ImageDimensions> {
        // Only process images
        if !mime_type.starts_with("image/") {
            return None;
        }

        // Try to load the image
        match image::load_from_memory(data) {
            Ok(img) => Some(ImageDimensions {
                width: img.width(),
                height: img.height(),
            }),
            Err(e) => {
                tracing::warn!("Failed to extract image dimensions: {}", e);
                None
            }
        }
    }

    /// Generate thumbnail for an image
    fn generate_thumbnail(data: &[u8], mime_type: &str, max_size: u32) -> Option<Vec<u8>> {
        // Only process images
        if !mime_type.starts_with("image/") {
            return None;
        }

        // Try to load and resize the image
        match image::load_from_memory(data) {
            Ok(img) => {
                // Resize to thumbnail (preserving aspect ratio)
                let thumb = img.thumbnail(max_size, max_size);

                // Encode as JPEG (good balance of size/quality for thumbnails)
                let mut buf = Vec::new();
                let mut cursor = std::io::Cursor::new(&mut buf);

                match thumb.write_to(&mut cursor, ImageFormat::Jpeg) {
                    Ok(_) => Some(buf),
                    Err(e) => {
                        tracing::warn!("Failed to encode thumbnail: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to generate thumbnail: {}", e);
                None
            }
        }
    }

    /// Get temp blob file path
    fn get_temp_blob_path(&self, cid: &str) -> std::path::PathBuf {
        self.config.storage.temp_dir.join(cid)
    }

    /// Stage a blob in temporary storage (Phase 1 of two-phase upload)
    ///
    /// Returns TempBlob with metadata for later commitment
    pub async fn stage_blob(&self, data: Vec<u8>, mime_type: Option<&str>, creator_did: &str) -> PdsResult<TempBlob> {
        // Validate size
        let size = data.len();
        atproto::blob::validate_blob_size(size, self.config.storage.max_blob_size)
            .map_err(|e| PdsError::Validation(e))?;

        // Detect MIME type from data if not provided
        let mime_type = mime_type
            .map(String::from)
            .or_else(|| {
                atproto::blob::detect_mime_type_from_data(&data).map(String::from)
            })
            .unwrap_or_else(|| "application/octet-stream".to_string());

        // Validate MIME type is allowed
        self.validate_mime_type(&mime_type)?;

        // Calculate CID
        let cid = self.calculate_cid(&data);

        // Extract image dimensions if this is an image
        let dimensions = Self::extract_image_dimensions(&data, &mime_type);
        let (width, height) = dimensions
            .map(|d| (Some(d.width as i64), Some(d.height as i64)))
            .unwrap_or((None, None));

        // Ensure temp directory exists
        fs::create_dir_all(&self.config.storage.temp_dir)
            .await
            .map_err(|e| PdsError::BlobStorage(format!("Failed to create temp directory: {}", e)))?;

        // Write to temp location
        let temp_path = self.get_temp_blob_path(&cid);
        fs::write(&temp_path, &data)
            .await
            .map_err(|e| PdsError::BlobStorage(format!("Failed to write temp blob: {}", e)))?;

        let temp_blob = TempBlob {
            cid: cid.clone(),
            mime_type,
            size: size as i64,
            creator_did: creator_did.to_string(),
            created_at: Utc::now(),
            width,
            height,
        };

        // Store temp blob metadata in database
        self.store_temp_blob_metadata(&temp_blob).await?;

        tracing::info!("Staged blob {} in temp storage", cid);

        Ok(temp_blob)
    }

    /// Commit a staged blob to permanent storage (Phase 2 of two-phase upload)
    ///
    /// Moves blob from temp to permanent storage and creates metadata
    pub async fn commit_blob(&self, cid: &str) -> PdsResult<()> {
        let temp_path = self.get_temp_blob_path(cid);

        // Check if temp blob exists
        if !temp_path.exists() {
            return Err(PdsError::NotFound(format!("Temp blob not found: {}", cid)));
        }

        // Read temp blob data
        let data = fs::read(&temp_path)
            .await
            .map_err(|e| PdsError::BlobStorage(format!("Failed to read temp blob: {}", e)))?;

        // Get metadata from database (should have been stored during stage)
        let metadata = self.get_temp_blob_metadata(cid).await?
            .ok_or_else(|| PdsError::NotFound(format!("Temp blob metadata not found: {}", cid)))?;

        // Extract dimensions for thumbnail generation
        let dimensions = if let (Some(w), Some(h)) = (metadata.width, metadata.height) {
            Some(ImageDimensions {
                width: w as u32,
                height: h as u32,
            })
        } else {
            None
        };

        // Generate thumbnail if this is an image
        let thumbnail_cid = if let Some(thumb_data) = Self::generate_thumbnail(&data, &metadata.mime_type, 256) {
            let thumb_cid = self.calculate_cid(&thumb_data);

            if !self.backend.exists(&thumb_cid).await? {
                self.backend.put(&thumb_cid, thumb_data.clone(), "image/jpeg").await?;

                let thumb_dimensions = Self::extract_image_dimensions(&thumb_data, "image/jpeg");
                self.store_metadata_full(
                    &thumb_cid,
                    "image/jpeg",
                    thumb_data.len() as i64,
                    &metadata.creator_did,
                    thumb_dimensions.as_ref(),
                    None,
                ).await?;
            }

            Some(thumb_cid)
        } else {
            None
        };

        // Move to permanent storage
        self.backend.put(cid, data, &metadata.mime_type).await?;

        // Store permanent metadata
        self.store_metadata_full(
            cid,
            &metadata.mime_type,
            metadata.size,
            &metadata.creator_did,
            dimensions.as_ref(),
            thumbnail_cid.as_deref(),
        ).await?;

        // Delete temp file
        fs::remove_file(&temp_path)
            .await
            .map_err(|e| PdsError::BlobStorage(format!("Failed to delete temp blob: {}", e)))?;

        // Delete temp metadata
        self.delete_temp_blob_metadata(cid).await?;

        tracing::info!("Committed blob {} to permanent storage", cid);

        Ok(())
    }

    /// Upload a blob
    ///
    /// Returns the blob metadata and reference
    pub async fn upload(&self, data: Vec<u8>, mime_type: Option<&str>, creator_did: &str) -> PdsResult<BlobRef> {
        // Validate size
        let size = data.len();
        atproto::blob::validate_blob_size(size, self.config.storage.max_blob_size)
            .map_err(|e| PdsError::Validation(e))?;

        // Detect MIME type from data if not provided
        let mime_type = mime_type
            .map(String::from)
            .or_else(|| {
                atproto::blob::detect_mime_type_from_data(&data).map(String::from)
            })
            .unwrap_or_else(|| "application/octet-stream".to_string());

        // Validate MIME type is allowed
        self.validate_mime_type(&mime_type)?;

        // Calculate CID (using SHA-256 hash)
        let cid = self.calculate_cid(&data);

        // Extract image dimensions if this is an image
        let dimensions = Self::extract_image_dimensions(&data, &mime_type);

        // Generate thumbnail if this is an image (256x256 max)
        let thumbnail_cid = if let Some(thumb_data) = Self::generate_thumbnail(&data, &mime_type, 256) {
            // Calculate thumbnail CID
            let thumb_cid = self.calculate_cid(&thumb_data);

            // Store thumbnail blob
            if !self.backend.exists(&thumb_cid).await? {
                self.backend.put(&thumb_cid, thumb_data.clone(), "image/jpeg").await?;

                // Extract dimensions from thumbnail
                let thumb_dimensions = Self::extract_image_dimensions(&thumb_data, "image/jpeg");

                // Store thumbnail metadata with dimensions
                self.store_metadata_full(
                    &thumb_cid,
                    "image/jpeg",
                    thumb_data.len() as i64,
                    creator_did,
                    thumb_dimensions.as_ref(),
                    None, // thumbnails don't have their own thumbnails
                ).await?;
            }

            Some(thumb_cid)
        } else {
            None
        };

        // Check if blob already exists
        if self.backend.exists(&cid).await? {
            // Blob already exists, just return the reference
            return Ok(BlobRef::new(cid, mime_type, size as i64));
        }

        // Store blob in backend
        self.backend.put(&cid, data, &mime_type).await?;

        // Store metadata in database with dimensions and thumbnail
        self.store_metadata_full(
            &cid,
            &mime_type,
            size as i64,
            creator_did,
            dimensions.as_ref(),
            thumbnail_cid.as_deref(),
        ).await?;

        Ok(BlobRef::new(cid, mime_type, size as i64))
    }

    /// Get a blob by CID
    pub async fn get(&self, cid: &str) -> PdsResult<Option<(Vec<u8>, String)>> {
        // Get blob data from backend
        let data = self.backend.get(cid).await?;

        if let Some(data) = data {
            // Get MIME type from database
            let metadata = self.get_metadata(cid).await?;
            let mime_type = metadata
                .map(|m| m.mime_type)
                .unwrap_or_else(|| "application/octet-stream".to_string());

            Ok(Some((data, mime_type)))
        } else {
            Ok(None)
        }
    }

    /// Delete a blob
    pub async fn delete(&self, cid: &str) -> PdsResult<()> {
        // Delete from backend
        self.backend.delete(cid).await?;

        // Delete metadata from database
        self.delete_metadata(cid).await?;

        Ok(())
    }

    /// Calculate CID for data using SHA-256
    fn calculate_cid(&self, data: &[u8]) -> String {
        let hash = Sha256::digest(data);
        format!("bafyrei{}", hex::encode(hash))
    }

    /// Validate MIME type is allowed
    fn validate_mime_type(&self, mime_type: &str) -> PdsResult<()> {
        // ATProto allows specific image and video types
        const ALLOWED_TYPES: &[&str] = &[
            "image/jpeg",
            "image/png",
            "image/gif",
            "image/webp",
            "video/mp4",
            "video/quicktime",
            "video/webm",
        ];

        if ALLOWED_TYPES.contains(&mime_type) {
            Ok(())
        } else {
            Err(PdsError::Validation(format!(
                "Unsupported MIME type: {}",
                mime_type
            )))
        }
    }

    /// Store blob metadata in database (basic version without dimensions)
    async fn store_metadata(&self, cid: &str, mime_type: &str, size: i64, creator_did: &str) -> PdsResult<()> {
        sqlx::query(
            r#"
            INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(cid) DO NOTHING
            "#,
        )
        .bind(cid)
        .bind(mime_type)
        .bind(size)
        .bind(creator_did)
        .bind(Utc::now())
        .execute(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        Ok(())
    }

    /// Store blob metadata in database with full information (dimensions, thumbnail)
    async fn store_metadata_full(
        &self,
        cid: &str,
        mime_type: &str,
        size: i64,
        creator_did: &str,
        dimensions: Option<&ImageDimensions>,
        thumbnail_cid: Option<&str>,
    ) -> PdsResult<()> {
        let (width, height) = dimensions
            .map(|d| (Some(d.width as i64), Some(d.height as i64)))
            .unwrap_or((None, None));

        sqlx::query(
            r#"
            INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, width, height, thumbnail_cid)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(cid) DO UPDATE SET
                width = excluded.width,
                height = excluded.height,
                thumbnail_cid = excluded.thumbnail_cid
            "#,
        )
        .bind(cid)
        .bind(mime_type)
        .bind(size)
        .bind(creator_did)
        .bind(Utc::now())
        .bind(width)
        .bind(height)
        .bind(thumbnail_cid)
        .execute(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        Ok(())
    }

    /// Store temp blob metadata in database
    async fn store_temp_blob_metadata(&self, temp_blob: &TempBlob) -> PdsResult<()> {
        sqlx::query(
            r#"
            INSERT INTO temp_blob_metadata (cid, mime_type, size, creator_did, created_at, width, height)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(cid) DO UPDATE SET
                mime_type = excluded.mime_type,
                size = excluded.size,
                width = excluded.width,
                height = excluded.height
            "#,
        )
        .bind(&temp_blob.cid)
        .bind(&temp_blob.mime_type)
        .bind(temp_blob.size)
        .bind(&temp_blob.creator_did)
        .bind(temp_blob.created_at)
        .bind(temp_blob.width)
        .bind(temp_blob.height)
        .execute(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        Ok(())
    }

    /// Get temp blob metadata from database
    async fn get_temp_blob_metadata(&self, cid: &str) -> PdsResult<Option<TempBlob>> {
        let result = sqlx::query(
            r#"
            SELECT cid, mime_type, size, creator_did, created_at, width, height
            FROM temp_blob_metadata
            WHERE cid = ?1
            "#,
        )
        .bind(cid)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        if let Some(row) = result {
            Ok(Some(TempBlob {
                cid: row.try_get("cid")?,
                mime_type: row.try_get("mime_type")?,
                size: row.try_get("size")?,
                creator_did: row.try_get("creator_did")?,
                created_at: row.try_get("created_at")?,
                width: row.try_get("width")?,
                height: row.try_get("height")?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Delete temp blob metadata from database
    async fn delete_temp_blob_metadata(&self, cid: &str) -> PdsResult<()> {
        sqlx::query("DELETE FROM temp_blob_metadata WHERE cid = ?1")
            .bind(cid)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        Ok(())
    }

    /// List orphaned temp blobs (older than ttl)
    pub async fn list_orphaned_temp_blobs(&self, ttl_hours: i64) -> PdsResult<Vec<String>> {
        let cutoff = Utc::now() - chrono::Duration::hours(ttl_hours);

        let rows = sqlx::query(
            r#"
            SELECT cid
            FROM temp_blob_metadata
            WHERE created_at < ?1
            ORDER BY created_at ASC
            "#,
        )
        .bind(cutoff)
        .fetch_all(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        let mut cids = Vec::new();
        for row in rows {
            cids.push(row.try_get("cid")?);
        }

        Ok(cids)
    }

    /// Delete orphaned temp blob (both file and metadata)
    pub async fn delete_temp_blob(&self, cid: &str) -> PdsResult<()> {
        // Delete temp file
        let temp_path = self.get_temp_blob_path(cid);
        if temp_path.exists() {
            fs::remove_file(&temp_path)
                .await
                .map_err(|e| PdsError::BlobStorage(format!("Failed to delete temp blob file: {}", e)))?;
        }

        // Delete metadata
        self.delete_temp_blob_metadata(cid).await?;

        Ok(())
    }

    /// Get blob metadata from database (public method)
    pub async fn get_metadata(&self, cid: &str) -> PdsResult<Option<BlobMetadata>> {
        let result = sqlx::query(
            r#"
            SELECT cid, mime_type, size, creator_did, created_at, width, height, alt_text, thumbnail_cid
            FROM blob_metadata
            WHERE cid = ?1
            "#,
        )
        .bind(cid)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        if let Some(row) = result {
            Ok(Some(BlobMetadata {
                cid: row.try_get("cid")?,
                mime_type: row.try_get("mime_type")?,
                size: row.try_get("size")?,
                creator_did: row.try_get("creator_did")?,
                created_at: row.try_get("created_at")?,
                width: row.try_get("width")?,
                height: row.try_get("height")?,
                alt_text: row.try_get("alt_text")?,
                thumbnail_cid: row.try_get("thumbnail_cid")?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Delete blob metadata from database
    async fn delete_metadata(&self, cid: &str) -> PdsResult<()> {
        sqlx::query("DELETE FROM blob_metadata WHERE cid = ?1")
            .bind(cid)
            .execute(&self.db)
            .await
            .map_err(|e| PdsError::Database(e))?;

        Ok(())
    }

    /// List blobs for a user
    pub async fn list_for_user(&self, did: &str, limit: i64) -> PdsResult<Vec<BlobMetadata>> {
        let rows = sqlx::query(
            r#"
            SELECT cid, mime_type, size, creator_did, created_at, width, height, alt_text, thumbnail_cid
            FROM blob_metadata
            WHERE creator_did = ?1
            ORDER BY created_at DESC
            LIMIT ?2
            "#,
        )
        .bind(did)
        .bind(limit)
        .fetch_all(&self.db)
        .await
        .map_err(|e| PdsError::Database(e))?;

        let mut blobs = Vec::new();
        for row in rows {
            blobs.push(BlobMetadata {
                cid: row.try_get("cid")?,
                mime_type: row.try_get("mime_type")?,
                size: row.try_get("size")?,
                creator_did: row.try_get("creator_did")?,
                created_at: row.try_get("created_at")?,
                width: row.try_get("width")?,
                height: row.try_get("height")?,
                alt_text: row.try_get("alt_text")?,
                thumbnail_cid: row.try_get("thumbnail_cid")?,
            });
        }

        Ok(blobs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn create_test_store() -> BlobStore {
        let dir = tempdir().unwrap();
        let config = BlobStoreConfig {
            storage: BlobStorageConfig {
                backend: BlobBackendType::Disk {
                    location: dir.path().to_path_buf(),
                },
                max_blob_size: 1024 * 1024,
                temp_dir: dir.path().join("tmp"),
            },
        };

        // Create in-memory database for testing
        let db = SqlitePool::connect(":memory:").await.unwrap();

        // Create blob_metadata table with new columns
        sqlx::query(
            r#"
            CREATE TABLE blob_metadata (
                cid TEXT PRIMARY KEY,
                mime_type TEXT NOT NULL,
                size INTEGER NOT NULL,
                creator_did TEXT NOT NULL,
                created_at DATETIME NOT NULL,
                width INTEGER,
                height INTEGER,
                alt_text TEXT,
                thumbnail_cid TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        BlobStore::new(config, db).unwrap()
    }

    #[tokio::test]
    async fn test_upload_and_get_blob() {
        let store = create_test_store().await;

        let data = b"test image data".to_vec();
        let blob_ref = store.upload(data.clone(), Some("image/png"), "did:plc:test").await.unwrap();

        assert_eq!(blob_ref.mime_type, "image/png");
        assert_eq!(blob_ref.size, 15);

        // Get the blob back
        let (retrieved_data, mime_type) = store.get(&blob_ref.r#ref.link).await.unwrap().unwrap();
        assert_eq!(retrieved_data, data);
        assert_eq!(mime_type, "image/png");
    }

    #[tokio::test]
    async fn test_upload_duplicate_blob() {
        let store = create_test_store().await;

        let data = b"duplicate data".to_vec();

        // Upload twice
        let blob_ref1 = store.upload(data.clone(), Some("image/jpeg"), "did:plc:test1").await.unwrap();
        let blob_ref2 = store.upload(data, Some("image/jpeg"), "did:plc:test2").await.unwrap();

        // Should have same CID (content-addressed)
        assert_eq!(blob_ref1.r#ref.link, blob_ref2.r#ref.link);
    }

    #[tokio::test]
    async fn test_upload_oversized_blob() {
        let store = create_test_store().await;

        let large_data = vec![0u8; 2 * 1024 * 1024]; // 2MB, over limit

        let result = store.upload(large_data, Some("image/png"), "did:plc:test").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("exceeds maximum"));
    }

    #[tokio::test]
    async fn test_upload_invalid_mime_type() {
        let store = create_test_store().await;

        let data = b"test data".to_vec();

        let result = store.upload(data, Some("application/exe"), "did:plc:test").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unsupported MIME type"));
    }

    #[tokio::test]
    async fn test_delete_blob() {
        let store = create_test_store().await;

        let data = b"to be deleted".to_vec();
        let blob_ref = store.upload(data, Some("image/png"), "did:plc:test").await.unwrap();

        // Delete
        store.delete(&blob_ref.r#ref.link).await.unwrap();

        // Should not exist
        let result = store.get(&blob_ref.r#ref.link).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_image_dimensions_extraction() {
        let store = create_test_store().await;

        // Create a small 10x10 PNG image
        let img = image::RgbImage::new(10, 10);
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        img.write_to(&mut cursor, ImageFormat::Png).unwrap();

        // Upload the image
        let blob_ref = store.upload(buf, Some("image/png"), "did:plc:test").await.unwrap();

        // Get metadata and verify dimensions
        let metadata = store.get_metadata(&blob_ref.r#ref.link).await.unwrap().unwrap();
        assert_eq!(metadata.width, Some(10));
        assert_eq!(metadata.height, Some(10));
        assert_eq!(metadata.mime_type, "image/png");
    }

    #[tokio::test]
    async fn test_thumbnail_generation() {
        let store = create_test_store().await;

        // Create a larger 1000x1000 PNG image
        let img = image::RgbImage::new(1000, 1000);
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        img.write_to(&mut cursor, ImageFormat::Png).unwrap();

        // Upload the image
        let blob_ref = store.upload(buf, Some("image/png"), "did:plc:test").await.unwrap();

        // Get metadata and verify thumbnail was created
        let metadata = store.get_metadata(&blob_ref.r#ref.link).await.unwrap().unwrap();
        assert!(metadata.thumbnail_cid.is_some(), "Thumbnail should be generated for images");

        // Verify thumbnail exists and is a valid blob
        let thumb_cid = metadata.thumbnail_cid.unwrap();
        let thumb_data = store.get(&thumb_cid).await.unwrap();
        assert!(thumb_data.is_some(), "Thumbnail blob should exist");

        // Verify thumbnail metadata
        let thumb_metadata = store.get_metadata(&thumb_cid).await.unwrap().unwrap();
        assert_eq!(thumb_metadata.mime_type, "image/jpeg");
        // Thumbnail should be max 256x256
        assert!(thumb_metadata.width.unwrap() <= 256);
        assert!(thumb_metadata.height.unwrap() <= 256);
    }

    #[tokio::test]
    async fn test_get_metadata() {
        let store = create_test_store().await;

        let data = b"test data".to_vec();
        let blob_ref = store.upload(data, Some("image/png"), "did:plc:test").await.unwrap();

        // Get metadata
        let metadata = store.get_metadata(&blob_ref.r#ref.link).await.unwrap().unwrap();
        assert_eq!(metadata.cid, blob_ref.r#ref.link);
        assert_eq!(metadata.mime_type, "image/png");
        assert_eq!(metadata.size, 9);
        assert_eq!(metadata.creator_did, "did:plc:test");
    }
}
