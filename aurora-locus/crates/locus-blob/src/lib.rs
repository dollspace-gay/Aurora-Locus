//! Blob storage (disk now; S3-compatible later), and the jobs that keep it
//! consistent with the blob rows: finishing moves, deleting queued keys
//! after a grace period, and garbage collection.
//!
//! Bytes are written to `<key>.part`, fsynced and renamed, so a key either
//! holds a complete copy or doesn't exist. Every copy has its own key, so
//! no step overwrites or deletes another copy (see `locus_store::blob`).

use std::path::{Component, Path, PathBuf};

use futures::{Stream, StreamExt};
use locus_core::lease::LeaseToken;
use locus_core::{CoreError, CoreResult};
use locus_store::Store;
use locus_store::blob::PendingMove;
use proto_blue_lex_data::Cid;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How many leading bytes are kept for MIME sniffing.
const SNIFF_BYTES: usize = 8192;

#[derive(Debug, Error)]
pub enum BlobError {
    #[error("blob is larger than the {limit}-byte limit")]
    TooLarge { limit: u64 },
    #[error("upload stream failed: {0}")]
    Stream(String),
    #[error("blob store: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid storage key {0:?}")]
    BadKey(String),
    #[error("copy of {0} doesn't match its CID")]
    Corrupt(String),
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
        let rel = Path::new(key);
        let safe = !key.is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)));
        if !safe {
            return Err(BlobError::BadKey(key.to_owned()));
        }
        Ok(self.root.join(rel))
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

    /// Copies `from` to `to`, checking the bytes hash to `cid`.
    pub async fn copy_verified(&self, from: &str, to: &str, cid: &Cid) -> Result<(), BlobError> {
        let src = tokio::fs::File::open(self.path(from)?).await?;
        let stream = futures::stream::unfold(src, |mut f| async move {
            let mut buf = vec![0u8; 64 * 1024];
            match f.read(&mut buf).await {
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    Some((Ok::<_, std::io::Error>(buf), f))
                }
                Err(e) => Some((Err(e), f)),
            }
        });
        let written = self.write_stream(to, Box::pin(stream), u64::MAX).await?;
        if written.cid.digest != cid.digest {
            let _ = self.delete(to).await;
            return Err(BlobError::Corrupt(to.to_owned()));
        }
        Ok(())
    }

    /// Opens a stored copy, if it exists.
    pub async fn open(&self, key: &str) -> Result<Option<tokio::fs::File>, BlobError> {
        match tokio::fs::File::open(self.path(key)?).await {
            Ok(f) => Ok(Some(f)),
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
pub async fn finish_move(store: &Store, disk: &DiskStore, m: &PendingMove) -> CoreResult<bool> {
    let cid = Cid::from_str_multibase(&m.cid).map_err(|e| CoreError::Storage(e.to_string()))?;
    disk.copy_verified(&m.from, &m.to, &cid).await?;
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
pub async fn run_pending_moves(store: &Store, disk: &DiskStore, limit: i64) -> CoreResult<usize> {
    let mut n = 0;
    for m in store.pending_blob_moves(limit).await? {
        match finish_move(store, disk, &m).await {
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

/// Deletes queued keys older than `grace_ms`, skipping (and dropping from
/// the queue) any key a row still names.
pub async fn run_deletes(
    store: &Store,
    disk: &DiskStore,
    grace_ms: i64,
    limit: i64,
) -> CoreResult<usize> {
    let mut n = 0;
    for (id, key, _epoch) in store.due_blob_deletes(grace_ms, limit).await? {
        if !store.blob_key_in_use(&key).await? {
            if let Err(e) = disk.delete(&key).await {
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

    #[tokio::test]
    async fn writes_are_atomic_hashed_and_capped() {
        let dir = tempfile::tempdir().unwrap();
        let disk = DiskStore::new(dir.path());
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
        assert!(disk.open("blobs/did:plc:x/c.1").await.unwrap().is_some());
        let wrong = Cid::for_raw(b"other");
        assert!(matches!(
            disk.copy_verified("tmp/a", "blobs/did:plc:x/c.2", &wrong)
                .await,
            Err(BlobError::Corrupt(_))
        ));
        assert!(disk.open("blobs/did:plc:x/c.2").await.unwrap().is_none());

        disk.delete("tmp/a").await.unwrap();
        disk.delete("tmp/a").await.unwrap();
        assert!(disk.open("../escape").await.is_err());
        assert!(disk.open("/etc/passwd").await.is_err());
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
