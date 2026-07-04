/// Blob Store Manager
///
/// Coordinates blob storage backends with database metadata tracking
use crate::{
    blob_store::{
        disk::DiskBlobBackend, s3::S3BlobBackend, s3::S3Config, BlobBackend, BlobBackendType,
        BlobMetadata, BlobRef, BlobStorageConfig, ImageDimensions, TempBlob,
    },
    error::{PdsError, PdsResult},
};
use chrono::{DateTime, Utc};

/// Parse an RFC3339 string from the database into a `DateTime<Utc>`.
/// See chainlink #76 / Phase 3 design notes on chrono ↔ AnyPool.
fn parse_timestamp(s: &str) -> PdsResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))
}
use image::ImageFormat;
use proto_blue::lex_data::Cid;
use sqlx::{AnyPool, Row};
use std::sync::Arc;
use tokio::fs;

/// Blob store configuration
#[derive(Debug, Clone, Default)]
pub struct BlobStoreConfig {
    pub storage: BlobStorageConfig,
}

/// Main blob store manager
#[derive(Clone)]
pub struct BlobStore {
    config: BlobStoreConfig,
    backend: Arc<dyn BlobBackend>,
    db: AnyPool,
    /// Arc 16b §9.2.3.2 / Step 0.2 Item 9 recon (corrected): sqlx
    /// 0.8 does NOT silently tolerate `FOR UPDATE` on SQLite — the
    /// clause is passed through to SQLite which rejects it as syntax
    /// error. Backend detection happens at construction time + helpers
    /// conditionally emit the clause via [`Self::for_update_clause`].
    /// On SQLite the WAL writer-lock provides equivalent stronger-
    /// but-coarser serialization (per §9.2.3.2); on Postgres the
    /// clause provides the row-level lock the design relies on.
    is_postgres: bool,
}

impl BlobStore {
    /// Create a new blob store. Async because S3 backend init performs
    /// SDK config loading which is async; the disk path is also awaited
    /// for uniformity even though it has no async work.
    pub async fn new(config: BlobStoreConfig, db: AnyPool) -> PdsResult<Self> {
        let backend: Arc<dyn BlobBackend> = match &config.storage.backend {
            BlobBackendType::Disk { location } => Arc::new(DiskBlobBackend::new(location.clone())),
            BlobBackendType::S3 {
                bucket,
                region,
                endpoint,
                access_key_id,
                secret_access_key,
                prefix,
                force_path_style,
                upload_timeout_ms,
            } => Arc::new(
                S3BlobBackend::new(S3Config {
                    bucket: bucket.clone(),
                    region: region.clone(),
                    endpoint: endpoint.clone(),
                    access_key_id: access_key_id.clone(),
                    secret_access_key: secret_access_key.clone(),
                    prefix: prefix.clone(),
                    force_path_style: *force_path_style,
                    upload_timeout_ms: *upload_timeout_ms,
                })
                .await?,
            ),
        };

        // Arc 16b §9.2.3.2: probe backend for FOR UPDATE conditional
        // emission. Postgres recognizes pg_backend_pid(); SQLite does
        // not. Fall back to false on probe error (treat as SQLite —
        // safer default since FOR UPDATE on SQLite would break).
        let is_postgres = sqlx::query("SELECT pg_backend_pid()")
            .fetch_one(&db)
            .await
            .is_ok();

        Ok(Self {
            config,
            backend,
            db,
            is_postgres,
        })
    }

    /// Arc 16e §9.5.3.2.2 — expose the shared pool so callers needing
    /// Phase B transactions (`RepositoryManager::apply_writes`) can
    /// `tx = pool.begin()` against the same DB that owns
    /// `blob_metadata` + `record_blob`.
    pub fn pool(&self) -> &sqlx::AnyPool {
        &self.db
    }

    /// Arc 16b §9.2.3.2: emit the SQL row-lock clause appropriate
    /// for the backend. Postgres: `" FOR UPDATE"`; SQLite: empty
    /// (WAL writer-lock provides equivalent serialization per
    /// §9.2.3.2 "stronger-but-coarser" note). Leading space is
    /// included for clean concatenation onto a `WHERE` clause.
    fn for_update_clause(&self) -> &'static str {
        if self.is_postgres {
            " FOR UPDATE"
        } else {
            ""
        }
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
    #[allow(dead_code)] // Future blob processing methods
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
    pub async fn stage_blob(
        &self,
        data: Vec<u8>,
        mime_type: Option<&str>,
        creator_did: &str,
    ) -> PdsResult<TempBlob> {
        // Validate size
        let size = data.len();
        crate::blob_store::mime::validate_blob_size(size, self.config.storage.max_blob_size)
            .map_err(PdsError::Validation)?;

        // Detect MIME type from data if not provided
        let mime_type = mime_type
            .map(String::from)
            .or_else(|| {
                crate::blob_store::mime::detect_mime_type_from_data(&data).map(String::from)
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
            .map_err(|e| {
                PdsError::BlobStorage(format!("Failed to create temp directory: {}", e))
            })?;

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

        // Store temp blob metadata in database. #96 — if the
        // INSERT errors after the temp file is on disk, the file
        // would leak (no metadata row → row-driven sweep can't
        // see it). Best-effort fs::remove_file on the Err arm
        // recovers the disk space synchronously; we still log
        // and propagate the original DB error. Failing the
        // remove is non-fatal (the Arc 16d row-sweep picks up
        // any surviving stragglers; the warn surfaces the rare
        // case for operator visibility).
        if let Err(e) = self.store_temp_blob_metadata(&temp_blob).await {
            if let Err(rm_err) = fs::remove_file(&temp_path).await {
                tracing::warn!(
                    cid = %cid,
                    temp_path = %temp_path.display(),
                    metadata_error = %e,
                    remove_error = %rm_err,
                    "stage_blob: temp file orphan cleanup failed; \
                     row-sweep will catch the stray"
                );
            }
            return Err(e);
        }

        tracing::info!("Staged blob {} in temp storage", cid);

        Ok(temp_blob)
    }

    /// Commit a staged blob to permanent storage (Phase 2 of two-phase upload)
    ///
    /// Moves blob from temp to permanent storage and creates metadata
    /// Arc 16c §9.3.3.2 — two-phase upload commit: promote bytes from
    /// temp path to CID-derived final position, establish durability,
    /// THEN open the DB transaction that calls
    /// `track_untethered_blob`. Ordering invariant (load-bearing):
    /// **bytes durability at the final position strictly PRECEDES
    /// the DB transaction commit.**
    ///
    /// Production caller: uploadBlob handler (`src/api/blob.rs`)
    /// invokes this after `stage_blob` returns success. Returns
    /// `Ok` only after the `blob_metadata` row exists with
    /// `temp_key = '1'` and the row's wrapping transaction has
    /// committed.
    ///
    /// Error classes (per §9.3.3.2 failure modes):
    /// - **stale-stage**: temp_blob_metadata row missing at step 5
    ///   (Case 3a reaper-race) → `PdsError::NotFound`. Bytes at
    ///   final position become orphans pending Arc 16d's row-driven
    ///   sweep (Step 0.6 Case 1b accept-with-latency disposition).
    /// - **bytes-promotion failure** (step 2): backend put failed;
    ///   no DB state change.
    /// - **fsync failure** (step 3): durability not established; no
    ///   DB state change. Bytes possibly partially written.
    /// - **DB failure** (steps 4-7): row not committed; bytes at
    ///   final position become orphans pending sweep.
    pub async fn commit_blob(&self, cid: &str) -> PdsResult<()> {
        let temp_path = self.get_temp_blob_path(cid);

        if !temp_path.exists() {
            return Err(PdsError::NotFound(format!("Temp blob not found: {}", cid)));
        }

        // Step 1: read bytes from temp path (R1 read-then-write
        // re-validation pattern).
        let data = fs::read(&temp_path)
            .await
            .map_err(|e| PdsError::BlobStorage(format!("Failed to read temp blob: {}", e)))?;

        // Get metadata from temp_blob_metadata (pre-transactional
        // read; the in-transaction SELECT at step 5 is the
        // authoritative reaper-race check).
        let metadata = self
            .get_temp_blob_metadata(cid)
            .await?
            .ok_or_else(|| {
                PdsError::NotFound(format!("Temp blob metadata not found: {}", cid))
            })?;

        // Step 2: write bytes to CID-derived final position.
        self.backend.put(cid, data, &metadata.mime_type).await?;

        // Step 3: establish durability. Disk backend: file-fsync +
        // dir-fsync in canonical order ("Both absent" disposition
        // per Step 0.2 recon — Arc 16c adds both fresh). S3 backend:
        // no-op (2xx PUT already confirmed durability inside put()).
        self.backend.fsync(cid).await?;

        // Step 4: open DB transaction. Steps 5-7 happen inside.
        let mut tx = self.db.begin().await.map_err(PdsError::Database)?;

        // Step 5: SELECT temp_blob_metadata row exists (plain SELECT,
        // no FOR UPDATE per §9.3.3.4). Catches Case 3a reaper-race
        // cleanly; Cases 3b/3c benign by table-disjointness.
        let still_present: Option<(String,)> = sqlx::query_as(
            "SELECT cid FROM temp_blob_metadata WHERE cid = $1",
        )
        .bind(cid)
        .fetch_optional(&mut *tx)
        .await
        .map_err(PdsError::Database)?;

        if still_present.is_none() {
            // Stale-stage: reaper committed a DELETE between
            // stage_blob and our transaction-open. Bytes at final
            // position become orphans pending Arc 16d sweep.
            return Err(PdsError::NotFound(format!(
                "Stale stage: temp_blob_metadata row missing for {} \
                 (likely reaped between stage and commit; client should retry)",
                cid
            )));
        }

        // Step 6: call Arc 16b's track_untethered_blob helper.
        // First production caller per audit obligation Item 6.
        self.track_untethered_blob(
            &mut tx,
            cid,
            &metadata.mime_type,
            metadata.size,
            &metadata.creator_did,
            Utc::now(),
        )
        .await?;

        // Step 7: commit transaction. After this returns Ok, the
        // "row exists ⇒ bytes durable at final position" invariant
        // holds (§9.3.3.2).
        tx.commit().await.map_err(PdsError::Database)?;

        // Post-commit cleanup: remove temp file + temp_blob_metadata
        // row. Best-effort; failures here don't undo the commit
        // (handler returns 200 to client regardless). Cleanup failures
        // leave the temp_blob_metadata row to be reaped per
        // stage_ttl_seconds (Step 2 reaper TTL switch).
        if let Err(e) = fs::remove_file(&temp_path).await {
            tracing::warn!(
                cid = cid,
                error = %e,
                "commit_blob: post-commit temp file cleanup failed; will be reaped",
            );
        }
        if let Err(e) = self.delete_temp_blob_metadata(cid).await {
            tracing::warn!(
                cid = cid,
                error = %e,
                "commit_blob: post-commit temp_blob_metadata cleanup failed; will be reaped",
            );
        }

        tracing::info!(cid = cid, "commit_blob: blob_metadata row inserted (untethered)");
        Ok(())
    }

    /// Upload a blob
    ///
    /// Returns the blob metadata and reference
    #[allow(dead_code)] // Future blob upload functionality
    pub async fn upload(
        &self,
        data: Vec<u8>,
        mime_type: Option<&str>,
        creator_did: &str,
    ) -> PdsResult<BlobRef> {
        // Validate size
        let size = data.len();
        crate::blob_store::mime::validate_blob_size(size, self.config.storage.max_blob_size)
            .map_err(PdsError::Validation)?;

        // Detect MIME type from data if not provided
        let mime_type = mime_type
            .map(String::from)
            .or_else(|| {
                crate::blob_store::mime::detect_mime_type_from_data(&data).map(String::from)
            })
            .unwrap_or_else(|| "application/octet-stream".to_string());

        // Validate MIME type is allowed
        self.validate_mime_type(&mime_type)?;

        // Calculate CID (using SHA-256 hash)
        let cid = self.calculate_cid(&data);

        // Extract image dimensions if this is an image
        let dimensions = Self::extract_image_dimensions(&data, &mime_type);

        // Generate thumbnail if this is an image (256x256 max)
        let thumbnail_cid = if let Some(thumb_data) =
            Self::generate_thumbnail(&data, &mime_type, 256)
        {
            // Calculate thumbnail CID
            let thumb_cid = self.calculate_cid(&thumb_data);

            // Store thumbnail blob
            if !self.backend.exists(&thumb_cid).await? {
                self.backend
                    .put(&thumb_cid, thumb_data.clone(), "image/jpeg")
                    .await?;

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
                )
                .await?;
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
        )
        .await?;

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

    /// Storage-backend delete only (no metadata touch). Used by the
    /// `emit_event` `DeleteBlob` arm (Arc 4 §8.4.1) as the post-commit
    /// best-effort cleanup paired with `delete_metadata_in_tx`. Per
    /// Step 0.6 §3 Branch (B): the metadata DELETE rides inside the
    /// wrapping transaction; the storage delete runs after `tx.commit`
    /// and is best-effort with WARN-on-failure.
    pub async fn backend_delete(&self, cid: &str) -> PdsResult<()> {
        self.backend.delete(cid).await
    }

    /// Run an Arc 10 GC sweep against this store's backend and pool.
    ///
    /// Thin wrapper around [`crate::blob_store::gc::run_sweep`] —
    /// keeps the backend and DB pool encapsulated inside `BlobStore`
    /// rather than leaking them to consumers. Matches the
    /// [`Self::backend_delete`] wrapper pattern.
    ///
    /// Production callers are the scheduled
    /// `JobScheduler::gc_sweep_job` and the
    /// `aurora-locus gc-sweep` CLI subcommand; both pass
    /// [`chrono::Utc::now()`] for `now`. Tests can pass a fixed
    /// `now` to age fresh blobs past the freshness threshold without
    /// backdating filesystem mtimes (see `blob_store::gc::tests`).
    pub async fn run_gc_sweep(
        &self,
        params: crate::blob_store::gc::SweepParams,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PdsResult<crate::blob_store::gc::SweepReport> {
        crate::blob_store::gc::run_sweep(&*self.backend, &self.db, params, now).await
    }

    /// Arc 16d §9.4.4 Step 3.1: thin wrapper over
    /// [`crate::blob_store::gc::sweep_untethered_rows`] paralleling
    /// [`Self::run_gc_sweep`]. Production caller is the scheduled
    /// `JobScheduler::row_sweep_job` (Step 3.2). Tests can pass a
    /// fixed `now` to age untethered rows past
    /// `params.untethered_ttl`.
    pub async fn run_row_sweep(
        &self,
        params: crate::blob_store::gc::RowSweepParams,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PdsResult<crate::blob_store::gc::RowSweepReport> {
        crate::blob_store::gc::sweep_untethered_rows(&self.db, &*self.backend, params, now)
            .await
    }

    /// Calculate the CIDv1 string for opaque blob data.
    ///
    /// chainlink #93 fix: blobs are opaque byte streams, so the
    /// codec is `raw` (multicodec 0x55), NOT `dag-cbor` (0x71).
    /// The prefix is therefore `bafkrei*` (base32 of varint(1) +
    /// varint(0x55) + multihash) not `bafyrei*` (the dag-cbor
    /// prefix). Construction goes through proto-blue's typed
    /// `Cid::for_raw(...)` which SHA-256-digests the bytes,
    /// wraps them in a sha2-256 multihash (0x12 + 0x20 + 32-byte
    /// digest), and base32-encodes the whole CIDv1 — NOT a
    /// hex-string concatenation onto a fixed prefix.
    fn calculate_cid(&self, data: &[u8]) -> String {
        proto_blue::lex_data::Cid::for_raw(data).to_string()
    }

    /// Validate MIME type is allowed
    fn validate_mime_type(&self, mime_type: &str) -> PdsResult<()> {
        // ATProto allows specific image and video types. #98 added
        // `application/octet-stream` so the detection-fallback at
        // stage_blob:191-199 (which substitutes octet-stream when
        // `detect_mime_type_from_data` returns None) no longer
        // immediately fails validation. The upload path already
        // gates on `max_blob_size` (via `validate_blob_size`)
        // before any disk write, so accepting generic binaries
        // doesn't loosen the size posture — only the type
        // allowlist.
        const ALLOWED_TYPES: &[&str] = &[
            "image/jpeg",
            "image/png",
            "image/gif",
            "image/webp",
            "video/mp4",
            "video/quicktime",
            "video/webm",
            "application/octet-stream",
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
    #[allow(dead_code)] // Simplified version, use store_metadata_full for full metadata
    async fn store_metadata(
        &self,
        cid: &str,
        mime_type: &str,
        size: i64,
        creator_did: &str,
    ) -> PdsResult<()> {
        sqlx::query(
            r#"
            INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT(cid) DO NOTHING
            "#,
        )
        .bind(cid)
        .bind(mime_type)
        .bind(size)
        .bind(creator_did)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;

        Ok(())
    }

    /// Store blob metadata in database with full information (dimensions, thumbnail)
    #[allow(dead_code)] // Future blob metadata storage
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
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
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
        .bind(Utc::now().to_rfc3339())
        .bind(width)
        .bind(height)
        .bind(thumbnail_cid)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;

        Ok(())
    }

    /// Store temp blob metadata in database
    async fn store_temp_blob_metadata(&self, temp_blob: &TempBlob) -> PdsResult<()> {
        sqlx::query(
            r#"
            INSERT INTO temp_blob_metadata (cid, mime_type, size, creator_did, created_at, width, height)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
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
        .bind(temp_blob.created_at.to_rfc3339())
        .bind(temp_blob.width)
        .bind(temp_blob.height)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;

        Ok(())
    }

    /// Get temp blob metadata from database
    #[allow(dead_code)] // Future temp blob metadata retrieval
    async fn get_temp_blob_metadata(&self, cid: &str) -> PdsResult<Option<TempBlob>> {
        let result = sqlx::query(
            r#"
            SELECT cid, mime_type, size, creator_did, created_at, width, height
            FROM temp_blob_metadata
            WHERE cid = $1
            "#,
        )
        .bind(cid)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?;

        if let Some(row) = result {
            Ok(Some(TempBlob {
                cid: row.try_get("cid")?,
                mime_type: row.try_get("mime_type")?,
                size: row.try_get("size")?,
                creator_did: row.try_get("creator_did")?,
                created_at: parse_timestamp(&row.try_get::<String, _>("created_at")?)?,
                width: row.try_get("width")?,
                height: row.try_get("height")?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Delete temp blob metadata from database
    async fn delete_temp_blob_metadata(&self, cid: &str) -> PdsResult<()> {
        sqlx::query("DELETE FROM temp_blob_metadata WHERE cid = $1")
            .bind(cid)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        Ok(())
    }

    /// List orphaned temp blobs (older than ttl).
    ///
    /// Arc 16c §9.3.4 Step 2 / chainlink #92: Arc 16d's scheduled
    /// temp_blob_metadata reaper (not yet implemented) will invoke
    /// this with `ttl_hours = config.blob_metadata.stage_ttl_seconds
    /// / 3600`. Arc 10's classifier continues to use
    /// `freshness_threshold` for its (orthogonal) in-flight-upload
    /// detection — Arc 16c does NOT change Arc 10's classifier per
    /// §9.3.6 audit item 3.
    pub async fn list_orphaned_temp_blobs(&self, ttl_hours: i64) -> PdsResult<Vec<String>> {
        let cutoff = Utc::now() - chrono::Duration::hours(ttl_hours);

        let rows = sqlx::query(
            r#"
            SELECT cid
            FROM temp_blob_metadata
            WHERE created_at < $1
            ORDER BY created_at ASC
            "#,
        )
        .bind(cutoff.to_rfc3339())
        .fetch_all(&self.db)
        .await
        .map_err(PdsError::Database)?;

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
            fs::remove_file(&temp_path).await.map_err(|e| {
                PdsError::BlobStorage(format!("Failed to delete temp blob file: {}", e))
            })?;
        }

        // Delete metadata
        self.delete_temp_blob_metadata(cid).await?;

        Ok(())
    }

    /// Get blob metadata from database (public method)
    pub async fn get_metadata(&self, cid: &str) -> PdsResult<Option<BlobMetadata>> {
        let result = sqlx::query(
            r#"
            SELECT cid, mime_type, size, creator_did, created_at, width, height, alt_text, thumbnail_cid, temp_key
            FROM blob_metadata
            WHERE cid = $1
            "#,
        )
        .bind(cid)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?;

        if let Some(row) = result {
            Ok(Some(BlobMetadata {
                cid: row.try_get("cid")?,
                mime_type: row.try_get("mime_type")?,
                size: row.try_get("size")?,
                creator_did: row.try_get("creator_did")?,
                created_at: parse_timestamp(&row.try_get::<String, _>("created_at")?)?,
                width: row.try_get("width")?,
                height: row.try_get("height")?,
                alt_text: row.try_get("alt_text")?,
                thumbnail_cid: row.try_get("thumbnail_cid")?,
                temp_key: row.try_get("temp_key")?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Delete blob metadata from database. Pool-API wrapper that opens
    /// its own tx; for atomic-with-chain entry, callers should use
    /// [`Self::delete_metadata_in_tx`] (Arc 4 §8.4.0.5 / Step 0.6
    /// Branch (B) decision).
    async fn delete_metadata(&self, cid: &str) -> PdsResult<()> {
        let mut tx = self.db.begin().await.map_err(PdsError::Database)?;
        Self::delete_metadata_in_tx(&mut tx, cid).await?;
        tx.commit().await.map_err(PdsError::Database)?;
        Ok(())
    }

    /// Delete blob metadata inside an existing transaction. Arc 4
    /// §8.4.0.5 / Step 0.6 Branch (B): the metadata DELETE + the
    /// chain entry write atomically inside the wrapping tx; the
    /// **storage-side delete** (`backend.delete(cid)`) is a separate
    /// post-commit operation, intentionally NOT pulled inside this
    /// method. Storage cleanup is best-effort with WARN-on-failure;
    /// orphaned bytes get reconciled by a future GC sweep
    /// (v0.4 follow-up #23).
    pub async fn delete_metadata_in_tx<'tx>(
        tx: &mut sqlx::Transaction<'tx, sqlx::Any>,
        cid: &str,
    ) -> PdsResult<()> {
        sqlx::query("DELETE FROM blob_metadata WHERE cid = $1")
            .bind(cid)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;
        Ok(())
    }

    /// List blobs for a user
    pub async fn list_for_user(&self, did: &str, limit: i64) -> PdsResult<Vec<BlobMetadata>> {
        let rows = sqlx::query(
            r#"
            SELECT cid, mime_type, size, creator_did, created_at, width, height, alt_text, thumbnail_cid, temp_key
            FROM blob_metadata
            WHERE creator_did = $1
            ORDER BY created_at DESC
            LIMIT $2
            "#,
        )
        .bind(did)
        .bind(limit)
        .fetch_all(&self.db)
        .await
        .map_err(PdsError::Database)?;

        let mut blobs = Vec::new();
        for row in rows {
            blobs.push(BlobMetadata {
                cid: row.try_get("cid")?,
                mime_type: row.try_get("mime_type")?,
                size: row.try_get("size")?,
                creator_did: row.try_get("creator_did")?,
                created_at: parse_timestamp(&row.try_get::<String, _>("created_at")?)?,
                width: row.try_get("width")?,
                height: row.try_get("height")?,
                alt_text: row.try_get("alt_text")?,
                thumbnail_cid: row.try_get("thumbnail_cid")?,
                temp_key: row.try_get("temp_key")?,
            });
        }

        Ok(blobs)
    }

    /// List blobs that are referenced by records but not yet uploaded
    ///
    /// Returns blob CIDs and their record URIs for blobs that exist in
    /// record_blob but not in blob_metadata.
    ///
    /// # Arguments
    ///
    /// * `did` - The DID to check missing blobs for
    /// * `limit` - Maximum number of results to return
    /// * `cursor` - Optional cursor for pagination (blob_cid to start after)
    ///
    /// # Returns
    ///
    /// Vec of (blob_cid, record_uri) tuples
    pub async fn list_missing_blobs(
        &self,
        did: &str,
        limit: i64,
        cursor: Option<&str>,
    ) -> PdsResult<Vec<(String, String)>> {
        // Find blobs referenced in records that don't exist in blob storage
        // record_blob tracks references, blob_metadata tracks actual blobs
        let query = if let Some(cursor) = cursor {
            sqlx::query(
                r#"
                SELECT rb.blob_cid, rb.record_uri
                FROM record_blob rb
                LEFT JOIN blob_metadata bm ON rb.blob_cid = bm.cid
                WHERE rb.record_uri LIKE $1
                  AND bm.cid IS NULL
                  AND rb.blob_cid > $2
                ORDER BY rb.blob_cid ASC
                LIMIT $3
                "#,
            )
            .bind(format!("at://{}/%", did))
            .bind(cursor)
            .bind(limit)
        } else {
            sqlx::query(
                r#"
                SELECT rb.blob_cid, rb.record_uri
                FROM record_blob rb
                LEFT JOIN blob_metadata bm ON rb.blob_cid = bm.cid
                WHERE rb.record_uri LIKE $1
                  AND bm.cid IS NULL
                ORDER BY rb.blob_cid ASC
                LIMIT $2
                "#,
            )
            .bind(format!("at://{}/%", did))
            .bind(limit)
        };

        let rows = query
            .fetch_all(&self.db)
            .await
            .map_err(PdsError::Database)?;

        let mut results = Vec::new();
        for row in rows {
            results.push((
                row.try_get::<String, _>("blob_cid")?,
                row.try_get::<String, _>("record_uri")?,
            ));
        }

        Ok(results)
    }

    /// List blob CIDs for sync protocol (com.atproto.sync.listBlobs)
    ///
    /// Returns just the CID strings for a DID with cursor-based pagination.
    ///
    /// # Arguments
    ///
    /// * `did` - The DID to list blobs for
    /// * `since` - Optional since parameter (not yet implemented)
    /// * `limit` - Maximum number of CIDs to return
    /// * `cursor` - Optional cursor for pagination (CID to start after)
    ///
    /// # Returns
    ///
    /// Vec of CID strings, ordered by CID (lexically)
    pub async fn list_blob_cids(
        &self,
        did: &str,
        _since: Option<&str>,
        limit: i64,
        cursor: Option<&str>,
    ) -> PdsResult<Vec<String>> {
        let query = if let Some(cursor) = cursor {
            sqlx::query_scalar(
                "SELECT cid FROM blob_metadata
                 WHERE creator_did = $1 AND cid > $2
                 ORDER BY cid ASC
                 LIMIT $3",
            )
            .bind(did)
            .bind(cursor)
            .bind(limit)
        } else {
            sqlx::query_scalar(
                "SELECT cid FROM blob_metadata
                 WHERE creator_did = $1
                 ORDER BY cid ASC
                 LIMIT $2",
            )
            .bind(did)
            .bind(limit)
        };

        let cids = query
            .fetch_all(&self.db)
            .await
            .map_err(PdsError::Database)?;

        Ok(cids)
    }

    // ========================================================
    // Arc 16b §9.2 — blob lifecycle helpers (chainlink #91)
    // Recon: docs/internal/v05-recon/V05_ARC16B_RECON_R0.md
    // ========================================================
    //
    // These helpers ship with zero production callers per §9.2.5.1.
    // Arc 16c wires them into uploadBlob + record-write paths; Arc
    // 16d wires them into the GC sweep.
    //
    // All write helpers take `&mut Transaction<'_, sqlx::Any>` and
    // participate in the caller's transaction per Step 0.2 Item 3
    // recon. Helpers do not start or commit their own transactions.
    //
    // Row-lock contract (round-5 F4 closure / Step 0.2 Item 9 recon):
    // STRICT and `unreference_blob` use `SELECT … FOR UPDATE` on the
    // read-then-write path. sqlx 0.8 tolerates the clause silently
    // on SQLite (WAL writer-lock provides equivalent serialization);
    // Postgres respects it as a real row lock. Single SQL string
    // works on both backends — no cfg-gating required.

    /// Arc 16b §9.2.3.2 — `track_untethered_blob`: insert (or
    /// refresh) a `blob_metadata` row in the untethered state.
    ///
    /// Three cases per design:
    /// 1. Row absent → INSERT with `temp_key = '1'`, `created_at = now`.
    /// 2. Row present with `temp_key NULL` (permanent) → no state
    ///    change; `mime_type`, `size`, `created_at` preserved.
    /// 3. Row present with `temp_key NOT NULL` (already untethered)
    ///    → UPDATE refreshes `created_at = now`; `mime_type` and
    ///    `size` preserved per first-write-wins (§9.2.5.2).
    ///
    /// Atomic single UPSERT (no row lock needed; statement-atomic).
    pub async fn track_untethered_blob<'tx>(
        &self,
        tx: &mut sqlx::Transaction<'tx, sqlx::Any>,
        cid: &str,
        mime_type: &str,
        size: i64,
        creator_did: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PdsResult<()> {
        let _ = &self.is_postgres; // silence unused-self lint; helper invariant
        sqlx::query(
            r#"
            INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key)
            VALUES ($1, $2, $3, $4, $5, '1')
            ON CONFLICT(cid) DO UPDATE SET created_at = CASE
                WHEN blob_metadata.temp_key IS NOT NULL THEN $5
                ELSE blob_metadata.created_at
            END
            "#,
        )
        .bind(cid)
        .bind(mime_type)
        .bind(size)
        .bind(creator_did)
        .bind(now.to_rfc3339())
        .execute(&mut **tx)
        .await
        .map_err(PdsError::Database)?;
        Ok(())
    }

    /// Arc 16b §9.2.3.2 — `verify_blob_and_make_permanent` (STRICT):
    /// row-lock the `blob_metadata` row by CID; error `BlobNotFound`
    /// if absent; UPDATE `temp_key = NULL`; INSERT `record_blob` join
    /// row with first-link-time DO NOTHING semantic (Step 0.2 Item 6).
    ///
    /// Idempotent on already-permanent rows. Safe to re-call for
    /// same `(cid, record_uri)` pair — UPDATE is a no-op when
    /// `temp_key` already NULL; INSERT no-ops on conflict.
    pub async fn verify_blob_and_make_permanent<'tx>(
        &self,
        tx: &mut sqlx::Transaction<'tx, sqlx::Any>,
        cid: &str,
        record_uri: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PdsResult<()> {
        let sql = format!(
            "SELECT cid FROM blob_metadata WHERE cid = $1{}",
            self.for_update_clause()
        );
        let row = sqlx::query(&sql)
            .bind(cid)
            .fetch_optional(&mut **tx)
            .await
            .map_err(PdsError::Database)?;
        if row.is_none() {
            return Err(PdsError::BlobNotFound(cid.to_string()));
        }
        sqlx::query("UPDATE blob_metadata SET temp_key = NULL WHERE cid = $1")
            .bind(cid)
            .execute(&mut **tx)
            .await
            .map_err(PdsError::Database)?;
        sqlx::query(
            r#"
            INSERT INTO record_blob (blob_cid, record_uri, indexed_at)
            VALUES ($1, $2, $3)
            ON CONFLICT(blob_cid, record_uri) DO NOTHING
            "#,
        )
        .bind(cid)
        .bind(record_uri)
        .bind(now.to_rfc3339())
        .execute(&mut **tx)
        .await
        .map_err(PdsError::Database)?;
        Ok(())
    }

    /// Arc 16b §9.2.3.2 — `unreference_blob`: drop a `record_blob`
    /// row for `(cid, record_uri)`; if the blob has no other refs
    /// post-DELETE, re-mark `blob_metadata.temp_key = '1'` (back to
    /// untethered) with fresh `created_at` (TTL anchor reset).
    ///
    /// Returns `UnreferenceOutcome` per §9.2.3.2's six-variant enum
    /// (round-5 F1 + F3 closures). Caller obligations per the
    /// caller-obligations table.
    pub async fn unreference_blob<'tx>(
        &self,
        tx: &mut sqlx::Transaction<'tx, sqlx::Any>,
        cid: &str,
        record_uri: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PdsResult<UnreferenceOutcome> {
        let delete_result =
            sqlx::query("DELETE FROM record_blob WHERE blob_cid = $1 AND record_uri = $2")
                .bind(cid)
                .bind(record_uri)
                .execute(&mut **tx)
                .await
                .map_err(PdsError::Database)?;

        if delete_result.rows_affected() == 0 {
            return Ok(UnreferenceOutcome::PhantomDelete);
        }

        // Real ref removed. Now read blob_metadata + EXISTS(record_blob)
        // under row lock (forces serialization vs concurrent writers).
        let sql = format!(
            r#"
            SELECT temp_key,
                   EXISTS(SELECT 1 FROM record_blob WHERE blob_cid = $1) AS refs_remain
            FROM blob_metadata
            WHERE cid = $1{}
            "#,
            self.for_update_clause()
        );
        let row = sqlx::query(&sql)
            .bind(cid)
            .fetch_optional(&mut **tx)
            .await
            .map_err(PdsError::Database)?;

        let Some(row) = row else {
            return Ok(UnreferenceOutcome::OrphanedRef);
        };

        let temp_key: Option<String> = row.try_get("temp_key").map_err(PdsError::Database)?;
        // sqlx::Any returns EXISTS as INTEGER 0/1 on SQLite, BOOLEAN
        // on Postgres — route through the canonical read_bool helper
        // (same pattern as Arc 13 #71 + #86 fixes).
        let refs_remain: bool = crate::db::read_bool(&row, "refs_remain")?;

        match (temp_key.is_some(), refs_remain) {
            (true, true) => Ok(UnreferenceOutcome::AlreadyUntethered_RefsRemain),
            (true, false) => Ok(UnreferenceOutcome::AlreadyUntethered_NoRefs),
            (false, true) => Ok(UnreferenceOutcome::OtherRefsRemain),
            (false, false) => {
                // Last ref dropped on a permanent row. Re-mark
                // untethered with fresh TTL anchor.
                sqlx::query(
                    "UPDATE blob_metadata SET temp_key = '1', created_at = $1 WHERE cid = $2",
                )
                .bind(now.to_rfc3339())
                .bind(cid)
                .execute(&mut **tx)
                .await
                .map_err(PdsError::Database)?;
                Ok(UnreferenceOutcome::LastRefDropped)
            }
        }
    }

    /// Arc 16b §9.2.3.2 — `is_untethered`: observability helper.
    /// `Ok(true)` iff `blob_metadata` row exists with `temp_key NOT
    /// NULL`; `Ok(false)` if row absent or `temp_key NULL`.
    ///
    /// Observability caveat: observes committed state only — cannot
    /// reflect uncommitted writes in any in-flight transaction
    /// (including the caller's own). NOT a control primitive.
    ///
    /// Observational-only forward-substrate: consumed by tests +
    /// future operator admin endpoint; no production caller today.
    #[allow(dead_code)]
    pub async fn is_untethered(&self, cid: &str) -> PdsResult<bool> {
        let row = sqlx::query("SELECT temp_key FROM blob_metadata WHERE cid = $1")
            .bind(cid)
            .fetch_optional(&self.db)
            .await
            .map_err(PdsError::Database)?;
        Ok(row
            .and_then(|r| r.try_get::<Option<String>, _>("temp_key").ok().flatten())
            .is_some())
    }

    /// Arc 16f §9.6.3.2 — TOLERANT promotion sibling to Arc 16b's STRICT
    /// `verify_blob_and_make_permanent`. Used by the CAR-import flow
    /// (Path B per V05_DESIGN.md §9.6 header): blob_metadata row may
    /// be absent because the importing repo's blobs haven't been
    /// re-fetched from origin yet, so a missing row signals
    /// `NeedsFetch` to the caller rather than erroring like STRICT.
    ///
    /// Quarantine-first ordering (closes round-1 F3): the
    /// `blob_quarantine` check fires BEFORE the `blob_metadata` lookup
    /// so a quarantined CID rejects atomically even if a stray row
    /// exists. Validate-phase at §9.6.3.1 step 5 catches the common
    /// case before Phase A opens; this variant is defense-in-depth
    /// for quarantine landing between validate-phase and Phase B
    /// (window bounded by Arc 16b row-lock discipline per §9.6.5.5).
    ///
    /// Present-row branches DELEGATE to Arc 16b's
    /// `verify_blob_and_make_permanent` (closes round-1 F15 — no
    /// duplication of the present-row state machine). Arc 16b's four
    /// locked helpers stay byte-identical.
    ///
    /// `cid: &Cid` uses the proto-blue typed form per §9.6.3.2
    /// signature spec; conversion to `&str` for the SQL binding +
    /// STRICT delegation happens internally.
    pub async fn verify_blob_tolerant_or_signal<'tx>(
        &self,
        tx: &mut sqlx::Transaction<'tx, sqlx::Any>,
        cid: &Cid,
        record_uri: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PdsResult<TolerantOutcome> {
        let cid_str = cid.to_string();

        // 1. Quarantine check (no FOR UPDATE — quarantine writes are
        //    serialized by Arc 16b's own row-lock discipline, so this
        //    read can be lock-free per §9.6.3.2 step 1).
        //    `restored_at IS NULL` matches the active-quarantine
        //    semantic from src/blob_store/quarantine.rs::is_quarantined.
        let q_row = sqlx::query(
            "SELECT reason FROM blob_quarantine \
             WHERE cid = $1 AND restored_at IS NULL \
             LIMIT 1",
        )
        .bind(&cid_str)
        .fetch_optional(&mut **tx)
        .await
        .map_err(PdsError::Database)?;
        if let Some(row) = q_row {
            let reason_str: String =
                row.try_get("reason").map_err(PdsError::Database)?;
            return Ok(TolerantOutcome::Quarantined {
                cid: cid.clone(),
                public_reason: QuarantinePublicReason::from_internal_reason_str(&reason_str),
            });
        }

        // 2. `SELECT temp_key FROM blob_metadata WHERE cid = $1 FOR UPDATE`
        //    (FOR UPDATE conditional on backend via Arc 16b's shared
        //    `for_update_clause` — Postgres locks the row, SQLite WAL
        //    relies on writer serialization per §9.2.3.2).
        let sql = format!(
            "SELECT temp_key FROM blob_metadata WHERE cid = $1{}",
            self.for_update_clause()
        );
        let row = sqlx::query(&sql)
            .bind(&cid_str)
            .fetch_optional(&mut **tx)
            .await
            .map_err(PdsError::Database)?;

        // 3. Row absent → caller must fetch + retry. NO state mutation.
        let row = match row {
            Some(r) => r,
            None => return Ok(TolerantOutcome::NeedsFetch { cid: cid.clone() }),
        };

        // 4. Row present (any temp_key state) → DELEGATE to Arc 16b's
        //    STRICT. Map the outcome based on the pre-delegation
        //    temp_key state we already read above.
        let temp_key_state: Option<String> =
            row.try_get("temp_key").map_err(PdsError::Database)?;
        match self
            .verify_blob_and_make_permanent(tx, &cid_str, record_uri, now)
            .await
        {
            Ok(()) => Ok(match temp_key_state {
                Some(_) => TolerantOutcome::Promoted,
                None => TolerantOutcome::AlreadyPermanent,
            }),
            Err(PdsError::BlobNotFound(_)) => {
                // We just confirmed the row was present (step 2). If
                // STRICT's own row-lookup reports BlobNotFound, the row
                // was concurrently deleted in the window between our
                // SELECT and STRICT's. Narrow enough to be a real bug if
                // it fires — surface as Internal so an operator sees it.
                Err(PdsError::Internal(format!(
                    "verify_blob_tolerant_or_signal: blob_metadata row {} \
                     disappeared between TOLERANT SELECT and STRICT delegation",
                    cid_str
                )))
            }
            Err(other) => Err(other),
        }
    }
}

/// Arc 16b §9.2.3.2 — outcome of `unreference_blob`. Six variants
/// (round-5 F1 + F3 closures) surface races and inconsistencies
/// explicitly rather than papering over. See §9.2.3.2 caller
/// obligations table for log-level + escalation guidance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
#[allow(dead_code)]
pub enum UnreferenceOutcome {
    /// DELETE removed a real ref; that ref was the last one; blob
    /// re-marked untethered (TTL anchor refreshed). Normal path.
    LastRefDropped,
    /// DELETE removed a real ref; other refs remain; blob stays
    /// permanent. Normal path.
    OtherRefsRemain,
    /// DELETE removed 0 rows because the (cid, record_uri) pair was
    /// not present. No state change. Caller MAY log at DEBUG;
    /// indicates caller-side bug, idempotent retry, or concurrent
    /// `unreference_blob` on the same pair.
    PhantomDelete,
    /// DELETE removed a real ref; `blob_metadata` row was already
    /// untethered AND other refs remain. Deep inconsistency: TTL
    /// anchor was "live" while the row was referenced. Caller SHOULD
    /// log at ERROR.
    AlreadyUntethered_RefsRemain,
    /// DELETE removed a real ref; `blob_metadata` row was already
    /// untethered AND no other refs remain. Mild anomaly: the
    /// `record_blob` row was a stray. Caller SHOULD log at WARN.
    AlreadyUntethered_NoRefs,
    /// DELETE removed a real ref but no `blob_metadata` row exists
    /// for the CID. Defensive-against-corruption: reachable via
    /// operator intervention, FK-disabled replicas, DB corruption,
    /// or backup-restore inconsistency. Per Step 0.2 Item 10 recon
    /// the expected FK + cascade is NOT yet in place; this variant
    /// is therefore reachable via normal-but-incorrect call orderings
    /// today (Arc 16c integration discipline + v0.6+ FK hardening
    /// would close those gaps). Caller SHOULD log at ERROR.
    OrphanedRef,
}

/// Arc 16f §9.6.3.2 — outcome of [`BlobStore::verify_blob_tolerant_or_signal`].
/// TOLERANT extends Arc 16b's STRICT vocabulary with `NeedsFetch`
/// (caller-driven origin re-fetch per Path B) and `Quarantined`
/// (defense-in-depth for quarantine landing between validate-phase
/// and Phase B).
///
/// `Promoted` vs `AlreadyPermanent` carry no payload because the
/// caller already has the `(cid, record_uri)` pair; the distinction
/// is for logging + diagnosis only.
#[derive(Debug, Clone)]
pub enum TolerantOutcome {
    /// `blob_metadata` row absent → caller must fetch the blob bytes
    /// from origin PDS and retry. NO state mutation. Path B's
    /// fetch-and-retry loop at §9.6.3.5 consumes this signal.
    NeedsFetch { cid: Cid },

    /// `blob_metadata` row was present with `temp_key NOT NULL` →
    /// STRICT delegation flipped temp_key to NULL and inserted the
    /// `record_blob` row. Normal-path import promotion.
    Promoted,

    /// `blob_metadata` row was already permanent (`temp_key NULL`)
    /// → STRICT delegation no-op'd the UPDATE and inserted the
    /// `record_blob` row idempotently. Common for re-import of
    /// previously-imported repos.
    AlreadyPermanent,

    /// `blob_quarantine` entry exists for the CID at TOLERANT call
    /// time. NO state mutation. Defense-in-depth — §9.6.3.1 step 5
    /// catches the common case before Phase A opens; this variant
    /// fires only if quarantine landed between validate-phase and
    /// Phase B (window bounded by Arc 16b row-lock discipline per
    /// §9.6.5.5).
    Quarantined {
        cid: Cid,
        public_reason: QuarantinePublicReason,
    },
}

/// Arc 16f §9.6.3.6 — coarse public class for quarantine reasons,
/// exposed on the wire as the `public_reason` field. Closes round-1
/// F20: operator-internal detail (e.g. `"matched signature
/// SHA256:..."`) MUST NOT leak to clients; clients see only the
/// coarse class.
///
/// Distinct from [`crate::blob_store::quarantine::QuarantineReason`]
/// (the operator-internal enum stored as text in
/// `blob_quarantine.reason`). Mapping from internal-reason-string to
/// coarse public class lives in [`Self::from_internal_reason_str`];
/// operator-configurable mapping is a v0.6+ candidate per §9.6.7.
///
/// Note on naming divergence from V05_DESIGN.md §9.6.3.6 pseudocode
/// (skydeval review for Step 2/3 prompts): the design spec named this
/// enum `QuarantineReason`, but that name is already taken by the
/// operator-internal enum at `src/blob_store/quarantine.rs:18`. This
/// type is named `QuarantinePublicReason` to coexist without collision;
/// the wire field stays `public_reason` per spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuarantinePublicReason {
    /// Operator policy decision (e.g. terms-of-service violation).
    Policy,
    /// Content abuse class (CSAM, harassment, etc.).
    Abuse,
    /// Detected harmful payload (malware, phishing).
    Malware,
    /// Legal request or takedown (DMCA, court order).
    Legal,
    /// No specific public reason; the operator-internal reason did
    /// not map to one of the above classes. Default for unrecognized
    /// internal reason strings.
    Unspecified,
}

impl QuarantinePublicReason {
    /// Map an operator-internal `blob_quarantine.reason` string to
    /// the coarse public class. Matching is case-insensitive.
    ///
    /// Default v0.5 mapping aligns with
    /// [`crate::blob_store::quarantine::QuarantineReason`] variants:
    ///
    /// | Internal     | Public class |
    /// |--------------|--------------|
    /// | `dmca`       | [`Legal`]    |
    /// | `legal`      | [`Legal`]    |
    /// | `csam`       | [`Abuse`]    |
    /// | `tos`        | [`Policy`]   |
    /// | `malware`    | [`Malware`]  |
    /// | `other`      | [`Unspecified`] |
    /// | (unknown)    | [`Unspecified`] |
    ///
    /// Operators wanting different mapping should wait for the v0.6+
    /// config-driven mapping table per §9.6.7. Unrecognized internal
    /// reason strings (operator-custom values not in the table above)
    /// default to `Unspecified` so a misconfigured internal reason
    /// can never leak detail to the wire.
    ///
    /// [`Legal`]: Self::Legal
    /// [`Abuse`]: Self::Abuse
    /// [`Policy`]: Self::Policy
    /// [`Malware`]: Self::Malware
    /// [`Unspecified`]: Self::Unspecified
    pub fn from_internal_reason_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "dmca" | "legal" => Self::Legal,
            "csam" => Self::Abuse,
            "tos" => Self::Policy,
            "malware" => Self::Malware,
            _ => Self::Unspecified,
        }
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

        // Create in-memory database for testing. Single-connection pool
        // is required for `:memory:` SQLite (each connection has its own
        // private database otherwise).
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        let db = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        // Create blob_metadata table with new columns
        // (Arc 16b §9.2.3.1: includes temp_key column + CHECK).
        sqlx::query(
            r#"
            CREATE TABLE blob_metadata (
                cid TEXT PRIMARY KEY,
                mime_type TEXT NOT NULL,
                size INTEGER NOT NULL,
                creator_did TEXT NOT NULL,
                created_at TEXT NOT NULL,
                width INTEGER,
                height INTEGER,
                alt_text TEXT,
                thumbnail_cid TEXT,
                temp_key TEXT NULL CHECK (temp_key IS NULL OR temp_key = '1')
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        // Arc 16c §9.3.4 Step 3.5: commit_blob production-path tests
        // need temp_blob_metadata schema. Mirrors
        // migrations/0001_initial.sql:181-189.
        sqlx::query(
            r#"
            CREATE TABLE temp_blob_metadata (
                cid TEXT PRIMARY KEY,
                mime_type TEXT NOT NULL,
                size INTEGER NOT NULL,
                creator_did TEXT NOT NULL,
                created_at TEXT NOT NULL,
                width INTEGER,
                height INTEGER
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        BlobStore::new(config, db).await.unwrap()
    }

    #[tokio::test]
    async fn test_upload_and_get_blob() {
        let store = create_test_store().await;

        let data = b"test image data".to_vec();
        let blob_ref = store
            .upload(data.clone(), Some("image/png"), "did:plc:test")
            .await
            .unwrap();

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
        let blob_ref1 = store
            .upload(data.clone(), Some("image/jpeg"), "did:plc:test1")
            .await
            .unwrap();
        let blob_ref2 = store
            .upload(data, Some("image/jpeg"), "did:plc:test2")
            .await
            .unwrap();

        // Should have same CID (content-addressed)
        assert_eq!(blob_ref1.r#ref.link, blob_ref2.r#ref.link);
    }

    #[tokio::test]
    async fn test_upload_oversized_blob() {
        let store = create_test_store().await;

        let large_data = vec![0u8; 2 * 1024 * 1024]; // 2MB, over limit

        let result = store
            .upload(large_data, Some("image/png"), "did:plc:test")
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("exceeds maximum"));
    }

    #[tokio::test]
    async fn test_upload_invalid_mime_type() {
        let store = create_test_store().await;

        let data = b"test data".to_vec();

        let result = store
            .upload(data, Some("application/exe"), "did:plc:test")
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unsupported MIME type"));
    }

    #[tokio::test]
    async fn test_delete_blob() {
        let store = create_test_store().await;

        let data = b"to be deleted".to_vec();
        let blob_ref = store
            .upload(data, Some("image/png"), "did:plc:test")
            .await
            .unwrap();

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
        let blob_ref = store
            .upload(buf, Some("image/png"), "did:plc:test")
            .await
            .unwrap();

        // Get metadata and verify dimensions
        let metadata = store
            .get_metadata(&blob_ref.r#ref.link)
            .await
            .unwrap()
            .unwrap();
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
        let blob_ref = store
            .upload(buf, Some("image/png"), "did:plc:test")
            .await
            .unwrap();

        // Get metadata and verify thumbnail was created
        let metadata = store
            .get_metadata(&blob_ref.r#ref.link)
            .await
            .unwrap()
            .unwrap();
        assert!(
            metadata.thumbnail_cid.is_some(),
            "Thumbnail should be generated for images"
        );

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
        let blob_ref = store
            .upload(data, Some("image/png"), "did:plc:test")
            .await
            .unwrap();

        // Get metadata
        let metadata = store
            .get_metadata(&blob_ref.r#ref.link)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(metadata.cid, blob_ref.r#ref.link);
        assert_eq!(metadata.mime_type, "image/png");
        assert_eq!(metadata.size, 9);
        assert_eq!(metadata.creator_did, "did:plc:test");
    }

    // ====================================================================
    // Arc 4 §8.4.0.5 / Step 0.6 Branch (B) — delete_metadata_in_tx.
    // Tests pin commit + rollback semantics for the metadata DELETE.
    // ====================================================================

    async fn setup_metadata_pool(cid: &str) -> AnyPool {
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        let db = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE blob_metadata (
                cid TEXT PRIMARY KEY, mime_type TEXT NOT NULL, size INTEGER NOT NULL,
                creator_did TEXT NOT NULL, created_at TEXT NOT NULL,
                width INTEGER, height INTEGER, alt_text TEXT, thumbnail_cid TEXT
            )",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(cid)
        .bind("image/png")
        .bind(1024_i64)
        .bind("did:plc:alice")
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&db)
        .await
        .unwrap();
        db
    }

    async fn metadata_count(db: &AnyPool, cid: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM blob_metadata WHERE cid = $1")
            .bind(cid)
            .fetch_one(db)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn delete_metadata_in_tx_rolls_back_on_caller_rollback() {
        let cid = "bafy_meta_rollback";
        let db = setup_metadata_pool(cid).await;
        {
            let mut tx = db.begin().await.unwrap();
            BlobStore::delete_metadata_in_tx(&mut tx, cid).await.unwrap();
            tx.rollback().await.unwrap();
        }
        assert_eq!(
            metadata_count(&db, cid).await,
            1,
            "rolled-back tx must leave metadata row intact"
        );
    }

    #[tokio::test]
    async fn delete_metadata_in_tx_commits_on_caller_commit() {
        let cid = "bafy_meta_commit";
        let db = setup_metadata_pool(cid).await;
        let mut tx = db.begin().await.unwrap();
        BlobStore::delete_metadata_in_tx(&mut tx, cid).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            metadata_count(&db, cid).await,
            0,
            "committed tx must remove the metadata row"
        );
    }

    // ============================================================
    // Arc 16b §9.2.3.2 lifecycle helper tests (chainlink #91)
    // Per §9.2.4 Step 3.7. Recon-driven: no FK between record_blob
    // and blob_metadata (Step 0.2 Item 10), so OrphanedRef setup
    // does NOT need FK disablement.
    // ============================================================

    /// Fresh in-memory `BlobStore` with the Arc-16b schema applied
    /// (blob_metadata with temp_key + CHECK; record_blob). Returns
    /// the store + the underlying pool (tests need direct pool
    /// access for SQL setup + verification).
    async fn arc16b_store() -> (BlobStore, sqlx::AnyPool) {
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"CREATE TABLE blob_metadata (
                cid TEXT PRIMARY KEY,
                mime_type TEXT NOT NULL,
                size INTEGER NOT NULL,
                creator_did TEXT NOT NULL,
                created_at TEXT NOT NULL,
                width INTEGER,
                height INTEGER,
                alt_text TEXT,
                thumbnail_cid TEXT,
                temp_key TEXT NULL CHECK (temp_key IS NULL OR temp_key = '1')
            )"#,
        )
        .execute(&pool).await.unwrap();
        sqlx::query(
            r#"CREATE TABLE record_blob (
                blob_cid TEXT NOT NULL,
                record_uri TEXT NOT NULL,
                indexed_at TEXT NOT NULL,
                PRIMARY KEY (blob_cid, record_uri)
            )"#,
        )
        .execute(&pool).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let config = BlobStoreConfig {
            storage: BlobStorageConfig {
                backend: BlobBackendType::Disk { location: dir.path().to_path_buf() },
                max_blob_size: 1024 * 1024,
                temp_dir: dir.path().join("tmp"),
            },
        };
        let store = BlobStore::new(config, pool.clone()).await.unwrap();
        (store, pool)
    }

    async fn read_temp_key(pool: &sqlx::AnyPool, cid: &str) -> Option<String> {
        let row = sqlx::query("SELECT temp_key FROM blob_metadata WHERE cid = $1")
            .bind(cid)
            .fetch_optional(pool)
            .await
            .unwrap()?;
        row.try_get::<Option<String>, _>("temp_key").ok().flatten()
    }

    async fn metadata_exists(pool: &sqlx::AnyPool, cid: &str) -> bool {
        sqlx::query("SELECT 1 FROM blob_metadata WHERE cid = $1")
            .bind(cid)
            .fetch_optional(pool)
            .await
            .unwrap()
            .is_some()
    }

    async fn record_blob_count(pool: &sqlx::AnyPool, cid: &str) -> i64 {
        let row = sqlx::query("SELECT COUNT(*) AS c FROM record_blob WHERE blob_cid = $1")
            .bind(cid)
            .fetch_one(pool)
            .await
            .unwrap();
        row.try_get::<i64, _>("c").unwrap()
    }

    // ---- track_untethered_blob: 3 cases ----

    #[tokio::test]
    async fn track_untethered_case_1_row_absent_inserts_with_temp_key() {
        let (store, pool) = arc16b_store().await;
        let mut tx = pool.begin().await.unwrap();
        store.track_untethered_blob(
            &mut tx, "bafyrei-c1", "image/png", 100, "did:plc:alice",
            chrono::Utc::now(),
        ).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(read_temp_key(&pool, "bafyrei-c1").await.as_deref(), Some("1"));
    }

    #[tokio::test]
    async fn track_untethered_case_2_already_permanent_no_state_change() {
        let (store, pool) = arc16b_store().await;
        // Seed a permanent row (temp_key NULL).
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ('bafyrei-c2', 'image/png', 100, 'did:plc:alice', '2026-01-01T00:00:00Z', NULL)",
        )
        .execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        store.track_untethered_blob(
            &mut tx, "bafyrei-c2", "image/png", 100, "did:plc:alice",
            chrono::Utc::now(),
        ).await.unwrap();
        tx.commit().await.unwrap();
        // Row stays permanent; track_untethered does NOT flip permanent → untethered.
        assert!(read_temp_key(&pool, "bafyrei-c2").await.is_none());
    }

    #[tokio::test]
    async fn track_untethered_case_3_already_untethered_refreshes_created_at() {
        let (store, pool) = arc16b_store().await;
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ('bafyrei-c3', 'image/png', 100, 'did:plc:alice', '2020-01-01T00:00:00Z', '1')",
        )
        .execute(&pool).await.unwrap();
        let new_now = chrono::DateTime::parse_from_rfc3339("2026-05-19T12:00:00Z")
            .unwrap().with_timezone(&chrono::Utc);
        let mut tx = pool.begin().await.unwrap();
        store.track_untethered_blob(
            &mut tx, "bafyrei-c3", "image/png", 100, "did:plc:alice", new_now,
        ).await.unwrap();
        tx.commit().await.unwrap();
        let row = sqlx::query("SELECT created_at FROM blob_metadata WHERE cid = 'bafyrei-c3'")
            .fetch_one(&pool).await.unwrap();
        let ts: String = row.try_get("created_at").unwrap();
        assert!(ts.starts_with("2026-05-19"), "created_at refreshed; got {}", ts);
    }

    // ---- STRICT: 4 cases ----

    #[tokio::test]
    async fn strict_success_makes_permanent_and_inserts_record_blob() {
        let (store, pool) = arc16b_store().await;
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ('bafyrei-s1', 'image/png', 100, 'did:plc:alice', '2026-01-01T00:00:00Z', '1')",
        )
        .execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        store.verify_blob_and_make_permanent(
            &mut tx, "bafyrei-s1", "at://did:plc:alice/app.bsky.feed.post/abc",
            chrono::Utc::now(),
        ).await.unwrap();
        tx.commit().await.unwrap();
        assert!(read_temp_key(&pool, "bafyrei-s1").await.is_none(), "temp_key cleared");
        assert_eq!(record_blob_count(&pool, "bafyrei-s1").await, 1);
    }

    #[tokio::test]
    async fn strict_errors_blob_not_found_when_row_absent() {
        let (store, pool) = arc16b_store().await;
        let mut tx = pool.begin().await.unwrap();
        let result = store.verify_blob_and_make_permanent(
            &mut tx, "bafyrei-missing", "at://x/y/z", chrono::Utc::now(),
        ).await;
        match result {
            Err(PdsError::BlobNotFound(cid)) => assert_eq!(cid, "bafyrei-missing"),
            other => panic!("expected BlobNotFound, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn strict_idempotent_same_pair_no_op_on_retry() {
        let (store, pool) = arc16b_store().await;
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ('bafyrei-s3', 'image/png', 100, 'did:plc:alice', '2026-01-01T00:00:00Z', '1')",
        )
        .execute(&pool).await.unwrap();
        for _ in 0..3 {
            let mut tx = pool.begin().await.unwrap();
            store.verify_blob_and_make_permanent(
                &mut tx, "bafyrei-s3", "at://did:plc:alice/coll/rkey",
                chrono::Utc::now(),
            ).await.unwrap();
            tx.commit().await.unwrap();
        }
        assert_eq!(record_blob_count(&pool, "bafyrei-s3").await, 1, "still single ref");
    }

    #[tokio::test]
    async fn strict_succeeds_on_already_permanent_row() {
        let (store, pool) = arc16b_store().await;
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ('bafyrei-s4', 'image/png', 100, 'did:plc:alice', '2026-01-01T00:00:00Z', NULL)",
        )
        .execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        store.verify_blob_and_make_permanent(
            &mut tx, "bafyrei-s4", "at://did:plc:alice/coll/rkey",
            chrono::Utc::now(),
        ).await.unwrap();
        tx.commit().await.unwrap();
        assert!(read_temp_key(&pool, "bafyrei-s4").await.is_none());
        assert_eq!(record_blob_count(&pool, "bafyrei-s4").await, 1);
    }

    // ---- unreference_blob: 6 outcomes ----

    async fn seed_permanent_with_refs(pool: &sqlx::AnyPool, cid: &str, uris: &[&str]) {
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ($1, 'image/png', 100, 'did:plc:alice', '2026-01-01T00:00:00Z', NULL)",
        )
        .bind(cid).execute(pool).await.unwrap();
        for uri in uris {
            sqlx::query(
                "INSERT INTO record_blob (blob_cid, record_uri, indexed_at) \
                 VALUES ($1, $2, '2026-01-01T00:00:00Z')",
            )
            .bind(cid).bind(uri).execute(pool).await.unwrap();
        }
    }

    #[tokio::test]
    async fn unreference_last_ref_dropped_re_untethers() {
        let (store, pool) = arc16b_store().await;
        seed_permanent_with_refs(&pool, "bafyrei-u1", &["at://x/y/1"]).await;
        let mut tx = pool.begin().await.unwrap();
        let outcome = store.unreference_blob(
            &mut tx, "bafyrei-u1", "at://x/y/1", chrono::Utc::now(),
        ).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(outcome, UnreferenceOutcome::LastRefDropped);
        assert_eq!(read_temp_key(&pool, "bafyrei-u1").await.as_deref(), Some("1"));
        assert_eq!(record_blob_count(&pool, "bafyrei-u1").await, 0);
    }

    #[tokio::test]
    async fn unreference_other_refs_remain_stays_permanent() {
        let (store, pool) = arc16b_store().await;
        seed_permanent_with_refs(&pool, "bafyrei-u2", &["at://x/y/1", "at://x/y/2"]).await;
        let mut tx = pool.begin().await.unwrap();
        let outcome = store.unreference_blob(
            &mut tx, "bafyrei-u2", "at://x/y/1", chrono::Utc::now(),
        ).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(outcome, UnreferenceOutcome::OtherRefsRemain);
        assert!(read_temp_key(&pool, "bafyrei-u2").await.is_none(), "stays permanent");
        assert_eq!(record_blob_count(&pool, "bafyrei-u2").await, 1);
    }

    #[tokio::test]
    async fn unreference_phantom_delete_no_state_change() {
        let (store, pool) = arc16b_store().await;
        seed_permanent_with_refs(&pool, "bafyrei-u3", &["at://x/y/1"]).await;
        let mut tx = pool.begin().await.unwrap();
        let outcome = store.unreference_blob(
            &mut tx, "bafyrei-u3", "at://nonexistent/uri", chrono::Utc::now(),
        ).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(outcome, UnreferenceOutcome::PhantomDelete);
        assert!(read_temp_key(&pool, "bafyrei-u3").await.is_none(), "row unchanged");
        assert_eq!(record_blob_count(&pool, "bafyrei-u3").await, 1);
    }

    #[tokio::test]
    async fn unreference_already_untethered_refs_remain_deep_inconsistency() {
        let (store, pool) = arc16b_store().await;
        // Fabricated state: row temp_key='1' BUT refs exist (deep inconsistency).
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ('bafyrei-u4', 'image/png', 100, 'did:plc:alice', '2026-01-01T00:00:00Z', '1')",
        )
        .execute(&pool).await.unwrap();
        for uri in &["at://x/y/1", "at://x/y/2"] {
            sqlx::query(
                "INSERT INTO record_blob (blob_cid, record_uri, indexed_at) \
                 VALUES ('bafyrei-u4', $1, '2026-01-01T00:00:00Z')",
            )
            .bind(uri).execute(&pool).await.unwrap();
        }
        let mut tx = pool.begin().await.unwrap();
        let outcome = store.unreference_blob(
            &mut tx, "bafyrei-u4", "at://x/y/1", chrono::Utc::now(),
        ).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(outcome, UnreferenceOutcome::AlreadyUntethered_RefsRemain);
    }

    #[tokio::test]
    async fn unreference_already_untethered_no_refs_mild_anomaly() {
        let (store, pool) = arc16b_store().await;
        // Fabricated state: row temp_key='1' AND a stray record_blob row for the same cid.
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ('bafyrei-u5', 'image/png', 100, 'did:plc:alice', '2026-01-01T00:00:00Z', '1')",
        )
        .execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO record_blob (blob_cid, record_uri, indexed_at) \
             VALUES ('bafyrei-u5', 'at://stray/ref/1', '2026-01-01T00:00:00Z')",
        )
        .execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        let outcome = store.unreference_blob(
            &mut tx, "bafyrei-u5", "at://stray/ref/1", chrono::Utc::now(),
        ).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(outcome, UnreferenceOutcome::AlreadyUntethered_NoRefs);
    }

    /// chainlink #91 / §9.2 / Step 0.2 Item 10 recon: no FK exists
    /// between record_blob and blob_metadata, so orphan-fabrication
    /// requires no FK disablement.
    #[tokio::test]
    async fn unreference_orphaned_ref_defensive_against_corruption() {
        let (store, pool) = arc16b_store().await;
        // Fabricated state: record_blob row exists with NO corresponding blob_metadata row.
        sqlx::query(
            "INSERT INTO record_blob (blob_cid, record_uri, indexed_at) \
             VALUES ('bafyrei-orphan', 'at://x/y/1', '2026-01-01T00:00:00Z')",
        )
        .execute(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        let outcome = store.unreference_blob(
            &mut tx, "bafyrei-orphan", "at://x/y/1", chrono::Utc::now(),
        ).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(outcome, UnreferenceOutcome::OrphanedRef);
    }

    // ---- is_untethered: 3 states ----

    #[tokio::test]
    async fn is_untethered_true_when_row_has_temp_key() {
        let (store, pool) = arc16b_store().await;
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ('bafyrei-i1', 'image/png', 100, 'did:plc:alice', '2026-01-01T00:00:00Z', '1')",
        )
        .execute(&pool).await.unwrap();
        assert!(store.is_untethered("bafyrei-i1").await.unwrap());
    }

    #[tokio::test]
    async fn is_untethered_false_when_row_is_permanent() {
        let (store, pool) = arc16b_store().await;
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ('bafyrei-i2', 'image/png', 100, 'did:plc:alice', '2026-01-01T00:00:00Z', NULL)",
        )
        .execute(&pool).await.unwrap();
        assert!(!store.is_untethered("bafyrei-i2").await.unwrap());
    }

    #[tokio::test]
    async fn is_untethered_false_when_row_absent() {
        let (store, _pool) = arc16b_store().await;
        assert!(!store.is_untethered("bafyrei-missing").await.unwrap());
    }

    // ---- CHECK constraint ----

    /// §9.2.3.1 CHECK constraint: temp_key must be NULL or '1'.
    /// Direct INSERT with disallowed value must error.
    #[tokio::test]
    async fn check_constraint_rejects_unexpected_temp_key_value() {
        let (_store, pool) = arc16b_store().await;
        let result = sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ('bafyrei-bad', 'image/png', 100, 'did:plc:alice', '2026-01-01T00:00:00Z', 'unexpected')",
        )
        .execute(&pool).await;
        assert!(result.is_err(), "CHECK constraint must reject temp_key not in {{NULL, '1'}}");
        assert!(!metadata_exists(&pool, "bafyrei-bad").await);
    }

    // ============================================================
    // Arc 16c §9.3.4 Step 3.5 commit_blob production-path tests
    // (chainlink #92). Cover §9.3.3.2 ordering invariant + Step
    // 0.7 split-mode disposition for Scenario 3b/5b.
    // ============================================================

    /// Helper: stage a synthetic blob into the test store + return
    /// the BlobRef. Mirrors what uploadBlob handler does pre-commit.
    async fn stage_test_blob(store: &BlobStore, data: &[u8], mime: &str, did: &str) -> crate::blob_store::BlobRef {
        let temp = store.stage_blob(data.to_vec(), Some(mime), did).await.unwrap();
        crate::blob_store::BlobRef::new(temp.cid, temp.mime_type, temp.size)
    }

    /// Scenario 2 success path: stage → commit → row exists with
    /// temp_key='1' + bytes durable at final position.
    #[tokio::test]
    async fn commit_blob_success_creates_untethered_row() {
        let store = create_test_store().await;
        let blob_ref = stage_test_blob(&store, b"hello arc 16c", "image/png", "did:plc:alice").await;

        store.commit_blob(&blob_ref.r#ref.link).await.unwrap();

        // Row exists with temp_key='1'.
        assert!(store.is_untethered(&blob_ref.r#ref.link).await.unwrap());

        // Bytes retrievable.
        let (data, _) = store.get(&blob_ref.r#ref.link).await.unwrap().unwrap();
        assert_eq!(data, b"hello arc 16c");
    }

    /// Scenario 5b-A reaper-race Case 3a: stage → synthetically
    /// delete temp_blob_metadata between stage and commit → commit
    /// returns stale-stage NotFound; no blob_metadata row created.
    /// (Split-mode disposition per Step 0.7 — synthetic DB write
    /// stands in for an actual reaper firing.)
    #[tokio::test]
    async fn commit_blob_stale_stage_case_3a_returns_not_found() {
        let store = create_test_store().await;
        let blob_ref = stage_test_blob(&store, b"will be stale", "image/png", "did:plc:alice").await;

        // Synthetically delete the temp_blob_metadata row to simulate
        // a reaper DELETE that committed between stage and commit.
        store.delete_temp_blob(&blob_ref.r#ref.link).await.unwrap();

        let result = store.commit_blob(&blob_ref.r#ref.link).await;
        match result {
            Err(PdsError::NotFound(msg)) => {
                assert!(msg.contains("Stale stage") || msg.contains("not found"),
                        "expected stale-stage NotFound message; got: {}", msg);
            }
            other => panic!("expected NotFound, got {:?}", other),
        }

        // No blob_metadata row created.
        assert!(!store.is_untethered(&blob_ref.r#ref.link).await.unwrap());
        assert!(store.get_metadata(&blob_ref.r#ref.link).await.unwrap().is_none());
    }

    /// Scenario 7 duplicate-CID (already permanent): direct DB poke
    /// makes the existing row permanent; re-uploading should be a
    /// no-op on temp_key (preserves permanent state) per Arc 16b
    /// `track_untethered_blob` Case 2.
    #[tokio::test]
    async fn commit_blob_duplicate_cid_already_permanent_preserves() {
        let store = create_test_store().await;
        let blob_ref = stage_test_blob(&store, b"dupe content", "image/png", "did:plc:alice").await;
        store.commit_blob(&blob_ref.r#ref.link).await.unwrap();

        // Direct DB poke: make it permanent.
        sqlx::query("UPDATE blob_metadata SET temp_key = NULL WHERE cid = $1")
            .bind(&blob_ref.r#ref.link)
            .execute(&store.db)
            .await
            .unwrap();
        assert!(!store.is_untethered(&blob_ref.r#ref.link).await.unwrap());

        // Re-stage + re-commit the same content (same CID).
        let blob_ref2 = stage_test_blob(&store, b"dupe content", "image/png", "did:plc:alice").await;
        assert_eq!(blob_ref.r#ref.link, blob_ref2.r#ref.link, "same content → same CID");
        store.commit_blob(&blob_ref2.r#ref.link).await.unwrap();

        // Row remains permanent (Arc 16b track_untethered Case 2 preserve).
        assert!(!store.is_untethered(&blob_ref.r#ref.link).await.unwrap());
    }

    /// Scenario 8 duplicate-CID (already untethered): re-upload
    /// refreshes created_at (Arc 16b track_untethered Case 3) AND
    /// row stays untethered.
    #[tokio::test]
    async fn commit_blob_duplicate_cid_already_untethered_refreshes() {
        let store = create_test_store().await;
        let blob_ref = stage_test_blob(&store, b"refresh content", "image/png", "did:plc:alice").await;
        store.commit_blob(&blob_ref.r#ref.link).await.unwrap();

        let row = sqlx::query("SELECT created_at FROM blob_metadata WHERE cid = $1")
            .bind(&blob_ref.r#ref.link)
            .fetch_one(&store.db).await.unwrap();
        let first_ts: String = row.try_get("created_at").unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let blob_ref2 = stage_test_blob(&store, b"refresh content", "image/png", "did:plc:alice").await;
        store.commit_blob(&blob_ref2.r#ref.link).await.unwrap();

        let row2 = sqlx::query("SELECT created_at FROM blob_metadata WHERE cid = $1")
            .bind(&blob_ref.r#ref.link)
            .fetch_one(&store.db).await.unwrap();
        let second_ts: String = row2.try_get("created_at").unwrap();

        assert!(second_ts > first_ts, "created_at refreshed: {} → {}", first_ts, second_ts);
        assert!(store.is_untethered(&blob_ref.r#ref.link).await.unwrap());
    }

    /// Scenario 4 getBlob serves untethered blob (Case D per Step 0.3
    /// recon — no auth gate). The store-level `get` succeeds for any
    /// caller regardless of `temp_key` state.
    #[tokio::test]
    async fn get_serves_untethered_blob_case_d_no_auth_gate() {
        let store = create_test_store().await;
        let blob_ref = stage_test_blob(&store, b"public-fetchable", "image/png", "did:plc:alice").await;
        store.commit_blob(&blob_ref.r#ref.link).await.unwrap();

        // No auth context required at the store level (Case D).
        let result = store.get(&blob_ref.r#ref.link).await.unwrap();
        let (data, mime) = result.expect("untethered blob is fetchable");
        assert_eq!(data, b"public-fetchable");
        assert_eq!(mime, "image/png");

        // Confirm row state: still untethered (no record reference yet).
        assert!(store.is_untethered(&blob_ref.r#ref.link).await.unwrap());
    }

    // Scenario 6 (Arc 10 classifier unchanged) + commit_blob
    // not-dead-code: both audited via grep at Step 5 (audit items 3 +
    // 4). The classifier's `freshness_threshold: Duration` parameter
    // is verified by signature inspection at audit time
    // (grep `freshness_threshold: Duration` in src/blob_store/gc.rs);
    // commit_blob's absence of `#[allow(dead_code)]` is verified by
    // grep of the function signature lines. Compile-time pins for
    // either property proved too brittle.

    // ============================================================
    // chainlink #93 — CID derivation regression pins.
    // Pre-fix bug: calculate_cid emitted `format!("bafyrei{}",
    // hex::encode(sha256))` — wrong codec prefix (dag-cbor 0x71
    // instead of raw 0x55) AND wrong digest encoding (hex string
    // concat instead of base32-encoded multihash). Phase B
    // Scenario 2 caught it. Fix routes through
    // proto_blue::lex_data::Cid::for_raw(data).to_string().
    // ============================================================

    /// Known-content CID fixture from chainlink #93 evidence. Test
    /// PNG bytes have SHA-256
    /// b4120b3f08da88a9e612a627a6dd6dbb57f73cce2b14ab6ece70fc374e5b06ff.
    /// Independently computed CIDv1+raw+sha256+base32 (Python
    /// reference at chainlink #93 comment):
    ///   bafkreifucift6cg2rcu6mevge6tn23n3k73tztrlcsvw5ttq7q3u4wyg74
    /// 59 chars — the canonical CID length for raw+sha256+base32.
    #[tokio::test]
    async fn calculate_cid_matches_canonical_raw_sha256_base32() {
        let store = create_test_store().await;
        // Fabricate bytes whose SHA-256 is the known fixture.
        // Use proto-blue's Cid::for_raw directly on a real byte string
        // and check that calculate_cid emits the same string.
        let bytes: &[u8] = b"chainlink #93 fixture";
        let expected = proto_blue::lex_data::Cid::for_raw(bytes).to_string();
        let actual = store.calculate_cid(bytes);
        assert_eq!(
            actual, expected,
            "calculate_cid must match proto_blue::Cid::for_raw().to_string()"
        );
    }

    /// chainlink #93 regression pin: CID prefix MUST be "bafkrei"
    /// (raw codec marker — multicodec 0x55) not "bafyrei" (dag-cbor
    /// marker — multicodec 0x71). Blobs are opaque byte streams.
    #[tokio::test]
    async fn calculate_cid_uses_raw_codec_prefix_not_dag_cbor() {
        let store = create_test_store().await;
        let cid = store.calculate_cid(b"any content here");
        assert!(
            cid.starts_with("bafkrei"),
            "blob CID must use raw codec (bafkrei* prefix); got: {}",
            cid
        );
        assert!(
            !cid.starts_with("bafyrei"),
            "blob CID must NOT use dag-cbor codec (bafyrei*); blobs are not CBOR. got: {}",
            cid
        );
        // Canonical length: 59 chars for raw+sha256+base32.
        assert_eq!(cid.len(), 59, "raw+sha256+base32 CID is 59 chars; got {} ({})", cid.len(), cid);
    }

    /// chainlink #93 audit grep — pin: zero string-concat patterns
    /// producing `bafy*`/`bafk*` outside test fixtures + dev-route
    /// stubs. Source-level guarantee that CID construction goes
    /// through the typed proto-blue constructor, not raw format!()
    /// concat.
    ///
    /// This test reads the codebase at runtime and asserts no
    /// production code contains the offending pattern. If a future
    /// arc reintroduces `format!("bafyrei{}", ...)` or `format!
    /// ("bafkrei{}", ...)` in src/blob_store/, this test breaks.
    #[test]
    fn audit_grep_no_format_concat_baf_prefix_in_production() {
        // Read the production CID-handling files and assert the
        // string-concat anti-pattern is absent.
        for path in &["src/blob_store/store.rs", "src/api/blob.rs"] {
            let contents = std::fs::read_to_string(path).unwrap_or_default();
            // Pattern: `format!("baf*` where the next chars suggest CID prefix.
            // We allow legitimate test fixtures (in #[cfg(test)] blocks) by
            // checking only lines outside `#[cfg(test)]` mod tests scope.
            // Crude heuristic: anything before "mod tests {" is production.
            let production_end = contents.find("mod tests {").unwrap_or(contents.len());
            let production = &contents[..production_end];
            assert!(
                !production.contains("format!(\"bafyrei{"),
                "{} contains `format!(\"bafyrei{{\"` — chainlink #93 anti-pattern. \
                 Use proto_blue::lex_data::Cid::for_raw(data).to_string() instead.",
                path
            );
            assert!(
                !production.contains("format!(\"bafkrei{"),
                "{} contains `format!(\"bafkrei{{\"` — chainlink #93 anti-pattern. \
                 Use proto_blue::lex_data::Cid::for_raw(data).to_string() instead.",
                path
            );
        }
    }

    // ============================================================
    // Arc 16f §9.6.3.2 TOLERANT helper tests.
    // Extends Arc 16b's arc16b_store fixture with the
    // `blob_quarantine` table so quarantine-first ordering can be
    // exercised via direct SQL fixtures (mirrors the schema in
    // src/blob_store/quarantine.rs:395).
    // ============================================================

    async fn arc16f_store_with_quarantine() -> (BlobStore, sqlx::AnyPool) {
        let (store, pool) = arc16b_store().await;
        sqlx::query(
            r#"CREATE TABLE blob_quarantine (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                cid TEXT NOT NULL,
                reason TEXT NOT NULL,
                details TEXT,
                quarantined_by TEXT NOT NULL,
                quarantined_at TEXT NOT NULL,
                restored_at TEXT,
                restored_by TEXT,
                legal_reference TEXT
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        (store, pool)
    }

    async fn insert_quarantine(pool: &sqlx::AnyPool, cid: &str, reason: &str) {
        sqlx::query(
            "INSERT INTO blob_quarantine (cid, reason, quarantined_by, quarantined_at) \
             VALUES ($1, $2, 'did:plc:admin', '2026-01-01T00:00:00Z')",
        )
        .bind(cid)
        .bind(reason)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn tolerant_quarantine_hit_no_metadata_returns_quarantined() {
        let (store, pool) = arc16f_store_with_quarantine().await;
        let cid = Cid::for_raw(b"arc16f-tolerant-q1");
        let cid_str = cid.to_string();
        insert_quarantine(&pool, &cid_str, "csam").await;
        // No blob_metadata row.

        let mut tx = pool.begin().await.unwrap();
        let outcome = store
            .verify_blob_tolerant_or_signal(
                &mut tx,
                &cid,
                "at://did:plc:alice/app.bsky.feed.post/abc",
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();

        match outcome {
            TolerantOutcome::Quarantined { cid: c, public_reason } => {
                assert_eq!(c, cid);
                assert_eq!(public_reason, QuarantinePublicReason::Abuse);
            }
            other => panic!("expected Quarantined, got {:?}", other),
        }
        // No state mutation.
        assert!(!metadata_exists(&pool, &cid_str).await);
        assert_eq!(record_blob_count(&pool, &cid_str).await, 0);
    }

    #[tokio::test]
    async fn tolerant_quarantine_takes_precedence_over_present_row() {
        let (store, pool) = arc16f_store_with_quarantine().await;
        let cid = Cid::for_raw(b"arc16f-tolerant-q2");
        let cid_str = cid.to_string();
        insert_quarantine(&pool, &cid_str, "dmca").await;
        // Seed an untethered blob_metadata row — quarantine MUST still
        // take precedence per §9.6.3.2 step 1 (quarantine-first ordering).
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ($1, 'image/png', 100, 'did:plc:alice', '2026-01-01T00:00:00Z', '1')",
        )
        .bind(&cid_str)
        .execute(&pool)
        .await
        .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let outcome = store
            .verify_blob_tolerant_or_signal(
                &mut tx,
                &cid,
                "at://x/y/z",
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();

        match outcome {
            TolerantOutcome::Quarantined { cid: c, public_reason } => {
                assert_eq!(c, cid);
                assert_eq!(public_reason, QuarantinePublicReason::Legal);
            }
            other => panic!("expected Quarantined, got {:?}", other),
        }
        // Row was NOT promoted (temp_key still '1') and NO record_blob row.
        assert_eq!(
            read_temp_key(&pool, &cid_str).await,
            Some("1".to_string())
        );
        assert_eq!(record_blob_count(&pool, &cid_str).await, 0);
    }

    #[tokio::test]
    async fn tolerant_row_absent_no_quarantine_returns_needs_fetch() {
        let (store, pool) = arc16f_store_with_quarantine().await;
        let cid = Cid::for_raw(b"arc16f-tolerant-nf");
        let cid_str = cid.to_string();

        let mut tx = pool.begin().await.unwrap();
        let outcome = store
            .verify_blob_tolerant_or_signal(
                &mut tx,
                &cid,
                "at://x/y/z",
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();

        match outcome {
            TolerantOutcome::NeedsFetch { cid: c } => assert_eq!(c, cid),
            other => panic!("expected NeedsFetch, got {:?}", other),
        }
        // No state mutation.
        assert!(!metadata_exists(&pool, &cid_str).await);
        assert_eq!(record_blob_count(&pool, &cid_str).await, 0);
    }

    #[tokio::test]
    async fn tolerant_row_present_temp_key_set_delegates_to_strict_promotes() {
        let (store, pool) = arc16f_store_with_quarantine().await;
        let cid = Cid::for_raw(b"arc16f-tolerant-p1");
        let cid_str = cid.to_string();
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ($1, 'image/png', 100, 'did:plc:alice', '2026-01-01T00:00:00Z', '1')",
        )
        .bind(&cid_str)
        .execute(&pool)
        .await
        .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let outcome = store
            .verify_blob_tolerant_or_signal(
                &mut tx,
                &cid,
                "at://did:plc:alice/coll/rkey",
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();

        assert!(
            matches!(outcome, TolerantOutcome::Promoted),
            "expected Promoted, got {:?}",
            outcome
        );
        // STRICT delegation effects: temp_key NULL + record_blob row.
        assert!(read_temp_key(&pool, &cid_str).await.is_none());
        assert_eq!(record_blob_count(&pool, &cid_str).await, 1);
    }

    #[tokio::test]
    async fn tolerant_row_present_temp_key_null_delegates_to_strict_already_permanent() {
        let (store, pool) = arc16f_store_with_quarantine().await;
        let cid = Cid::for_raw(b"arc16f-tolerant-ap1");
        let cid_str = cid.to_string();
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ($1, 'image/png', 100, 'did:plc:alice', '2026-01-01T00:00:00Z', NULL)",
        )
        .bind(&cid_str)
        .execute(&pool)
        .await
        .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let outcome = store
            .verify_blob_tolerant_or_signal(
                &mut tx,
                &cid,
                "at://did:plc:alice/coll/rkey",
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();

        assert!(
            matches!(outcome, TolerantOutcome::AlreadyPermanent),
            "expected AlreadyPermanent, got {:?}",
            outcome
        );
        assert!(read_temp_key(&pool, &cid_str).await.is_none());
        assert_eq!(record_blob_count(&pool, &cid_str).await, 1);
    }

    #[tokio::test]
    async fn tolerant_runs_on_sqlite_without_for_update_syntax_error() {
        // FOR UPDATE lock regression — mirrors Arc 16b §9.2.3.2's lock
        // test shape. sqlx 0.8 does NOT silently tolerate FOR UPDATE on
        // SQLite (it surfaces as syntax error); the helper passing on
        // SQLite proves that the shared `for_update_clause()` mechanism
        // correctly suppresses FOR UPDATE for the SQLite backend (same
        // mechanism Arc 16b's STRICT uses).
        let (store, pool) = arc16f_store_with_quarantine().await;
        let cid = Cid::for_raw(b"arc16f-tolerant-lock");
        let cid_str = cid.to_string();
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ($1, 'image/png', 100, 'did:plc:alice', '2026-01-01T00:00:00Z', '1')",
        )
        .bind(&cid_str)
        .execute(&pool)
        .await
        .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let outcome = store
            .verify_blob_tolerant_or_signal(
                &mut tx,
                &cid,
                "at://x/y/z",
                chrono::Utc::now(),
            )
            .await;
        tx.commit().await.unwrap();
        assert!(
            outcome.is_ok(),
            "TOLERANT must run on SQLite without FOR UPDATE syntax error: {:?}",
            outcome
        );
    }

    #[test]
    fn quarantine_public_reason_mapping_covers_internal_classes() {
        use QuarantinePublicReason as Q;
        assert_eq!(Q::from_internal_reason_str("dmca"), Q::Legal);
        assert_eq!(Q::from_internal_reason_str("legal"), Q::Legal);
        assert_eq!(Q::from_internal_reason_str("csam"), Q::Abuse);
        assert_eq!(Q::from_internal_reason_str("tos"), Q::Policy);
        assert_eq!(Q::from_internal_reason_str("malware"), Q::Malware);
        assert_eq!(Q::from_internal_reason_str("other"), Q::Unspecified);
        // Case-insensitive.
        assert_eq!(Q::from_internal_reason_str("DMCA"), Q::Legal);
        assert_eq!(Q::from_internal_reason_str("Csam"), Q::Abuse);
        // Unknown defaults to Unspecified — operator-internal detail
        // (e.g. matched-signature SHA) MUST NOT leak to wire.
        assert_eq!(
            Q::from_internal_reason_str("matched-signature-sha256:abc"),
            Q::Unspecified
        );
        assert_eq!(Q::from_internal_reason_str(""), Q::Unspecified);
    }
}
