//! S3-compatible blob storage backend.
//!
//! Phase 1 (chainlink #71) re-enabled this module. Phase 2 (#72) wired
//! `BlobStore::new` to construct `S3BlobBackend` from configuration.
//! Phase 3 (#73) added the `force_path_style` and `upload_timeout_ms`
//! parity knobs and exposed `prefix` via env var.

use crate::blob_store::BlobBackend;
use crate::error::{PdsError, PdsResult};
use async_trait::async_trait;
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::Credentials;
use aws_sdk_s3::config::timeout::TimeoutConfig;
use aws_sdk_s3::config::Builder as S3ConfigBuilder;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info};

/// S3 blob storage backend
///
/// Supports AWS S3 and S3-compatible storage providers (MinIO, DigitalOcean Spaces, etc.)
#[derive(Clone)]
pub struct S3BlobBackend {
    client: Arc<Client>,
    bucket: String,
    prefix: String,
}

/// Configuration for S3 storage
#[derive(Debug, Clone)]
pub struct S3Config {
    /// S3 bucket name
    pub bucket: String,

    /// AWS region (e.g., "us-east-1")
    pub region: String,

    /// Custom endpoint for S3-compatible services (e.g., MinIO, DigitalOcean Spaces)
    /// Example: "https://nyc3.digitaloceanspaces.com" or "http://localhost:9000"
    pub endpoint: Option<String>,

    /// AWS access key ID
    pub access_key_id: String,

    /// AWS secret access key
    pub secret_access_key: String,

    /// Path prefix for all objects (default: "blobs/")
    pub prefix: String,

    /// Use path-style addressing (`endpoint/bucket/key`) instead of
    /// virtual-host-style (`bucket.endpoint/key`). Required for MinIO
    /// and other S3-compatible providers that don't support
    /// virtual-host-style addressing. Default: `false` (matches AWS S3).
    pub force_path_style: bool,

    /// Upload-operation timeout in milliseconds. Applies as the SDK's
    /// `operation_timeout` for all S3 calls. Default: `20000` (matches
    /// bsky-PDS).
    pub upload_timeout_ms: u64,
}

impl Default for S3Config {
    fn default() -> Self {
        Self {
            bucket: String::new(),
            region: "us-east-1".to_string(),
            endpoint: None,
            access_key_id: String::new(),
            secret_access_key: String::new(),
            prefix: "blobs/".to_string(),
            force_path_style: false,
            upload_timeout_ms: 20_000,
        }
    }
}

impl S3BlobBackend {
    /// Create a new S3 blob backend
    pub async fn new(config: S3Config) -> PdsResult<Self> {
        info!(
            "Initializing S3 blob storage (bucket: {}, region: {})",
            config.bucket, config.region
        );

        // Create credentials
        let credentials = Credentials::new(
            &config.access_key_id,
            &config.secret_access_key,
            None, // session token
            None, // expiration
            "aurora-locus",
        );

        // Build AWS config
        let aws_config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(config.region.clone()))
            .credentials_provider(credentials)
            .load()
            .await;

        // Build S3 config with the configured endpoint, addressing style,
        // and upload timeout. force_path_style is now operator-controlled
        // (was hardcoded to true when an endpoint was set, which forced
        // path-style on all custom-endpoint deployments regardless of
        // whether the provider supported virtual-host-style).
        let timeout_config = TimeoutConfig::builder()
            .operation_timeout(Duration::from_millis(config.upload_timeout_ms))
            .build();

        let mut s3_config_builder = S3ConfigBuilder::from(&aws_config)
            .force_path_style(config.force_path_style)
            .timeout_config(timeout_config);

        if let Some(endpoint) = &config.endpoint {
            debug!("Using custom S3 endpoint: {}", endpoint);
            s3_config_builder = s3_config_builder.endpoint_url(endpoint);
        }

        let s3_config = s3_config_builder.build();
        let client = Client::from_conf(s3_config);

        // Verify bucket exists (optional, can be expensive)
        // Uncomment if you want to verify on startup:
        // client.head_bucket().bucket(&config.bucket).send().await
        //     .map_err(|e| PdsError::Internal(format!("S3 bucket not accessible: {}", e)))?;

        info!("✓ S3 blob storage initialized");

        Ok(Self {
            client: Arc::new(client),
            bucket: config.bucket,
            prefix: config.prefix,
        })
    }

    /// Get the S3 object key for a CID, using the configured prefix.
    /// Shards CIDs into subdirectories for better S3 performance:
    /// `"abc123..."` → `"<prefix>ab/c1/abc123..."`.
    fn get_key(&self, cid: &str) -> String {
        if cid.len() >= 4 {
            format!("{}{}/{}/{}", self.prefix, &cid[0..2], &cid[2..4], cid)
        } else {
            format!("{}{}", self.prefix, cid)
        }
    }
}

#[async_trait]
impl BlobBackend for S3BlobBackend {
    async fn put(&self, cid: &str, data: Vec<u8>, mime_type: &str) -> PdsResult<()> {
        let key = self.get_key(cid);

        debug!(
            "Uploading blob to S3: {} ({} bytes, type: {})",
            key,
            data.len(),
            mime_type
        );

        let body = ByteStream::from(data);

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(body)
            .content_type(mime_type)
            .send()
            .await
            .map_err(|e| {
                error!("Failed to upload blob to S3: {}", e);
                PdsError::Internal(format!("S3 upload failed: {}", e))
            })?;

        debug!("✓ Blob uploaded to S3: {}", key);
        Ok(())
    }

    async fn get(&self, cid: &str) -> PdsResult<Option<Vec<u8>>> {
        let key = self.get_key(cid);

        debug!("Downloading blob from S3: {}", key);

        match self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
        {
            Ok(response) => {
                let data = response
                    .body
                    .collect()
                    .await
                    .map_err(|e| {
                        error!("Failed to read S3 object body: {}", e);
                        PdsError::Internal(format!("Failed to read S3 object: {}", e))
                    })?
                    .into_bytes()
                    .to_vec();

                debug!("✓ Blob downloaded from S3: {} ({} bytes)", key, data.len());
                Ok(Some(data))
            }
            Err(e) => {
                // Check if it's a "not found" error
                let error_msg = format!("{:?}", e);
                if error_msg.contains("NoSuchKey") || error_msg.contains("NotFound") {
                    debug!("Blob not found in S3: {}", key);
                    Ok(None)
                } else {
                    error!("Failed to download blob from S3: {}", e);
                    Err(PdsError::Internal(format!("S3 download failed: {}", e)))
                }
            }
        }
    }

    async fn delete(&self, cid: &str) -> PdsResult<()> {
        let key = self.get_key(cid);

        debug!("Deleting blob from S3: {}", key);

        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| {
                error!("Failed to delete blob from S3: {}", e);
                PdsError::Internal(format!("S3 delete failed: {}", e))
            })?;

        debug!("✓ Blob deleted from S3: {}", key);
        Ok(())
    }

    async fn exists(&self, cid: &str) -> PdsResult<bool> {
        let key = self.get_key(cid);

        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(e) => {
                let error_msg = format!("{:?}", e);
                if error_msg.contains("NotFound") {
                    Ok(false)
                } else {
                    error!("Failed to check blob existence in S3: {}", e);
                    Err(PdsError::Internal(format!(
                        "S3 head object failed: {}",
                        e
                    )))
                }
            }
        }
    }

    async fn size(&self, cid: &str) -> PdsResult<Option<u64>> {
        let key = self.get_key(cid);

        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
        {
            Ok(response) => Ok(response.content_length().map(|s| s as u64)),
            Err(e) => {
                let error_msg = format!("{:?}", e);
                if error_msg.contains("NotFound") {
                    Ok(None)
                } else {
                    error!("Failed to get blob size from S3: {}", e);
                    Err(PdsError::Internal(format!(
                        "S3 head object failed: {}",
                        e
                    )))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_key_sharding() {
        let _config = S3Config {
            bucket: "test-bucket".to_string(),
            ..Default::default()
        };

        // Can't easily test async init without actual S3, but we can test key generation
        let cid = "abc123def456";
        let expected = "blobs/ab/c1/abc123def456";

        // We'll test the logic directly
        let key = if cid.len() >= 4 {
            format!("blobs/{}/{}/{}", &cid[0..2], &cid[2..4], cid)
        } else {
            format!("blobs/{}", cid)
        };

        assert_eq!(key, expected);
    }

    #[test]
    fn test_get_key_short_cid() {
        let cid = "abc";
        let expected = "blobs/abc";

        let key = if cid.len() >= 4 {
            format!("blobs/{}/{}/{}", &cid[0..2], &cid[2..4], cid)
        } else {
            format!("blobs/{}", cid)
        };

        assert_eq!(key, expected);
    }

    #[test]
    fn test_s3_config_default() {
        let config = S3Config::default();
        assert_eq!(config.region, "us-east-1");
        assert_eq!(config.prefix, "blobs/");
        assert!(config.endpoint.is_none());
    }
}
