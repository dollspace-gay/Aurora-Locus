//! `migrate blobs`: copies every tracked object to another backend while
//! the active one keeps serving, then cuts over with a short pause.
//!
//! Each step of the leased job: a copy pass over the rows whose key
//! sequence is above the last pass's mark, then a verify of the same range
//! (every row has a copy record); once a pass finds few changes and no
//! unacknowledged missing source, the cutover: pause (no transaction can
//! set a key), a final pass over the rows above the verified mark plus one
//! retry of each missing source, its verify, and the flip, all bounded by
//! `blobs.cutover_max_pause_ms`; anything short of that clears the pause
//! and the job tries again later. After the flip, moves begun under the old
//! epoch restart and the old epoch's deletes are queued for the new
//! backend; every node reloads onto it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use locus_blob::{BlobStore, DiskStore};
use locus_core::lease::LeaseToken;
use locus_core::{CoreError, CoreResult, locus_nsid};
use locus_store::migrate_blobs::{BlobMigration, Tracked};
use proto_blue_lex_data::Cid;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::admin_auth::{self, Role, Surface};
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

/// A pass with no more changed rows than this tries the cutover.
const FEW_CHANGES: u64 = 100;
const PAGE: i64 = 500;

/// The migration's target store.
async fn target_store(s: &AppState, m: &BlobMigration) -> CoreResult<BlobStore> {
    match m.target.as_str() {
        "disk" => {
            let root = match &m.disk_root {
                Some(r) => std::path::PathBuf::from(r),
                None => s.scratch.root().to_path_buf(),
            };
            Ok(BlobStore::disk(DiskStore::new(root)))
        }
        _ => {
            let env: HashMap<String, String> = std::env::vars().collect();
            let config = locus_blob::s3_config(&s.store, &s.master_key, &s.snapshot(), &env)
                .await?
                .ok_or_else(|| {
                    CoreError::Config(
                        "migrating to S3 needs the S3 settings (blobs.s3_bucket and the rest)"
                            .into(),
                    )
                })?;
            Ok(BlobStore::s3(locus_blob::S3Store::new(&config)?))
        }
    }
}

/// Copies one tracked object to the target and records the outcome.
/// Returns whether it was copied (false: a missing source).
async fn copy_one(
    s: &AppState,
    lease: &LeaseToken,
    target: &BlobStore,
    row: &Tracked,
) -> CoreResult<bool> {
    let mut last_err = None;
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let result = match &row.cid {
            Some(cid) => {
                let cid =
                    Cid::from_str_multibase(cid).map_err(|e| CoreError::Storage(e.to_string()))?;
                locus_blob::copy_verified_between(
                    &s.blobs,
                    &row.storage_key,
                    target,
                    &row.storage_key,
                    &cid,
                )
                .await
            }
            None => match s.blobs.read(&row.storage_key, None).await {
                Ok(Some(r)) => target
                    .write_stream(&row.storage_key, r.body, u64::MAX)
                    .await
                    .map(|_| ()),
                Ok(None) => Err(locus_blob::BlobError::Missing(row.storage_key.clone())),
                Err(e) => Err(e),
            },
        };
        match result {
            Ok(()) => {
                s.store
                    .record_copy(lease, &row.storage_key, "copied")
                    .await?;
                return Ok(true);
            }
            Err(e) => last_err = Some(e),
        }
    }
    tracing::warn!(
        key = row.storage_key,
        "blob migration: the source can't be copied ({}); recorded as missing",
        last_err.map(|e| e.to_string()).unwrap_or_default()
    );
    s.store
        .record_copy(lease, &row.storage_key, "missing_source")
        .await?;
    Ok(false)
}

/// Copies the rows in (`after`, `through`] that have no copy record.
/// Returns how many it copied or recorded.
async fn copy_range(
    s: &AppState,
    lease: &LeaseToken,
    target: &BlobStore,
    after: i64,
    through: i64,
) -> CoreResult<u64> {
    let mut n = 0;
    let mut from = after;
    loop {
        let page = s.store.uncopied_between(from, through, false, PAGE).await?;
        let Some(last) = page.last().map(|r| r.key_seq) else {
            return Ok(n);
        };
        for row in &page {
            copy_one(s, lease, target, row).await?;
            n += 1;
        }
        from = last;
    }
}

/// What one step did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Idle,
    Copied(u64),
    Waiting(String),
    CutoverRetried(String),
    Flipped(i64),
    Done,
}

/// One step of the job (under its lease).
pub async fn step(s: &AppState, lease: &LeaseToken) -> CoreResult<Step> {
    let Some(m) = s.store.active_blob_migration().await? else {
        return Ok(Step::Idle);
    };
    if m.phase == "post_flip" {
        let moves = s.store.finish_blob_migration(lease, &m.id).await?;
        tracing::info!(id = m.id, moves, "blob migration done");
        return Ok(Step::Done);
    }
    let target = target_store(s, &m).await?;
    target.check().await?;
    // A copy pass, then its verify.
    let mark = s.store.key_seq_mark(lease).await?;
    let changed = copy_range(s, lease, &target, m.copied_through, mark).await?;
    let gaps = copy_range(s, lease, &target, m.verified_through, mark).await?;
    if gaps > 0 {
        tracing::warn!(
            id = m.id,
            gaps,
            "blob migration verify found rows without a copy; copied"
        );
    }
    s.store
        .set_migration_marks(lease, &m.id, mark, mark, None)
        .await?;
    if changed > FEW_CHANGES {
        return Ok(Step::Copied(changed));
    }
    let unacked: Vec<String> = s
        .store
        .missing_sources()
        .await?
        .into_iter()
        .filter(|(_, acked)| !acked)
        .map(|(k, _)| k)
        .collect();
    if !unacked.is_empty() {
        // One retry each before waiting on the operator.
        for key in &unacked {
            let row = Tracked {
                storage_key: key.clone(),
                cid: None,
                key_seq: 0,
            };
            let _ = copy_one(s, lease, &target, &row).await;
        }
        let still = s
            .store
            .missing_sources()
            .await?
            .into_iter()
            .filter(|(_, a)| !a)
            .count();
        if still > 0 {
            let note = format!(
                "waiting: {still} objects are missing from the source; check them, then \
                 `aurora-locus migrate blobs --accept-missing`"
            );
            s.store
                .set_migration_marks(lease, &m.id, mark, mark, Some(&note))
                .await?;
            return Ok(Step::Waiting(note));
        }
    }
    // The cutover.
    let max = Duration::from_millis(
        u64::try_from(s.snapshot().int("blobs.cutover_max_pause_ms")).unwrap_or(60_000),
    );
    if !s.store.pause_for_cutover(lease, &m.id).await? {
        return Ok(Step::Idle);
    }
    let started = Instant::now();
    let fin = async {
        // The final pass: everything above the verified mark, then its verify
        // (acknowledged missing sources count; nothing can change meanwhile).
        copy_range(s, lease, &target, mark, i64::MAX).await?;
        let gaps = s.store.uncopied_between(mark, i64::MAX, true, 1).await?;
        let unacked = s
            .store
            .missing_sources()
            .await?
            .into_iter()
            .any(|(_, a)| !a);
        if !gaps.is_empty() || unacked {
            return Ok::<_, CoreError>(Some("the final pass left rows without a copy".to_owned()));
        }
        Ok(None)
    };
    let outcome = tokio::time::timeout(max, fin).await;
    let failure = match outcome {
        Ok(Ok(None)) if started.elapsed() <= max => None,
        Ok(Ok(Some(why))) => Some(why),
        Ok(Err(e)) => Some(e.to_string()),
        _ => Some(format!(
            "the final pass took longer than {} ms",
            max.as_millis()
        )),
    };
    if let Some(why) = failure {
        s.store.clear_pause(lease).await?;
        let note = format!("cutover retried later: {why}");
        s.store
            .set_migration_marks(lease, &m.id, mark, mark, Some(&note))
            .await?;
        return Ok(Step::CutoverRetried(note));
    }
    match s.store.flip_blob_backend(lease, &m).await? {
        Some(epoch) => {
            tracing::info!(id = m.id, target = m.target, epoch, "blob backend flipped");
            crate::keys::reload_soon(s);
            Ok(Step::Flipped(epoch))
        }
        None => {
            s.store.clear_pause(lease).await?;
            Ok(Step::Idle)
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartInput {
    to: String,
    disk_root: Option<String>,
}

/// `admin.migrateBlobs.start`.
pub async fn start(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<StartInput>,
) -> XrpcResult<Json<Value>> {
    let who = admin_auth::require(
        &s,
        &headers,
        locus_nsid!("admin.migrateBlobs.start"),
        Surface::Locus,
        Role::Admin,
    )
    .await?;
    if input.to != "disk" && input.to != "s3" {
        return Err(XrpcError::invalid_request("Migrate to disk or s3"));
    }
    let id = format!(
        "{}-{}",
        s.store.db_now_ms().await?,
        hex::encode(rand::random::<[u8; 3]>())
    );
    let row = BlobMigration {
        id: id.clone(),
        target: input.to.clone(),
        disk_root: input.disk_root.clone(),
        phase: "copying".into(),
        copied_through: 0,
        verified_through: 0,
        passes: 0,
        started_at: 0,
        started_by: who.actor.clone(),
        finished_at: None,
        note: None,
    };
    // The target must answer before anything is recorded.
    target_store(&s, &row)
        .await
        .map_err(|e| XrpcError::invalid_request(e.to_string()))?
        .check()
        .await
        .map_err(|e| XrpcError::invalid_request(format!("the target doesn't answer: {e}")))?;
    s.store
        .start_blob_migration(&id, &input.to, input.disk_root.as_deref(), &who.actor)
        .await?;
    admin_auth::audit(
        &s,
        &who,
        "migrateBlobs.start",
        Some(&id),
        Some(json!({"to": input.to, "diskRoot": input.disk_root})),
    )
    .await;
    Ok(Json(json!({"id": id})))
}

/// `viewer.migrateBlobs.status`.
pub async fn status(State(s): State<Arc<AppState>>, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    admin_auth::require(
        &s,
        &headers,
        locus_nsid!("viewer.migrateBlobs.status"),
        Surface::Locus,
        Role::Viewer,
    )
    .await?;
    let backend = s.store.blob_backend().await?;
    let latest = s.store.latest_blob_migration().await?;
    let missing = s.store.missing_sources().await?;
    Ok(Json(json!({
        "backend": {"active": backend.active, "epoch": backend.epoch, "paused": backend.paused, "diskRoot": backend.disk_root},
        "migration": latest.map(|m| json!({
            "id": m.id, "to": m.target, "diskRoot": m.disk_root, "phase": m.phase,
            "copiedThrough": m.copied_through, "verifiedThrough": m.verified_through,
            "passes": m.passes, "startedAt": m.started_at, "startedBy": m.started_by,
            "finishedAt": m.finished_at, "note": m.note,
        })),
        "missingSources": missing.iter().map(|(k, a)| json!({"key": k, "acknowledged": a})).collect::<Vec<_>>(),
    })))
}

/// `admin.migrateBlobs.cancel`.
pub async fn cancel(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(_): Json<Value>,
) -> XrpcResult<Json<Value>> {
    let who = admin_auth::require(
        &s,
        &headers,
        locus_nsid!("admin.migrateBlobs.cancel"),
        Surface::Locus,
        Role::Admin,
    )
    .await?;
    let id = s
        .store
        .cancel_blob_migration()
        .await?
        .ok_or_else(|| XrpcError::invalid_request("No blob migration is running"))?;
    admin_auth::audit(&s, &who, "migrateBlobs.cancel", Some(&id), None).await;
    Ok(Json(json!({"id": id})))
}

/// `admin.migrateBlobs.acceptMissing`: acknowledges the objects missing
/// from the source, so the cutover can go ahead without them.
pub async fn accept_missing(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(_): Json<Value>,
) -> XrpcResult<Json<Value>> {
    let who = admin_auth::require(
        &s,
        &headers,
        locus_nsid!("admin.migrateBlobs.acceptMissing"),
        Surface::Locus,
        Role::Admin,
    )
    .await?;
    let keys: Vec<String> = s
        .store
        .missing_sources()
        .await?
        .into_iter()
        .filter(|(_, a)| !a)
        .map(|(k, _)| k)
        .collect();
    let n = s.store.acknowledge_missing_sources().await?;
    admin_auth::audit(
        &s,
        &who,
        "migrateBlobs.acceptMissing",
        None,
        Some(json!({"keys": keys})),
    )
    .await;
    Ok(Json(json!({"acknowledged": n})))
}
