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

    /// The directory keys are under.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where `key` lives on disk.
    pub fn local_path(&self, key: &str) -> Result<PathBuf, BlobError> {
        self.path(key)
    }

    /// Up to `limit` keys under `prefix` (a directory, `blobs/`), after
    /// `after`, in order, with their modification time (ms). A write in
    /// progress (`.part`) is listed too: a crashed one is an orphan.
    pub async fn list_page(
        &self,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, i64)>, BlobError> {
        let base = self.path(prefix.trim_end_matches('/'))?;
        let root = self.root.clone();
        let after = after.map(str::to_owned);
        tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            let mut stack = vec![base];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for e in entries.flatten() {
                    let path = e.path();
                    let Ok(meta) = e.metadata() else { continue };
                    if meta.is_dir() {
                        stack.push(path);
                        continue;
                    }
                    let Ok(rel) = path.strip_prefix(&root) else {
                        continue;
                    };
                    let key = rel.to_string_lossy().replace('\\', "/");
                    if after.as_deref().is_some_and(|a| key.as_str() <= a) {
                        continue;
                    }
                    let modified = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
                    out.push((key, modified));
                }
            }
            out.sort();
            out.truncate(limit);
            out
        })
        .await
        .map_err(|e| BlobError::Backend(e.to_string()))
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

#[derive(Debug, Clone)]
enum Inner {
    Disk(DiskStore),
    S3(S3Store),
}

/// A blob store (the active one, or a migration's source or target), and
/// the backend epoch it was opened at.
#[derive(Debug, Clone)]
pub struct BlobStore {
    inner: Inner,
    epoch: i64,
}

impl BlobStore {
    #[must_use]
    pub fn disk(d: DiskStore) -> Self {
        Self {
            inner: Inner::Disk(d),
            epoch: 0,
        }
    }

    #[must_use]
    pub fn s3(s: S3Store) -> Self {
        Self {
            inner: Inner::S3(s),
            epoch: 0,
        }
    }

    /// This store as the backend of `epoch`.
    #[must_use]
    pub fn with_epoch(mut self, epoch: i64) -> Self {
        self.epoch = epoch;
        self
    }

    /// The backend epoch this store was opened at: a write noted under
    /// another epoch can't name a key (its bytes are on another backend).
    #[must_use]
    pub fn epoch(&self) -> i64 {
        self.epoch
    }

    /// The backend's name, as `blob_backend.active` records it.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match &self.inner {
            Inner::Disk(_) => "disk",
            Inner::S3(_) => "s3",
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
        match &self.inner {
            Inner::Disk(d) => d.write_stream(key, body, limit).await,
            Inner::S3(s) => s.write_stream(key, body, limit).await,
        }
    }

    /// Reads a stored copy, or the bytes `first..=last` of it; None if
    /// there's no such key.
    pub async fn read(
        &self,
        key: &str,
        range: Option<(u64, u64)>,
    ) -> Result<Option<Reader>, BlobError> {
        match &self.inner {
            Inner::Disk(d) => d.read(key, range).await,
            Inner::S3(s) => s.read(key, range).await,
        }
    }

    /// A stored copy's size, if it exists.
    pub async fn size(&self, key: &str) -> Result<Option<u64>, BlobError> {
        match &self.inner {
            Inner::Disk(d) => d.size(key).await,
            Inner::S3(s) => s.size(key).await,
        }
    }

    /// Deletes a copy; a missing one is success.
    pub async fn delete(&self, key: &str) -> Result<(), BlobError> {
        match &self.inner {
            Inner::Disk(d) => d.delete(key).await,
            Inner::S3(s) => s.delete(key).await,
        }
    }

    /// Up to `limit` keys under `prefix` after `after`, in order, with
    /// their modification time (ms).
    pub async fn list_page(
        &self,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, i64)>, BlobError> {
        match &self.inner {
            Inner::Disk(d) => d.list_page(prefix, after, limit).await,
            Inner::S3(s) => s.list_page(prefix, after, limit).await,
        }
    }

    /// Copies `from` to `to` in this store, checking the bytes hash to
    /// `cid`.
    pub async fn copy_verified(&self, from: &str, to: &str, cid: &Cid) -> Result<(), BlobError> {
        copy_verified_between(self, from, self, to, cid).await
    }

    /// Brings back a deleted copy from the bucket's versions, where the
    /// store keeps them (S3 with versioning). Returns whether it did.
    pub async fn restore_newest_version(&self, key: &str) -> Result<bool, BlobError> {
        match &self.inner {
            Inner::Disk(_) => Ok(false),
            Inner::S3(s) => s.restore_newest_version(key).await,
        }
    }

    /// Whether the store answers (for S3, every bucket).
    pub async fn check(&self) -> Result<(), BlobError> {
        match &self.inner {
            Inner::Disk(_) => Ok(()),
            Inner::S3(s) => s.check().await,
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

/// S3 settings with each `PDS_BLOBSTORE_S3_*` (and
/// `LOCUS_BLOBSTORE_S3_GATED_BUCKET`) value in `env` taking precedence, as
/// a restore takes them: the restored settings, overridable by env. None
/// if there's no bucket either way.
#[must_use]
pub fn s3_env_overrides(
    config: Option<S3Config>,
    env: &HashMap<String, String>,
) -> Option<S3Config> {
    let get = |k: &str| {
        env.get(k)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let mut c = match (config, get("PDS_BLOBSTORE_S3_BUCKET")) {
        (Some(c), _) => c,
        (None, Some(_)) => S3Config {
            upload_timeout: Duration::from_secs(20),
            ..S3Config::default()
        },
        (None, None) => return None,
    };
    if let Some(v) = get("PDS_BLOBSTORE_S3_BUCKET") {
        c.bucket = v;
    }
    if let Some(v) = get("LOCUS_BLOBSTORE_S3_GATED_BUCKET") {
        c.gated_bucket = Some(v);
    }
    if let Some(v) = get("PDS_BLOBSTORE_S3_REGION") {
        c.region = Some(v);
    }
    if let Some(v) = get("PDS_BLOBSTORE_S3_ENDPOINT") {
        c.endpoint = Some(v);
    }
    if let Some(v) = get("PDS_BLOBSTORE_S3_FORCE_PATH_STYLE") {
        c.force_path_style = matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes");
    }
    if let Some(v) = get("PDS_BLOBSTORE_S3_ACCESS_KEY_ID") {
        c.access_key_id = v;
    }
    if let Some(v) = get("PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY") {
        c.secret_access_key = v;
    }
    Some(c)
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
        ("s3", Some(config)) => Ok(BlobStore::s3(S3Store::new(&config)?).with_epoch(backend.epoch)),
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
            Ok(BlobStore::disk(DiskStore::new(root)).with_epoch(backend.epoch))
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
    if backend.epoch != blobs.epoch() {
        // The backend changed since this node opened its store: the copy
        // is on the old one. The move restarts on the new backend.
        tx.queue_blob_delete(&m.to, blobs.epoch()).await?;
        tx.commit().await?;
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
    // While a backup runs: nothing until its delete mark is recorded,
    // then nothing queued after its snapshot, so every key it names is
    // still there to copy.
    let max_id = match store.backup_delete_limit().await? {
        None => None,
        Some(None) => return Ok(0),
        Some(Some(mark)) => Some(mark),
    };
    let mut n = 0;
    for (id, key, epoch) in store.due_blob_deletes(grace_ms, limit, max_id).await? {
        if epoch > blobs.epoch() {
            // Queued for a newer backend than this node's store: a node
            // that has reloaded takes it.
            continue;
        }
        // An older epoch's key is on the old backend, left to the
        // operator; a migration's flip re-queued it for this one.
        if epoch == blobs.epoch() && !store.blob_key_in_use(&key).await? {
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

/// The key prefixes locus writes; the sweep lists only these and the
/// in-place prefixes an import recorded, never the rest of a disk root.
pub const SWEPT_PREFIXES: &[&str] = &["blobs/", "gated/", "quarantine/", "branding/", "tmp/"];

/// The orphan sweep: lists the active store under locus's prefixes and
/// queues for deletion (through the delete job) every key older than
/// `min_age_ms` that nothing names. Returns how many were queued.
pub async fn run_orphan_sweep(
    store: &Store,
    blobs: &BlobStore,
    min_age_ms: i64,
) -> CoreResult<usize> {
    let backend = store.blob_backend().await?;
    if backend.paused || backend.epoch != blobs.epoch() {
        // A cutover is under way, or this node hasn't reloaded after one.
        return Ok(0);
    }
    let cutoff = locus_core::time::now_ms() - min_age_ms;
    let mut prefixes: Vec<String> = SWEPT_PREFIXES.iter().map(|p| (*p).to_owned()).collect();
    for p in store.import_prefixes().await? {
        prefixes.push(format!("{}/", p.trim_end_matches('/')));
    }
    let moving = store.pending_move_keys().await?;
    let mut queued = 0;
    for prefix in prefixes {
        let mut after: Option<String> = None;
        loop {
            let page = blobs
                .list_page(&prefix, after.as_deref(), 1000)
                .await
                .map_err(|e| CoreError::Storage(e.to_string()))?;
            let (Some(first), Some(last)) = (page.first(), page.last()) else {
                break;
            };
            let tracked = store.tracked_keys_in_range(&first.0, &last.0).await?;
            let orphans: Vec<&String> = page
                .iter()
                .filter(|(k, at)| *at < cutoff && !tracked.contains(k) && !moving.contains(k))
                .map(|(k, _)| k)
                .collect();
            if !orphans.is_empty() {
                let mut tx = store.begin().await?;
                let b = tx.blob_backend().await?;
                if b.paused || b.epoch != backend.epoch {
                    tx.rollback().await?;
                    return Ok(queued);
                }
                for k in &orphans {
                    tx.queue_blob_delete(k, b.epoch).await?;
                }
                tx.commit().await?;
                queued += orphans.len();
            }
            after = Some(last.0.clone());
            if page.len() < 1000 {
                break;
            }
        }
    }
    Ok(queued)
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
        let disk = BlobStore::disk(DiskStore::new(dir.path()));
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
        Some(BlobStore::s3(S3Store::new(&config).unwrap()))
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

    #[tokio::test]
    async fn s3_versions_bring_a_deleted_copy_back() {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let (Some(endpoint), Some(bucket)) = (
            var("LOCUS_TEST_S3_ENDPOINT"),
            var("LOCUS_TEST_S3_VERSIONED_BUCKET"),
        ) else {
            eprintln!("LOCUS_TEST_S3_VERSIONED_BUCKET isn't set; skipping");
            return;
        };
        let s3 = BlobStore::s3(
            S3Store::new(&S3Config {
                bucket,
                endpoint: Some(endpoint),
                force_path_style: true,
                access_key_id: var("AWS_ACCESS_KEY_ID").unwrap_or_default(),
                secret_access_key: var("AWS_SECRET_ACCESS_KEY").unwrap_or_default(),
                upload_timeout: Duration::from_secs(30),
                ..S3Config::default()
            })
            .unwrap(),
        );
        let key = format!("blobs/did:plc:v/{}.1", locus_store::blob::new_generation());
        s3.write_stream(&key, chunks(&[b"first"]), 100)
            .await
            .unwrap();
        s3.write_stream(&key, chunks(&[b"second"]), 100)
            .await
            .unwrap();
        s3.delete(&key).await.unwrap();
        assert_eq!(s3.size(&key).await.unwrap(), None);
        assert!(s3.restore_newest_version(&key).await.unwrap());
        let r = s3.read(&key, None).await.unwrap().unwrap();
        assert_eq!(bytes_of(r).await, b"second", "the newest version");
        let never = format!("blobs/did:plc:v/{}.1", locus_store::blob::new_generation());
        assert!(!s3.restore_newest_version(&never).await.unwrap());
        // Disk keeps no versions.
        let dir = tempfile::tempdir().unwrap();
        let disk = BlobStore::disk(DiskStore::new(dir.path()));
        assert!(!disk.restore_newest_version("blobs/x").await.unwrap());
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
