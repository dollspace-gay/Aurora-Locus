//! `backup create` (SQLite installs): a database snapshot first, then
//! exactly the objects that snapshot names, then an encrypted key backup,
//! and last a manifest saying the backup is whole. The run holds the
//! `backup` lease; while it does, the delete job deletes nothing the
//! snapshot could name. A run that loses its lease is marked failed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use futures::StreamExt as _;
use locus_core::locus_nsid;
use locus_store::Store;
use locus_store::backups::BACKUP_LEASE;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt as _;
use zeroize::Zeroizing;

use crate::admin_auth::{self, Role, Surface};
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

/// The key-store name of the scheduled backups' key backup passphrase.
pub const PASSPHRASE_SECRET: &str = "backups/passphrase";
/// The manifest's `format`.
pub const FORMAT: &str = "aurora-locus-backup";
const LEASE_TTL: Duration = Duration::from_secs(120);

/// What a backup says about itself, in `manifest.json`, written last.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub id: String,
    pub created_at: i64,
    pub finished_at: i64,
    /// `done`, or `failed` (objects it names may be missing).
    pub status: String,
    pub error: Option<String>,
    pub service_did: String,
    pub master_key_id: String,
    pub delete_mark: i64,
    pub objects: u64,
    /// Keys the snapshot names whose objects weren't in the store.
    pub missing: Vec<String>,
    /// Whether `keys.backup` is there.
    pub keys_backup: bool,
}

/// Where backups go.
pub fn backups_dir(s: &AppState) -> PathBuf {
    let dir = s.snapshot().text("backups.dir").trim().to_owned();
    if dir.is_empty() {
        s.scratch
            .local_path("backups")
            .unwrap_or_else(|_| PathBuf::from("backups"))
    } else {
        PathBuf::from(dir)
    }
}

/// Starts a backup in the background; returns its id. Refused while one
/// runs, and on Postgres.
pub async fn start(
    s: Arc<AppState>,
    passphrase: Option<Zeroizing<String>>,
    by: &str,
) -> XrpcResult<String> {
    if s.store.is_postgres() {
        return Err(XrpcError::invalid_request(
            "backup create is for SQLite installs; on Postgres, use point-in-time recovery and \
             versioned buckets, with `aurora-locus keys backup` for the keys",
        ));
    }
    if let Some(p) = &passphrase
        && p.chars().count() < locus_core::key_backup::MIN_PASSPHRASE
    {
        return Err(XrpcError::invalid_request(
            "A key backup passphrase is at least 12 characters",
        ));
    }
    let lease = s
        .store
        .try_acquire_lease(BACKUP_LEASE, &s.node_id, LEASE_TTL)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("A backup is already running"))?;
    let started = s.store.db_now_ms().await?;
    let id = format!("{started}-{}", hex::encode(rand::random::<[u8; 3]>()));
    let dir = backups_dir(&s).join(&id);
    tokio::fs::create_dir_all(dir.join("objects"))
        .await
        .map_err(|e| XrpcError::internal(format!("{}: {e}", dir.display())))?;
    s.store
        .start_backup_run(&id, &dir.display().to_string(), by)
        .await?;
    let run_id = id.clone();
    tokio::spawn(async move {
        let lost = Arc::new(AtomicBool::new(false));
        let renewer = {
            let (store, lease, lost) = (s.store.clone(), lease.clone(), Arc::clone(&lost));
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(LEASE_TTL / 4).await;
                    if !store.renew_lease(&lease).await.unwrap_or(false) {
                        lost.store(true, Ordering::SeqCst);
                        return;
                    }
                }
            })
        };
        let result = run(&s, &run_id, &dir, started, passphrase, &lost).await;
        renewer.abort();
        let (status, error, objects, missing, keys) = match &result {
            Ok(m) => ("done", None, m.objects, m.missing.len(), m.keys_backup),
            Err(e) => ("failed", Some(e.clone()), 0, 0, false),
        };
        if let Err(e) = &result {
            tracing::warn!(id = run_id, "backup failed: {e}");
            let _ = write_manifest(
                &dir,
                &Manifest {
                    format: FORMAT.into(),
                    version: 1,
                    id: run_id.clone(),
                    created_at: started,
                    finished_at: locus_core::time::now_ms(),
                    status: "failed".into(),
                    error: Some(e.clone()),
                    service_did: s.service_did.clone(),
                    master_key_id: s.master_key.key_id(),
                    delete_mark: 0,
                    objects: 0,
                    missing: Vec::new(),
                    keys_backup: false,
                },
            )
            .await;
        } else {
            tracing::info!(id = run_id, objects, missing, "backup made");
        }
        let _ = s
            .store
            .finish_backup_run(
                &run_id,
                status,
                i64::try_from(objects).unwrap_or(i64::MAX),
                i64::try_from(missing).unwrap_or(i64::MAX),
                keys,
                error.as_deref(),
            )
            .await;
        let _ = s.store.release_lease(&lease).await;
        prune(&s).await;
    });
    Ok(id)
}

async fn write_manifest(dir: &Path, m: &Manifest) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(m).map_err(std::io::Error::other)?;
    let tmp = dir.join("manifest.json.part");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::File::open(&tmp).await?.sync_all().await?;
    tokio::fs::rename(&tmp, dir.join("manifest.json")).await
}

async fn run(
    s: &Arc<AppState>,
    id: &str,
    dir: &Path,
    started: i64,
    passphrase: Option<Zeroizing<String>>,
    lost: &AtomicBool,
) -> Result<Manifest, String> {
    let err = |e: &dyn std::fmt::Display| e.to_string();
    let check_lease = || {
        if lost.load(Ordering::SeqCst) {
            Err("the backup lost its lease, so objects it names may have been deleted".to_owned())
        } else {
            Ok(())
        }
    };
    // 1. The snapshot, and the delete mark read from it.
    let db = dir.join("db.sqlite");
    s.store.sqlite_snapshot(&db).await.map_err(|e| err(&e))?;
    let snap = Store::open_sqlite_read_only(&db)
        .await
        .map_err(|e| err(&e))?;
    let mark = snap.max_blob_delete_id().await.map_err(|e| err(&e))?;
    s.store
        .set_backup_mark(id, mark)
        .await
        .map_err(|e| err(&e))?;
    check_lease()?;
    // 2. Exactly the objects the snapshot names.
    let out = locus_blob::BlobStore::Disk(locus_blob::DiskStore::new(dir.join("objects")));
    let mut copied = 0u64;
    let mut missing = Vec::new();
    for pending in [false, true] {
        let mut after = String::new();
        loop {
            let keys = if pending {
                snap.pending_keys_after(&after, 500).await
            } else {
                snap.tracked_keys_after(&after, 500).await
            }
            .map_err(|e| err(&e))?;
            let Some(last) = keys.last().cloned() else {
                break;
            };
            for key in &keys {
                check_lease()?;
                match s.blobs.read(key, None).await.map_err(|e| err(&e))? {
                    Some(r) => {
                        out.write_stream(key, r.body, u64::MAX)
                            .await
                            .map_err(|e| err(&e))?;
                        copied += 1;
                    }
                    // A pending move's copy may not exist yet.
                    None if pending => {}
                    None => missing.push(key.clone()),
                }
            }
            after = last;
        }
    }
    snap.close().await;
    // 3. The key backup.
    let keys_backup = match passphrase {
        Some(p) => {
            let body = crate::keys::backup_stream(Arc::clone(s), &p, format!("backup:{id}"))
                .await
                .map_err(|e| e.message)?;
            let mut f = tokio::fs::File::create(dir.join("keys.backup"))
                .await
                .map_err(|e| err(&e))?;
            let mut stream = body.into_data_stream();
            while let Some(chunk) = stream.next().await {
                f.write_all(&chunk.map_err(|e| err(&e))?)
                    .await
                    .map_err(|e| err(&e))?;
            }
            f.sync_all().await.map_err(|e| err(&e))?;
            true
        }
        None => false,
    };
    check_lease()?;
    // 4. The manifest, last: a backup without one isn't whole.
    let m = Manifest {
        format: FORMAT.into(),
        version: 1,
        id: id.to_owned(),
        created_at: started,
        finished_at: locus_core::time::now_ms(),
        status: "done".into(),
        error: None,
        service_did: s.service_did.clone(),
        master_key_id: s.master_key.key_id(),
        delete_mark: mark,
        objects: copied,
        missing,
        keys_backup,
    };
    write_manifest(dir, &m).await.map_err(|e| err(&e))?;
    Ok(m)
}

/// Removes finished backups beyond `backups.keep`, oldest first.
async fn prune(s: &AppState) {
    let keep = usize::try_from(s.snapshot().int("backups.keep")).unwrap_or(7);
    let Ok(runs) = s.store.backup_runs(1000).await else {
        return;
    };
    for r in runs.iter().filter(|r| r.status == "done").skip(keep) {
        if let Err(e) = tokio::fs::remove_dir_all(&r.dir).await {
            tracing::debug!(dir = r.dir, "old backup not removed: {e}");
        }
    }
}

/// Starts a scheduled backup when one is due.
pub async fn schedule_step(s: Arc<AppState>) -> locus_core::CoreResult<bool> {
    let hours = s.snapshot().int("backups.every_hours");
    if hours <= 0 || s.store.is_postgres() {
        return Ok(false);
    }
    let last = s.store.backup_runs(1).await?.into_iter().next();
    let now = s.store.db_now_ms().await?;
    if last.is_some_and(|r| now - r.started_at < hours * 3_600_000) {
        return Ok(false);
    }
    let passphrase = s
        .store
        .get_secret(PASSPHRASE_SECRET, &s.master_key)
        .await?
        .and_then(|b| String::from_utf8(b.to_vec()).ok())
        .map(Zeroizing::new);
    if passphrase.is_none() {
        tracing::warn!(
            "scheduled backup without a key backup: set one with `aurora-locus keys set backup-passphrase`"
        );
    }
    match start(s, passphrase, "schedule").await {
        Ok(_) => Ok(true),
        Err(e) => {
            tracing::warn!("scheduled backup didn't start: {}", e.message);
            Ok(false)
        }
    }
}

#[derive(Deserialize, Default)]
pub struct CreateInput {
    /// The key backup's passphrase; none for no key backup.
    passphrase: Option<String>,
}

/// `owner.backup.create`.
pub async fn create(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<CreateInput>,
) -> XrpcResult<Json<Value>> {
    let who = admin_auth::require(
        &s,
        &headers,
        locus_nsid!("owner.backup.create"),
        Surface::Locus,
        Role::Owner,
    )
    .await?;
    let id = start(
        Arc::clone(&s),
        input.passphrase.map(Zeroizing::new),
        &who.actor,
    )
    .await?;
    admin_auth::audit(&s, &who, "backup.create", Some(&id), None).await;
    Ok(Json(
        json!({"id": id, "dir": backups_dir(&s).join(&id).display().to_string()}),
    ))
}

/// `viewer.backup.list`.
pub async fn list(State(s): State<Arc<AppState>>, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    admin_auth::require(
        &s,
        &headers,
        locus_nsid!("viewer.backup.list"),
        Surface::Locus,
        Role::Viewer,
    )
    .await?;
    let runs = s.store.backup_runs(50).await?;
    Ok(Json(json!({
        "backups": runs.iter().map(|r| json!({
            "id": r.id,
            "status": r.status,
            "startedAt": r.started_at,
            "finishedAt": r.finished_at,
            "dir": r.dir,
            "objects": r.objects,
            "missing": r.missing,
            "keysBackup": r.with_keys,
            "by": r.started_by,
            "error": r.error,
        })).collect::<Vec<_>>(),
    })))
}
