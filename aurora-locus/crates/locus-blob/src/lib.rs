//! Blob storage (local disk or an S3-compatible bucket), and the jobs that
//! keep it consistent with the blob rows: finishing moves, placing blobs in
//! the store their state calls for, deleting queued keys after a grace
//! period, and garbage collection.
//!
//! Every copy has its own key, so no step overwrites or deletes another
//! copy (see `locus_store::blob`). A key either holds a complete copy or
//! doesn't exist: on disk, bytes go to `<key>.part`, are fsynced and
//! renamed; on S3 an object appears only when its upload completes.

mod s3;

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use locus_core::keys::MasterKey;
use locus_core::lease::LeaseToken;
use locus_core::settings::Snapshot;
use locus_core::{CoreError, CoreResult};
use locus_store::Store;
use locus_store::blob::PendingMove;
use proto_blue_lex_data::Cid;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

pub use s3::{S3Config, S3Store};

/// How many leading bytes are kept for MIME sniffing.
pub(crate) const SNIFF_BYTES: usize = 8192;

#[derive(Debug, Error)]
pub enum BlobError {
    #[error("blob is larger than the {limit}-byte limit")]
    TooLarge { limit: u64 },
    #[error("upload stream failed: {0}")]
    Stream(String),
    #[error("blob store: {0}")]
    Io(#[from] std::io::Error),
    #[error("blob store: {0}")]
    Backend(String),
    #[error("invalid storage key {0:?}")]
    BadKey(String),
    #[error("copy of {0} doesn't match its CID")]
    Corrupt(String),
    #[error("{0} isn't in the blob store")]
    Missing(String),
}

impl From<BlobError> for CoreError {
    fn from(e: BlobError) -> Self {
        CoreError::Storage(e.to_string())
    }
}

/// What a write produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub size: u64,
    pub cid: Cid,
    /// The first bytes, for MIME sniffing.
    pub head: Vec<u8>,
}

/// The bytes of a read.
pub type ByteStream = Pin<Box<dyn Stream<Item = std::io::Result<Bytes>> + Send>>;

/// A stored copy being read.
pub struct Reader {
    /// How many bytes `body` yields.
    pub len: u64,
    pub body: ByteStream,
}

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reader").field("len", &self.len).finish()
    }
}

/// A key is relative and plain: no empty, `.`, `..` or root parts.
pub(crate) fn check_key(key: &str) -> Result<(), BlobError> {
    let safe = !key.is_empty()
        && !key.starts_with('/')
        && Path::new(key)
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    if safe {
        Ok(())
    } else {
        Err(BlobError::BadKey(key.to_owned()))
    }
}

/// Blobs on local disk under one root.
#[derive(Debug, Clone)]
pub struct DiskStore {
    root: PathBuf,
}

impl DiskStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, key: &str) -> Result<PathBuf, BlobError> {
        check_key(key)?;
        Ok(self.root.join(key))
    }

    /// Where `key` lives on disk.
    pub fn local_path(&self, key: &str) -> Result<PathBuf, BlobError> {
        self.path(key)
    }

    /// Streams `body` to `key`, refusing more than `limit` bytes, and
    /// returns its size, raw CID and first bytes.
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
        let path = self.path(key)?;
        let part = part_path(&path);
        if let Some(dir) = part.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let mut file = tokio::fs::File::create(&part).await?;
        let mut hasher = Sha256::new();
        let mut size: u64 = 0;
        let mut head = Vec::new();
        let result: Result<(), BlobError> = async {
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
                file.write_all(bytes).await?;
            }
            file.sync_all().await?;
            Ok(())
        }
        .await;
        drop(file);
        if let Err(e) = result {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(e);
        }
        tokio::fs::rename(&part, &path).await?;
        Ok(Written {
            size,
            cid: Cid::for_raw_hash(hasher.finalize().into()),
            head,
        })
    }

    /// Reads a stored copy, or the bytes `first..=last` of it.
    pub async fn read(
        &self,
        key: &str,
        range: Option<(u64, u64)>,
    ) -> Result<Option<Reader>, BlobError> {
        let mut file = match tokio::fs::File::open(self.path(key)?).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let size = file.metadata().await?.len();
        let len = match range {
            Some((first, last)) => {
                file.seek(std::io::SeekFrom::Start(first)).await?;
                (last + 1).min(size).saturating_sub(first)
            }
            None => size,
        };
        Ok(Some(Reader {
            len,
            body: Box::pin(tokio_util::io::ReaderStream::new(file.take(len))),
        }))
    }

    /// A stored copy's size, if it exists.
    pub async fn size(&self, key: &str) -> Result<Option<u64>, BlobError> {
        match tokio::fs::metadata(self.path(key)?).await {
            Ok(m) => Ok(Some(m.len())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Deletes a copy; a missing one is success.
    pub async fn delete(&self, key: &str) -> Result<(), BlobError> {
        match tokio::fs::remove_file(self.path(key)?).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

fn part_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".part");
    PathBuf::from(p)
}

/// The active blob store.
#[derive(Debug, Clone)]
pub enum BlobStore {
    Disk(DiskStore),
    S3(S3Store),
}

impl BlobStore {
    /// The backend's name, as `blob_backend.active` records it.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Disk(_) => "disk",
            Self::S3(_) => "s3",
        }
    }

    /// Streams `body` to `key`, refusing more than `limit` bytes, and
    /// returns its size, raw CID and first bytes.
    pub async fn write_stream<S, B, E>(
        &self,
        key: &str,
        body: S,
        limit: u64,
    ) -> Result<Written, BlobError>
    where
        S: Stream<Item = Result<B, E>> + Unpin,
        B: AsRef<[u8]>,
        E: std::fmt::Display,
    {
        match self {
            Self::Disk(d) => d.write_stream(key, body, limit).await,
            Self::S3(s) => s.write_stream(key, body, limit).await,
        }
    }

    /// Reads a stored copy, or the bytes `first..=last` of it; None if
    /// there's no such key.
    pub async fn read(
        &self,
        key: &str,
        range: Option<(u64, u64)>,
    ) -> Result<Option<Reader>, BlobError> {
        match self {
            Self::Disk(d) => d.read(key, range).await,
            Self::S3(s) => s.read(key, range).await,
        }
    }

    /// A stored copy's size, if it exists.
    pub async fn size(&self, key: &str) -> Result<Option<u64>, BlobError> {
        match self {
            Self::Disk(d) => d.size(key).await,
            Self::S3(s) => s.size(key).await,
        }
    }

    /// Deletes a copy; a missing one is success.
    pub async fn delete(&self, key: &str) -> Result<(), BlobError> {
        match self {
            Self::Disk(d) => d.delete(key).await,
            Self::S3(s) => s.delete(key).await,
        }
    }

    /// Copies `from` to `to` in this store, checking the bytes hash to
    /// `cid`.
    pub async fn copy_verified(&self, from: &str, to: &str, cid: &Cid) -> Result<(), BlobError> {
        copy_verified_between(self, from, self, to, cid).await
    }

    /// Whether the store answers (for S3, every bucket).
    pub async fn check(&self) -> Result<(), BlobError> {
        match self {
            Self::Disk(_) => Ok(()),
            Self::S3(s) => s.check().await,
        }
    }
}

/// The key-store name of the S3 secret access key.
pub const S3_SECRET: &str = "blobs/s3_secret_access_key";

/// Stores `PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY` in the key store when it
/// differs from what's there.
pub async fn import_s3_secret(
    store: &Store,
    key: &MasterKey,
    env: &HashMap<String, String>,
) -> CoreResult<()> {
    let Some(v) = env
        .get("PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY")
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    else {
        return Ok(());
    };
    let current = store.get_secret(S3_SECRET, key).await?;
    if current.as_deref().map(Vec::as_slice) != Some(v.as_bytes()) {
        let mut tx = store.begin().await?;
        tx.put_secret(S3_SECRET, v.as_bytes(), key).await?;
        tx.commit().await?;
        tracing::info!("S3 secret access key applied from PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY");
    }
    Ok(())
}

/// The S3 settings, if a bucket is set. The access key falls back to
/// `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and `AWS_SESSION_TOKEN`.
pub async fn s3_config(
    store: &Store,
    key: &MasterKey,
    snap: &Snapshot,
    env: &HashMap<String, String>,
) -> CoreResult<Option<S3Config>> {
    let bucket = snap.text("blobs.s3_bucket").trim();
    if bucket.is_empty() {
        return Ok(None);
    }
    let text = |k: &str| Some(snap.text(k).trim().to_owned()).filter(|v| !v.is_empty());
    let env_var = |k: &str| {
        env.get(k)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let secret = store
        .get_secret(S3_SECRET, key)
        .await?
        .and_then(|b| String::from_utf8(b.to_vec()).ok())
        .filter(|s| !s.is_empty());
    let (access_key_id, secret_access_key, session_token) =
        match (text("blobs.s3_access_key_id"), secret) {
            (Some(id), Some(secret)) => (id, secret, None),
            (None, None) => (
                env_var("AWS_ACCESS_KEY_ID").unwrap_or_default(),
                env_var("AWS_SECRET_ACCESS_KEY").unwrap_or_default(),
                env_var("AWS_SESSION_TOKEN"),
            ),
            _ => {
                return Err(CoreError::Config(
                    "S3 needs both an access key id (PDS_BLOBSTORE_S3_ACCESS_KEY_ID) and a secret \
                     access key (PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY)"
                        .into(),
                ));
            }
        };
    let ms = |k: &str| u64::try_from(snap.int(k)).unwrap_or(0);
    Ok(Some(S3Config {
        bucket: bucket.to_owned(),
        gated_bucket: text("blobs.s3_gated_bucket"),
        region: text("blobs.s3_region"),
        endpoint: text("blobs.s3_endpoint"),
        force_path_style: snap.bool("blobs.s3_force_path_style"),
        access_key_id,
        secret_access_key,
        session_token,
        upload_timeout: Duration::from_millis(ms("blobs.s3_upload_timeout_ms").max(1_000)),
        request_timeout: Some(ms("blobs.s3_request_timeout_ms"))
            .filter(|n| *n > 0)
            .map(Duration::from_millis),
    }))
}

/// The blob store this node uses: the install's active backend
/// (`blob_backend.active`). An install that has stored nothing yet adopts
/// S3 when a bucket is configured; one whose blobs are on disk keeps them
/// there until `migrate blobs` moves them.
pub async fn open_blob_store(
    store: &Store,
    key: &MasterKey,
    snap: &Snapshot,
    env: &HashMap<String, String>,
    data_dir: &Path,
) -> CoreResult<BlobStore> {
    import_s3_secret(store, key, env).await?;
    let s3 = s3_config(store, key, snap, env).await?;
    let mut backend = store.blob_backend().await?;
    if backend.active == "disk" && s3.is_some() && store.adopt_blob_backend_if_empty("s3").await? {
        tracing::info!("blobs will be stored in S3 (a new install with a bucket configured)");
        backend = store.blob_backend().await?;
    }
    match (backend.active.as_str(), s3) {
        ("s3", Some(config)) => Ok(BlobStore::S3(S3Store::new(&config)?)),
        ("s3", None) => Err(CoreError::Config(
            "blobs are stored in S3, but no bucket is configured (PDS_BLOBSTORE_S3_BUCKET)".into(),
        )),
        ("disk", s3) => {
            if s3.is_some() {
                tracing::warn!(
                    "an S3 bucket is configured, but this install's blobs are on disk; \
                     `aurora-locus migrate blobs --to s3` moves them"
                );
            }
            let root = backend
                .disk_root
                .map_or_else(|| data_dir.to_path_buf(), PathBuf::from);
            Ok(BlobStore::Disk(DiskStore::new(root)))
        }
        (other, _) => Err(CoreError::Config(format!("unknown blob backend {other:?}"))),
    }
}

/// Copies `from` in `src` to `to` in `dst`, checking the bytes hash to
/// `cid`. A copy that doesn't match is deleted again.
pub async fn copy_verified_between(
    src: &BlobStore,
    from: &str,
    dst: &BlobStore,
    to: &str,
    cid: &Cid,
) -> Result<(), BlobError> {
    let r = src
        .read(from, None)
        .await?
        .ok_or_else(|| BlobError::Missing(from.to_owned()))?;
    let written = dst.write_stream(to, r.body, u64::MAX).await?;
    if written.cid.digest != cid.digest {
        let _ = dst.delete(to).await;
        return Err(BlobError::Corrupt(to.to_owned()));
    }
    Ok(())
}

/// The MIME type of an upload: a sniffed type wins when one is detected,
/// otherwise the request's `Content-Type`, otherwise
/// `application/octet-stream`, as the reference.
#[must_use]
pub fn mime_type(head: &[u8], content_type: Option<&str>) -> String {
    if let Some(kind) = infer::get(head) {
        return kind.mime_type().to_owned();
    }
    content_type
        .map(|c| c.split(';').next().unwrap_or(c).trim().to_ascii_lowercase())
        .filter(|c| c.contains('/') && !c.contains(char::is_whitespace))
        .unwrap_or_else(|| "application/octet-stream".to_owned())
}

/// Step 2 and 3 of a move: copy, then point the row at the new key. A
/// superseded move queues its copy instead. Returns whether it took effect.
pub async fn finish_move(store: &Store, blobs: &BlobStore, m: &PendingMove) -> CoreResult<bool> {
    let cid = Cid::from_str_multibase(&m.cid).map_err(|e| CoreError::Storage(e.to_string()))?;
    blobs.copy_verified(&m.from, &m.to, &cid).await?;
    let mut tx = store.begin().await?;
    let backend = tx.blob_backend().await?;
    if backend.paused {
        // A backend cutover is pausing key changes; try again later.
        tx.rollback().await?;
        return Ok(false);
    }
    let done = tx
        .finish_blob_move(&m.did, &m.cid, &m.to, backend.epoch)
        .await?;
    tx.commit().await?;
    Ok(done)
}

/// Finishes up to `limit` pending moves (a commit's moves normally finish
/// right after it; this catches any a crash or failure left).
pub async fn run_pending_moves(store: &Store, blobs: &BlobStore, limit: i64) -> CoreResult<usize> {
    let mut n = 0;
    for m in store.pending_blob_moves(limit).await? {
        match finish_move(store, blobs, &m).await {
            Ok(true) => n += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!(
                did = m.did,
                cid = m.cid,
                "blob move failed, will retry: {e}"
            ),
        }
    }
    Ok(n)
}

/// Starts a move for up to `limit` blobs whose bytes aren't in the store
/// their state calls for (normal, gated or quarantine), and finishes them.
/// `gate_on` is whether the private media gate is in effect. Returns how
/// many moves it started.
pub async fn run_placement(
    store: &Store,
    blobs: &BlobStore,
    gate_on: bool,
    limit: i64,
) -> CoreResult<usize> {
    let mut started = Vec::new();
    for (did, cid) in store.misplaced_blobs(gate_on, limit).await? {
        let mut tx = store.begin().await?;
        let epoch = tx.blob_backend().await?.epoch;
        let m = tx.place_blob(&did, &cid, gate_on, epoch).await?;
        tx.commit().await?;
        started.extend(m);
    }
    for m in &started {
        if let Err(e) = finish_move(store, blobs, m).await {
            tracing::warn!(
                did = m.did,
                cid = m.cid,
                "blob move failed, will retry: {e}"
            );
        }
    }
    Ok(started.len())
}

/// Deletes queued keys older than `grace_ms`, skipping (and dropping from
/// the queue) any key a row still names.
pub async fn run_deletes(
    store: &Store,
    blobs: &BlobStore,
    grace_ms: i64,
    limit: i64,
) -> CoreResult<usize> {
    let mut n = 0;
    for (id, key, _epoch) in store.due_blob_deletes(grace_ms, limit).await? {
        if !store.blob_key_in_use(&key).await? {
            if let Err(e) = blobs.delete(&key).await {
                tracing::warn!(key, "blob delete failed, will retry: {e}");
                store.bump_blob_delete(id).await?;
                continue;
            }
            n += 1;
        }
        let mut tx = store.begin().await?;
        tx.remove_blob_delete(id).await?;
        tx.commit().await?;
    }
    Ok(n)
}

/// GC passes until nothing is left to collect, each batch fenced on the
/// job's lease. Returns how many blob rows went.
pub async fn run_gc(
    store: &Store,
    lease: &LeaseToken,
    temp_ttl_ms: i64,
    grace_ms: i64,
    batch: i64,
) -> CoreResult<u64> {
    let mut total = 0;
    loop {
        let mut tx = store.begin().await?;
        tx.fence(lease).await?;
        let epoch = tx.blob_backend().await?.epoch;
        let n = tx
            .gc_blob_batch(temp_ttl_ms, grace_ms, batch, epoch)
            .await?;
        tx.commit().await?;
        total += n;
        if n < batch as u64 {
            return Ok(total);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(
        parts: &[&'static [u8]],
    ) -> impl Stream<Item = Result<&'static [u8], String>> + Unpin {
        futures::stream::iter(parts.iter().map(|p| Ok(*p)).collect::<Vec<_>>())
    }

    async fn bytes_of(r: Reader) -> Vec<u8> {
        let mut out = Vec::new();
        let mut body = r.body;
        while let Some(c) = body.next().await {
            out.extend_from_slice(&c.unwrap());
        }
        out
    }

    #[tokio::test]
    async fn writes_are_atomic_hashed_and_capped() {
        let dir = tempfile::tempdir().unwrap();
        let disk = BlobStore::Disk(DiskStore::new(dir.path()));
        let w = disk
            .write_stream("tmp/a", chunks(&[b"hello ", b"world"]), 100)
            .await
            .unwrap();
        assert_eq!(w.size, 11);
        assert_eq!(w.cid, Cid::for_raw(b"hello world"));
        assert!(!dir.path().join("tmp/a.part").exists());

        let err = disk
            .write_stream("tmp/b", chunks(&[b"0123456789"]), 5)
            .await
            .unwrap_err();
        assert!(matches!(err, BlobError::TooLarge { limit: 5 }));
        assert!(!dir.path().join("tmp/b").exists() && !dir.path().join("tmp/b.part").exists());

        disk.copy_verified("tmp/a", "blobs/did:plc:x/c.1", &w.cid)
            .await
            .unwrap();
        let r = disk
            .read("blobs/did:plc:x/c.1", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.len, 11);
        assert_eq!(bytes_of(r).await, b"hello world");
        let r = disk
            .read("blobs/did:plc:x/c.1", Some((6, 9)))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.len, 4);
        assert_eq!(bytes_of(r).await, b"worl");
        assert_eq!(disk.size("blobs/did:plc:x/c.1").await.unwrap(), Some(11));
        let wrong = Cid::for_raw(b"other");
        assert!(matches!(
            disk.copy_verified("tmp/a", "blobs/did:plc:x/c.2", &wrong)
                .await,
            Err(BlobError::Corrupt(_))
        ));
        assert!(
            disk.read("blobs/did:plc:x/c.2", None)
                .await
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            disk.copy_verified("tmp/none", "blobs/did:plc:x/c.3", &w.cid)
                .await,
            Err(BlobError::Missing(_))
        ));

        disk.delete("tmp/a").await.unwrap();
        disk.delete("tmp/a").await.unwrap();
        assert_eq!(disk.size("tmp/a").await.unwrap(), None);
        assert!(disk.read("../escape", None).await.is_err());
        assert!(disk.read("/etc/passwd", None).await.is_err());
    }

    /// An S3 store from `LOCUS_TEST_S3_*` (MinIO in a container, say), or
    /// None to skip.
    fn test_s3(gated: bool) -> Option<BlobStore> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let endpoint = var("LOCUS_TEST_S3_ENDPOINT")?;
        let config = S3Config {
            bucket: var("LOCUS_TEST_S3_BUCKET").unwrap_or_else(|| "locus-test".into()),
            gated_bucket: gated.then(|| var("LOCUS_TEST_S3_GATED_BUCKET")).flatten(),
            region: None,
            endpoint: Some(endpoint),
            force_path_style: true,
            access_key_id: var("AWS_ACCESS_KEY_ID").unwrap_or_default(),
            secret_access_key: var("AWS_SECRET_ACCESS_KEY").unwrap_or_default(),
            session_token: None,
            upload_timeout: Duration::from_secs(30),
            request_timeout: None,
        };
        Some(BlobStore::S3(S3Store::new(&config).unwrap()))
    }

    #[tokio::test]
    async fn s3_writes_reads_copies_and_deletes() {
        let Some(s3) = test_s3(true) else {
            eprintln!("LOCUS_TEST_S3_ENDPOINT isn't set; skipping");
            return;
        };
        s3.check().await.unwrap();
        let run = locus_store::blob::new_generation();
        let key = |k: &str| format!("tmp/{run}-{k}");

        let w = s3
            .write_stream(&key("a"), chunks(&[b"hello ", b"world"]), 100)
            .await
            .unwrap();
        assert_eq!((w.size, w.cid.clone()), (11, Cid::for_raw(b"hello world")));
        let r = s3.read(&key("a"), Some((6, 9))).await.unwrap().unwrap();
        assert_eq!((r.len, bytes_of(r).await), (4, b"worl".to_vec()));
        assert_eq!(s3.size(&key("a")).await.unwrap(), Some(11));
        assert!(s3.read(&key("none"), None).await.unwrap().is_none());
        assert_eq!(s3.size(&key("none")).await.unwrap(), None);

        // Larger than a part: a multipart upload, whole or not at all.
        let big: Vec<u8> = (0..(9 * 1024 * 1024 + 123))
            .map(|i: usize| (i % 251) as u8)
            .collect();
        let pieces: Vec<Result<Vec<u8>, String>> =
            big.chunks(1 << 20).map(|c| Ok(c.to_vec())).collect();
        let w = s3
            .write_stream(&key("big"), futures::stream::iter(pieces.clone()), u64::MAX)
            .await
            .unwrap();
        assert_eq!(w.cid, Cid::for_raw(&big));
        let r = s3.read(&key("big"), None).await.unwrap().unwrap();
        assert_eq!(r.len, big.len() as u64);
        assert!(bytes_of(r).await == big);
        let err = s3
            .write_stream(
                &key("too-big"),
                futures::stream::iter(pieces),
                9 * 1024 * 1024,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, BlobError::TooLarge { .. }));
        assert_eq!(
            s3.size(&key("too-big")).await.unwrap(),
            None,
            "never appeared"
        );

        // Copies are checked against the CID.
        let to = format!("blobs/did:plc:s3test/{run}.1");
        s3.copy_verified(&key("big"), &to, &w.cid).await.unwrap();
        assert_eq!(s3.size(&to).await.unwrap(), Some(big.len() as u64));
        let bad = format!("blobs/did:plc:s3test/{run}.2");
        assert!(matches!(
            s3.copy_verified(&key("a"), &bad, &w.cid).await,
            Err(BlobError::Corrupt(_))
        ));
        assert_eq!(s3.size(&bad).await.unwrap(), None);

        // Gated keys go to the gated bucket when there is one.
        if std::env::var("LOCUS_TEST_S3_GATED_BUCKET").is_ok_and(|v| !v.is_empty()) {
            let gated = format!("gated/did:plc:s3test/{run}.3");
            s3.copy_verified(&key("a"), &gated, &Cid::for_raw(b"hello world"))
                .await
                .unwrap();
            let main_only = test_s3(false).unwrap();
            assert_eq!(
                main_only.size(&gated).await.unwrap(),
                None,
                "not in the main bucket"
            );
            assert_eq!(s3.size(&gated).await.unwrap(), Some(11));
            s3.delete(&gated).await.unwrap();
        }

        for k in [key("a"), key("big"), to] {
            s3.delete(&k).await.unwrap();
            s3.delete(&k).await.unwrap();
            assert_eq!(s3.size(&k).await.unwrap(), None);
        }
        assert!(s3.read("../escape", None).await.is_err());
    }

    #[test]
    fn sniffed_types_win() {
        let png = [
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, b'I', b'H', b'D', b'R',
        ];
        assert_eq!(mime_type(&png, Some("text/plain")), "image/png");
        assert_eq!(
            mime_type(b"hello", Some("Text/Plain; charset=utf-8")),
            "text/plain"
        );
        assert_eq!(mime_type(b"hello", None), "application/octet-stream");
        assert_eq!(
            mime_type(b"hello", Some("garbage")),
            "application/octet-stream"
        );
    }
}
