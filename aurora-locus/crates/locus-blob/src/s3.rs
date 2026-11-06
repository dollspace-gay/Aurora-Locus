//! Blobs in an S3-compatible bucket (AWS, MinIO, R2, B2 and similar), with
//! a custom endpoint and path-style addressing. Gated blobs (`gated/…`) can
//! live in a bucket of their own.
//!
//! Writes stream in parts: anything under one part is a single
//! `PutObject`, anything larger a multipart upload, so an object only
//! appears once all of it is there.

use std::time::Duration;

use aws_sdk_s3::Client;
use aws_sdk_s3::config::http::HttpResponse;
use aws_sdk_s3::config::{
    BehaviorVersion, Credentials, Region, RequestChecksumCalculation, ResponseChecksumValidation,
};
use aws_sdk_s3::error::{DisplayErrorContext, SdkError};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
use aws_smithy_http_client::tls;
use futures::{Stream, StreamExt};
use proto_blue_lex_data::Cid;
use sha2::{Digest, Sha256};

use crate::{BlobError, Reader, SNIFF_BYTES, Written, check_key};

/// The size of each multipart part (S3's minimum is 5 MiB).
const PART: usize = 8 * 1024 * 1024;

/// Where an S3-compatible store is and how to sign in to it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct S3Config {
    pub bucket: String,
    /// A separate bucket for gated blobs; None keeps them in `bucket`.
    pub gated_bucket: Option<String>,
    pub region: Option<String>,
    pub endpoint: Option<String>,
    pub force_path_style: bool,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    /// The limit on each upload request.
    pub upload_timeout: Duration,
    /// The limit on every other request; None for the SDK's default.
    pub request_timeout: Option<Duration>,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("bucket", &self.bucket)
            .field("gated_bucket", &self.gated_bucket)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            .field("force_path_style", &self.force_path_style)
            .field("access_key_id", &self.access_key_id)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct S3Store {
    client: Client,
    bucket: String,
    gated_bucket: Option<String>,
    upload_timeout: Duration,
}

impl std::fmt::Debug for S3Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Store")
            .field("bucket", &self.bucket)
            .field("gated_bucket", &self.gated_bucket)
            .finish_non_exhaustive()
    }
}

fn backend<E>(e: SdkError<E, HttpResponse>) -> BlobError
where
    E: std::error::Error + 'static,
{
    BlobError::Backend(DisplayErrorContext(e).to_string())
}

fn not_found<E>(e: &SdkError<E, HttpResponse>) -> bool {
    e.raw_response().is_some_and(|r| r.status().as_u16() == 404)
}

struct Multipart {
    id: String,
    parts: Vec<CompletedPart>,
}

impl S3Store {
    /// A client for `config`. Nothing is contacted until the first request.
    pub fn new(config: &S3Config) -> Result<Self, BlobError> {
        if config.bucket.is_empty() {
            return Err(BlobError::Backend("S3 needs a bucket".into()));
        }
        if config.access_key_id.is_empty() || config.secret_access_key.is_empty() {
            return Err(BlobError::Backend(
                "S3 needs both an access key id and a secret access key".into(),
            ));
        }
        let http = aws_smithy_http_client::Builder::new()
            .tls_provider(tls::Provider::Rustls(
                tls::rustls_provider::CryptoMode::Ring,
            ))
            .build_https();
        let mut timeouts = aws_sdk_s3::config::timeout::TimeoutConfig::builder()
            .connect_timeout(Duration::from_secs(10));
        if let Some(t) = config.request_timeout {
            timeouts = timeouts.operation_attempt_timeout(t);
        }
        let mut b = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .http_client(http)
            .region(Region::new(
                config
                    .region
                    .clone()
                    .filter(|r| !r.is_empty())
                    .unwrap_or_else(|| "us-east-1".to_owned()),
            ))
            .credentials_provider(Credentials::new(
                config.access_key_id.clone(),
                config.secret_access_key.clone(),
                config.session_token.clone(),
                None,
                "locus",
            ))
            .force_path_style(config.force_path_style)
            .timeout_config(timeouts.build())
            // Checksums only where S3 requires them: many compatible stores
            // refuse the newer default ones.
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired);
        if let Some(e) = config.endpoint.as_deref().filter(|e| !e.is_empty()) {
            b = b.endpoint_url(e);
        }
        Ok(Self {
            client: Client::from_conf(b.build()),
            bucket: config.bucket.clone(),
            gated_bucket: config.gated_bucket.clone().filter(|g| !g.is_empty()),
            upload_timeout: config.upload_timeout,
        })
    }

    fn bucket_for(&self, key: &str) -> &str {
        match &self.gated_bucket {
            Some(g) if key.starts_with("gated/") => g,
            _ => &self.bucket,
        }
    }

    async fn timed<T>(
        &self,
        what: &str,
        fut: impl Future<Output = Result<T, BlobError>>,
    ) -> Result<T, BlobError> {
        tokio::time::timeout(self.upload_timeout, fut)
            .await
            .map_err(|_| BlobError::Backend(format!("{what} timed out")))?
    }

    async fn upload_part(
        &self,
        key: &str,
        mp: &mut Option<Multipart>,
        bytes: Vec<u8>,
    ) -> Result<(), BlobError> {
        let bucket = self.bucket_for(key);
        if mp.is_none() {
            let out = self
                .timed("starting an upload", async {
                    self.client
                        .create_multipart_upload()
                        .bucket(bucket)
                        .key(key)
                        .send()
                        .await
                        .map_err(backend)
                })
                .await?;
            let id = out
                .upload_id()
                .ok_or_else(|| BlobError::Backend("no upload id".into()))?;
            *mp = Some(Multipart {
                id: id.to_owned(),
                parts: Vec::new(),
            });
        }
        let Some(m) = mp.as_mut() else {
            return Err(BlobError::Backend("no upload".into()));
        };
        let number = i32::try_from(m.parts.len() + 1)
            .map_err(|_| BlobError::Backend("too many parts".into()))?;
        let out = self
            .timed("an upload part", async {
                self.client
                    .upload_part()
                    .bucket(bucket)
                    .key(key)
                    .upload_id(&m.id)
                    .part_number(number)
                    .body(ByteStream::from(bytes))
                    .send()
                    .await
                    .map_err(backend)
            })
            .await?;
        m.parts.push(
            CompletedPart::builder()
                .part_number(number)
                .set_e_tag(out.e_tag().map(str::to_owned))
                .build(),
        );
        Ok(())
    }

    async fn fill<S, B, E>(
        &self,
        key: &str,
        body: &mut S,
        limit: u64,
        mp: &mut Option<Multipart>,
    ) -> Result<Written, BlobError>
    where
        S: Stream<Item = Result<B, E>> + Unpin,
        B: AsRef<[u8]>,
        E: std::fmt::Display,
    {
        let mut hasher = Sha256::new();
        let mut size: u64 = 0;
        let mut head = Vec::new();
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| BlobError::Stream(e.to_string()))?;
            let bytes = chunk.as_ref();
            size += bytes.len() as u64;
            if size > limit {
                return Err(BlobError::TooLarge { limit });
            }
            if head.len() < SNIFF_BYTES {
                let take = (SNIFF_BYTES - head.len()).min(bytes.len());
                head.extend_from_slice(&bytes[..take]);
            }
            hasher.update(bytes);
            buf.extend_from_slice(bytes);
            while buf.len() >= PART {
                let rest = buf.split_off(PART);
                let part = std::mem::replace(&mut buf, rest);
                self.upload_part(key, mp, part).await?;
            }
        }
        let bucket = self.bucket_for(key);
        if mp.is_none() {
            self.timed("an upload", async {
                self.client
                    .put_object()
                    .bucket(bucket)
                    .key(key)
                    .body(ByteStream::from(buf))
                    .send()
                    .await
                    .map_err(backend)
            })
            .await?;
        } else {
            if !buf.is_empty() {
                self.upload_part(key, mp, buf).await?;
            }
            let Some(m) = mp.as_ref() else {
                return Err(BlobError::Backend("no upload".into()));
            };
            self.timed("finishing an upload", async {
                self.client
                    .complete_multipart_upload()
                    .bucket(bucket)
                    .key(key)
                    .upload_id(&m.id)
                    .multipart_upload(
                        CompletedMultipartUpload::builder()
                            .set_parts(Some(m.parts.clone()))
                            .build(),
                    )
                    .send()
                    .await
                    .map_err(backend)
            })
            .await?;
        }
        Ok(Written {
            size,
            cid: Cid::for_raw_hash(hasher.finalize().into()),
            head,
        })
    }

    /// Streams `body` to `key`, refusing more than `limit` bytes.
    pub async fn write_stream<S, B, E>(
        &self,
        key: &str,
        mut body: S,
        limit: u64,
    ) -> Result<Written, BlobError>
    where
        S: Stream<Item = Result<B, E>> + Unpin,
        B: AsRef<[u8]>,
        E: std::fmt::Display,
    {
        check_key(key)?;
        let mut mp = None;
        let result = self.fill(key, &mut body, limit, &mut mp).await;
        if result.is_err()
            && let Some(m) = mp
        {
            let abort = self
                .client
                .abort_multipart_upload()
                .bucket(self.bucket_for(key))
                .key(key)
                .upload_id(&m.id)
                .send()
                .await;
            if let Err(e) = abort {
                tracing::warn!(
                    key,
                    "abandoned upload not aborted (a bucket lifecycle rule can clean it up): {}",
                    DisplayErrorContext(e)
                );
            }
        }
        result
    }

    /// Reads a stored copy, or the bytes `first..=last` of it.
    pub async fn read(
        &self,
        key: &str,
        range: Option<(u64, u64)>,
    ) -> Result<Option<Reader>, BlobError> {
        check_key(key)?;
        let mut req = self
            .client
            .get_object()
            .bucket(self.bucket_for(key))
            .key(key);
        if let Some((first, last)) = range {
            req = req.range(format!("bytes={first}-{last}"));
        }
        match req.send().await {
            Ok(out) => {
                let len = out
                    .content_length()
                    .and_then(|n| u64::try_from(n).ok())
                    .unwrap_or(0);
                let body = tokio_util::io::ReaderStream::new(out.body.into_async_read());
                Ok(Some(Reader {
                    len,
                    body: Box::pin(body),
                }))
            }
            Err(e) if not_found(&e) => Ok(None),
            Err(e) => Err(backend(e)),
        }
    }

    /// A stored copy's size, if it exists.
    pub async fn size(&self, key: &str) -> Result<Option<u64>, BlobError> {
        check_key(key)?;
        match self
            .client
            .head_object()
            .bucket(self.bucket_for(key))
            .key(key)
            .send()
            .await
        {
            Ok(out) => Ok(Some(
                out.content_length()
                    .and_then(|n| u64::try_from(n).ok())
                    .unwrap_or(0),
            )),
            Err(e) if not_found(&e) => Ok(None),
            Err(e) => Err(backend(e)),
        }
    }

    /// Deletes a copy (without a version id, so a versioned bucket keeps
    /// it as a noncurrent version); a missing one is success.
    pub async fn delete(&self, key: &str) -> Result<(), BlobError> {
        check_key(key)?;
        match self
            .client
            .delete_object()
            .bucket(self.bucket_for(key))
            .key(key)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if not_found(&e) => Ok(()),
            Err(e) => Err(backend(e)),
        }
    }

    /// On a versioned bucket, makes the newest version of `key` that isn't
    /// a delete marker current again. Returns whether there was one.
    pub async fn restore_newest_version(&self, key: &str) -> Result<bool, BlobError> {
        check_key(key)?;
        let bucket = self.bucket_for(key);
        let out = self
            .client
            .list_object_versions()
            .bucket(bucket)
            .prefix(key)
            .send()
            .await
            .map_err(backend)?;
        // S3 lists a key's versions newest first (timestamps can tie).
        let newest = out.versions().iter().find(|v| v.key() == Some(key));
        let Some(version) = newest.and_then(|v| v.version_id()) else {
            return Ok(false);
        };
        let source = format!(
            "{bucket}/{}?versionId={}",
            percent_path(key),
            percent_path(version)
        );
        self.client
            .copy_object()
            .bucket(bucket)
            .key(key)
            .copy_source(source)
            .send()
            .await
            .map_err(backend)?;
        Ok(true)
    }

    /// Whether every bucket answers.
    pub async fn check(&self) -> Result<(), BlobError> {
        let mut buckets = vec![self.bucket.as_str()];
        buckets.extend(self.gated_bucket.as_deref());
        for b in buckets {
            self.client
                .head_bucket()
                .bucket(b)
                .send()
                .await
                .map_err(|e| {
                    BlobError::Backend(format!("bucket {b}: {}", DisplayErrorContext(e)))
                })?;
        }
        Ok(())
    }
}

/// A key for a copy source: everything but unreserved characters and `/`
/// percent-encoded.
fn percent_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
