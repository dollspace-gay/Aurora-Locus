/// Disk-based blob storage backend
use crate::{
    blob_store::{BlobBackend, BlobListEntry, BlobListPage},
    error::{PdsError, PdsResult},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::time::SystemTime;
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
    #[allow(dead_code)] // Future blob directory management
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

        fs::write(&blob_path, data)
            .await
            .map_err(|e| PdsError::BlobStorage(format!("Failed to write blob {}: {}", cid, e)))?;

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

    async fn list_all_blobs(
        &self,
        cursor: Option<String>,
        page_size: usize,
    ) -> PdsResult<BlobListPage> {
        // Cursor format: "{shard}/{filename}". `None` means
        // start from the beginning of the first shard. The
        // walk yields entries strictly *after* the cursor
        // position so calling code can resume from the
        // previous page's `next_cursor` without seeing
        // duplicates.
        let (start_shard, after_filename) = parse_cursor(cursor.as_deref())?;

        // Empty base directory = empty store. tokio::fs::read_dir
        // on a missing directory is a NotFound error; treat as
        // empty page.
        let shards = match list_sorted_dir(&self.base_path).await {
            Ok(s) => s,
            Err(PdsError::BlobStorage(msg))
                if msg.contains("os error 2") || msg.contains("No such file") =>
            {
                return Ok(BlobListPage {
                    entries: Vec::new(),
                    next_cursor: None,
                });
            }
            Err(e) => return Err(e),
        };

        let mut entries: Vec<BlobListEntry> = Vec::with_capacity(page_size);

        for shard in shards.iter() {
            // Skip shards lexicographically before the cursor
            // shard. The exact-match shard still gets walked
            // (with an inner filename skip).
            if !start_shard.is_empty() && shard.as_str() < start_shard.as_str() {
                continue;
            }
            let shard_path = self.base_path.join(shard);
            let filenames = list_sorted_dir(&shard_path).await?;

            for filename in filenames.iter() {
                // Within the cursor's shard, skip filenames at
                // or before the cursor filename. Outside the
                // cursor's shard, walk all filenames.
                if !after_filename.is_empty()
                    && shard.as_str() == start_shard.as_str()
                    && filename.as_str() <= after_filename.as_str()
                {
                    continue;
                }

                let file_path = shard_path.join(filename);
                let metadata = fs::metadata(&file_path).await.map_err(|e| {
                    PdsError::BlobStorage(format!(
                        "Failed to stat blob {}/{}: {}",
                        shard, filename, e
                    ))
                })?;
                let modified = metadata.modified().map_err(|e| {
                    PdsError::BlobStorage(format!(
                        "Failed to read modified time for {}/{}: {}",
                        shard, filename, e
                    ))
                })?;
                let last_modified = system_time_to_datetime(modified);

                entries.push(BlobListEntry {
                    cid: filename.clone(),
                    last_modified,
                });

                if entries.len() >= page_size {
                    let next_cursor = Some(format!("{}/{}", shard, filename));
                    return Ok(BlobListPage {
                        entries,
                        next_cursor,
                    });
                }
            }
        }

        Ok(BlobListPage {
            entries,
            next_cursor: None,
        })
    }
}

/// Parse an opaque cursor into `(shard, filename)`. Empty
/// strings mean "start from the beginning."
fn parse_cursor(cursor: Option<&str>) -> PdsResult<(String, String)> {
    match cursor {
        None => Ok((String::new(), String::new())),
        Some(c) => {
            let (shard, filename) = c.split_once('/').ok_or_else(|| {
                PdsError::Validation(format!(
                    "Invalid list_all_blobs cursor (expected 'shard/filename'): {}",
                    c
                ))
            })?;
            Ok((shard.to_string(), filename.to_string()))
        }
    }
}

/// List a directory's entries (file or directory names) in
/// lexicographic order. Used both to walk shards and to walk
/// files within a shard.
async fn list_sorted_dir(path: &std::path::Path) -> PdsResult<Vec<String>> {
    let mut entries = fs::read_dir(path).await.map_err(|e| {
        PdsError::BlobStorage(format!("Failed to read dir {}: {}", path.display(), e))
    })?;
    let mut names = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| PdsError::BlobStorage(format!("Failed to read dir entry: {}", e)))?
    {
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

fn system_time_to_datetime(t: SystemTime) -> DateTime<Utc> {
    // Filesystem timestamps before the Unix epoch are not
    // expected for blob storage; clamp to epoch on the rare
    // failure path so the caller always gets a usable value.
    let duration = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    DateTime::<Utc>::from_timestamp(
        duration.as_secs() as i64,
        duration.subsec_nanos(),
    )
    .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
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

    // ---- Arc 10 Step 1 (chainlink #57): list_all_blobs ----

    /// Empty store: empty page + no continuation token.
    #[tokio::test]
    async fn test_list_all_blobs_empty_store() {
        let dir = tempdir().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());

        let page = backend.list_all_blobs(None, 100).await.unwrap();
        assert!(page.entries.is_empty());
        assert!(page.next_cursor.is_none());
    }

    /// Store with fewer blobs than page_size returns all entries
    /// with no continuation cursor.
    #[tokio::test]
    async fn test_list_all_blobs_single_page() {
        let dir = tempdir().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());

        for cid in ["bafyaaaa001", "bafyaaaa002", "bafycccc003"] {
            backend.put(cid, b"data".to_vec(), "text/plain").await.unwrap();
        }

        let page = backend.list_all_blobs(None, 100).await.unwrap();
        assert_eq!(page.entries.len(), 3);
        assert!(page.next_cursor.is_none());
        // Entries should be lexicographically ordered.
        let cids: Vec<&str> = page.entries.iter().map(|e| e.cid.as_str()).collect();
        assert_eq!(cids, vec!["bafyaaaa001", "bafyaaaa002", "bafycccc003"]);
    }

    /// More blobs than page_size: first call returns page_size
    /// + cursor; second call returns the rest + None.
    #[tokio::test]
    async fn test_list_all_blobs_multi_page() {
        let dir = tempdir().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());

        // Seed 5 blobs in the same shard ("ba") for predictability.
        for i in 1..=5 {
            let cid = format!("bafy0000{:03}", i);
            backend.put(&cid, b"data".to_vec(), "text/plain").await.unwrap();
        }

        let page1 = backend.list_all_blobs(None, 2).await.unwrap();
        assert_eq!(page1.entries.len(), 2);
        assert_eq!(page1.entries[0].cid, "bafy0000001");
        assert_eq!(page1.entries[1].cid, "bafy0000002");
        assert_eq!(page1.next_cursor.as_deref(), Some("ba/bafy0000002"));

        let page2 = backend.list_all_blobs(page1.next_cursor, 2).await.unwrap();
        assert_eq!(page2.entries.len(), 2);
        assert_eq!(page2.entries[0].cid, "bafy0000003");
        assert_eq!(page2.entries[1].cid, "bafy0000004");
        assert_eq!(page2.next_cursor.as_deref(), Some("ba/bafy0000004"));

        let page3 = backend.list_all_blobs(page2.next_cursor, 2).await.unwrap();
        assert_eq!(page3.entries.len(), 1);
        assert_eq!(page3.entries[0].cid, "bafy0000005");
        assert!(page3.next_cursor.is_none());
    }

    /// Pagination across shards: cursor in shard "ba" should
    /// continue into "ca" on the next page.
    #[tokio::test]
    async fn test_list_all_blobs_cursor_resumption_across_shards() {
        let dir = tempdir().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());

        // Seed in three shards: aa, bb, cc.
        for cid in [
            "aaxxx001", "aaxxx002",
            "bbyyy001", "bbyyy002",
            "cczzz001",
        ] {
            backend.put(cid, b"data".to_vec(), "text/plain").await.unwrap();
        }

        // First page (size 3) should cover aa + start of bb.
        let page1 = backend.list_all_blobs(None, 3).await.unwrap();
        let cids1: Vec<&str> = page1.entries.iter().map(|e| e.cid.as_str()).collect();
        assert_eq!(cids1, vec!["aaxxx001", "aaxxx002", "bbyyy001"]);
        assert_eq!(page1.next_cursor.as_deref(), Some("bb/bbyyy001"));

        // Second page should continue from bb/bbyyy001, picking up
        // bbyyy002 then walking into cc/cczzz001.
        let page2 = backend.list_all_blobs(page1.next_cursor, 3).await.unwrap();
        let cids2: Vec<&str> = page2.entries.iter().map(|e| e.cid.as_str()).collect();
        assert_eq!(cids2, vec!["bbyyy002", "cczzz001"]);
        assert!(page2.next_cursor.is_none());
    }

    /// `last_modified` is populated and reflects recent
    /// creation (within ~2 minutes of "now").
    #[tokio::test]
    async fn test_list_all_blobs_last_modified_populated() {
        let dir = tempdir().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());

        let before = chrono::Utc::now() - chrono::Duration::seconds(60);
        backend
            .put("bafymodified001", b"data".to_vec(), "text/plain")
            .await
            .unwrap();
        let after = chrono::Utc::now() + chrono::Duration::seconds(60);

        let page = backend.list_all_blobs(None, 10).await.unwrap();
        assert_eq!(page.entries.len(), 1);
        let entry = &page.entries[0];
        assert!(
            entry.last_modified >= before && entry.last_modified <= after,
            "last_modified {} not in window [{}, {}]",
            entry.last_modified,
            before,
            after
        );
    }

    /// Malformed cursor (no `/`) returns a Validation error
    /// rather than silently restarting — that silent restart
    /// would be a data-loss footgun for the sweep caller.
    #[tokio::test]
    async fn test_list_all_blobs_malformed_cursor_returns_error() {
        let dir = tempdir().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());

        let result = backend
            .list_all_blobs(Some("no-slash-here".to_string()), 10)
            .await;
        match result {
            Err(PdsError::Validation(msg)) => {
                assert!(msg.contains("Invalid list_all_blobs cursor"));
            }
            other => panic!("expected Validation error, got: {:?}", other),
        }
    }
}
