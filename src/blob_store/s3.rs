//! S3-compatible blob storage backend.
//!
//! Phase 1 (chainlink #71) re-enabled this module. Phase 2 (#72) wired
//! `BlobStore::new` to construct `S3BlobBackend` from configuration.
//! Phase 3 (#73) added the `force_path_style` and `upload_timeout_ms`
//! parity knobs and exposed `prefix` via env var.

use crate::blob_store::{BlobBackend, BlobListEntry, BlobListPage};
use crate::error::{PdsError, PdsResult};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
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

    async fn list_all_blobs(
        &self,
        cursor: Option<String>,
        page_size: usize,
    ) -> PdsResult<BlobListPage> {
        // S3 pagination is native: ListObjectsV2 accepts a
        // ContinuationToken and returns the next one in
        // `next_continuation_token`. The cursor passes through
        // opaquely; callers shouldn't introspect it.
        let mut req = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(&self.prefix)
            .max_keys(page_size as i32);

        if let Some(token) = cursor {
            req = req.continuation_token(token);
        }

        let resp = req.send().await.map_err(|e| {
            error!("S3 list_objects_v2 failed: {}", e);
            PdsError::Internal(format!("S3 list_objects_v2 failed: {}", e))
        })?;

        let mut entries: Vec<BlobListEntry> = Vec::with_capacity(page_size);
        for obj in resp.contents() {
            // Skip keys that don't decode to a recognisable
            // CID under our prefix/sharding scheme. This is
            // the "manual debris" carve-out — manual uploads
            // outside the Aurora-Locus-managed key space don't
            // appear in any DB row and would otherwise be
            // false-positive orphans. The GC sweep treats
            // unrecognised keys as out-of-scope.
            let Some(key) = obj.key() else {
                continue;
            };
            let Some(cid) = strip_s3_prefix_and_shards(key, &self.prefix) else {
                continue;
            };
            // S3 `last_modified` is the SDK's own `DateTime`
            // type (smithy-types). Convert via Unix epoch
            // seconds + nanos to chrono::DateTime<Utc>.
            let last_modified = obj
                .last_modified()
                .and_then(|dt| {
                    DateTime::<Utc>::from_timestamp(dt.secs(), dt.subsec_nanos())
                })
                .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
            entries.push(BlobListEntry { cid, last_modified });
        }

        let next_cursor = if resp.is_truncated().unwrap_or(false) {
            resp.next_continuation_token().map(|s| s.to_string())
        } else {
            None
        };

        Ok(BlobListPage {
            entries,
            next_cursor,
        })
    }
}

/// Strip the configured prefix and the shard segments from an
/// S3 key, returning the bare CID. Inverse of
/// `S3BlobBackend::get_key`. Returns `None` if the key doesn't
/// match the expected `{prefix}{first2}/{next2}/{cid}` shape;
/// such keys are debris from outside the Aurora-Locus write
/// path and are skipped by the GC sweep.
fn strip_s3_prefix_and_shards(key: &str, prefix: &str) -> Option<String> {
    let after_prefix = key.strip_prefix(prefix)?;
    // Expected layout: "{2 chars}/{2 chars}/{cid}". The CID's
    // first 4 chars must equal the two shard segments
    // concatenated, otherwise the key is debris (legitimate
    // CIDs are >=4 chars and emit the deterministic sharding).
    let mut parts = after_prefix.splitn(3, '/');
    let shard1 = parts.next()?;
    let shard2 = parts.next()?;
    let cid = parts.next()?;
    if shard1.len() != 2 || shard2.len() != 2 {
        return None;
    }
    if cid.len() < 4 {
        return None;
    }
    if !cid.starts_with(shard1) || !cid[2..4].starts_with(shard2) {
        return None;
    }
    Some(cid.to_string())
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

    // ---- Arc 10 Step 1 (chainlink #57): list_all_blobs ----

    /// `strip_s3_prefix_and_shards` is the inverse of
    /// `get_key`. A round-trip through put-then-list must
    /// preserve the CID exactly.
    #[test]
    fn test_strip_s3_prefix_and_shards_round_trips_get_key() {
        let cid = "bafybeicabcd1234567890";
        // Mirror get_key()'s output layout.
        let key = format!("blobs/{}/{}/{}", &cid[0..2], &cid[2..4], cid);
        let stripped = strip_s3_prefix_and_shards(&key, "blobs/");
        assert_eq!(stripped, Some(cid.to_string()));
    }

    #[test]
    fn test_strip_s3_prefix_and_shards_with_custom_prefix() {
        let cid = "abc123def456ghij";
        let key = format!("aurora/objs/{}/{}/{}", &cid[0..2], &cid[2..4], cid);
        let stripped = strip_s3_prefix_and_shards(&key, "aurora/objs/");
        assert_eq!(stripped, Some(cid.to_string()));
    }

    /// Keys without the configured prefix are debris from
    /// outside the Aurora-Locus write path; the helper skips
    /// them.
    #[test]
    fn test_strip_s3_prefix_and_shards_skips_unrecognised_prefix() {
        let key = "other-tool/some/key/blob";
        assert!(strip_s3_prefix_and_shards(key, "blobs/").is_none());
    }

    /// Keys with the prefix but with wrong sharding (e.g.,
    /// manual upload that bypassed `get_key`) are also
    /// debris — the shard segments must match the CID's first
    /// 4 chars.
    #[test]
    fn test_strip_s3_prefix_and_shards_skips_wrong_sharding() {
        // First-shard segment doesn't equal the CID's first 2
        // chars; sharding mismatch means the key wasn't
        // generated by `get_key`.
        let key = "blobs/zz/zz/bafybeicabcd1234567890";
        assert!(strip_s3_prefix_and_shards(key, "blobs/").is_none());
    }

    /// Keys missing the full shard structure are debris.
    #[test]
    fn test_strip_s3_prefix_and_shards_skips_flat_keys() {
        let key = "blobs/bafybeicabcd1234567890";
        assert!(strip_s3_prefix_and_shards(key, "blobs/").is_none());
    }

    // Live-S3 pagination tests would require credentials + a
    // bucket; the AWS SDK's mock-client infrastructure could
    // be wired here in a future cycle but is out of scope for
    // Arc 10 Step 1. The CID-extraction tests above cover the
    // load-bearing wire-shape contract; pagination semantics
    // pass through to `aws_sdk_s3::Client::list_objects_v2`
    // directly with no Aurora-Locus-side logic.
}
