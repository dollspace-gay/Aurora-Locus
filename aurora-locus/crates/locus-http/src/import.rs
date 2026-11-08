//! Repository import: `com.atproto.repo.importRepo`,
//! `com.atproto.repo.listMissingBlobs` and
//! `app.nearhorizon.locus.account.getImportStatus`.

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use locus_repo::import::ImportError;
use locus_store::accounts::Status;
use locus_store::bulk::BulkJob;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth;
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

type St = State<Arc<AppState>>;

fn import_error(e: ImportError) -> XrpcError {
    match e {
        ImportError::Invalid(m) => XrpcError::invalid_request(m),
        ImportError::Core(locus_core::CoreError::Conflict(_)) => crate::account::write_blocked(),
        ImportError::Core(c) => c.into(),
    }
}

fn too_large() -> XrpcError {
    XrpcError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "PayloadTooLarge",
        "request entity too large",
    )
}

/// `com.atproto.repo.importRepo`: the CAR is streamed to a temp file, then
/// applied as an import job. Returns once the record index follows the new
/// root; the job's last phase runs in the background, writes blocked until
/// it ends.
pub async fn import_repo(State(s): St, headers: HeaderMap, body: Body) -> XrpcResult<()> {
    let authed = auth::authorization(&s, &headers, auth::FULL, &[], |p| {
        Ok(p.assert_account("repo", "manage")?)
    })
    .await?;
    let account = auth::account_checked(&s, &authed.did, false).await?;
    let snap = s.snapshot();
    if !snap.bool("accounts.accept_imports") {
        return Err(XrpcError::invalid_request(
            "Service is not accepting repo imports",
        ));
    }
    let limit = u64::try_from(snap.int("limits.import_size")).unwrap_or(u64::MAX);
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > limit) {
        return Err(too_large());
    }
    let did = account.did.clone();
    let job = s
        .repo
        .start_import(&did, &s.node_id)
        .await
        .map_err(import_error)?;

    let key = format!("imports/{}.car", job.id);
    let written = s
        .scratch
        .write_stream(&key, body.into_data_stream(), limit)
        .await;
    if let Err(e) = written {
        let (err, why) = match e {
            locus_blob::BlobError::TooLarge { .. } => {
                (too_large(), "the upload is too large".to_owned())
            }
            other => (
                XrpcError::invalid_request("Could not read the upload"),
                format!("could not read the upload: {other}"),
            ),
        };
        if let Err(re) = s.repo.import_rollback(&job, &why).await {
            tracing::warn!(job = job.id, "import rollback failed: {re}");
        }
        return Err(err);
    }
    let result = run(&s, &job, &key, account.status == Status::Active).await;
    if let Err(e) = s.scratch.delete(&key).await {
        tracing::warn!(job = job.id, "could not remove the import upload: {e}");
    }
    result?;
    spawn_finish(&s, job);
    Ok(())
}

async fn run(s: &AppState, job: &BulkJob, key: &str, active: bool) -> XrpcResult<()> {
    let path = s
        .scratch
        .local_path(key)
        .map_err(|e| XrpcError::internal(e.to_string()))?;
    // The account's current key, for the signature check (a mismatch is
    // noted, not refused).
    let doc_key = s
        .resolve_did_doc(&job.did, false)
        .await
        .ok()
        .flatten()
        .and_then(|d| proto_blue_common::get_signing_did_key(&d));
    let signer = s.signing_key(&job.did).await?;
    let moves = s
        .repo
        .run_import(job, &path, doc_key.as_deref(), &signer, active)
        .await
        .map_err(import_error)?;
    crate::repo::finish_moves(s, &moves).await;
    Ok(())
}

/// Runs an import's last phase in the background; the bulk-job resumer
/// finishes it if this node stops first.
fn spawn_finish(s: &Arc<AppState>, job: BulkJob) {
    let s = Arc::clone(s);
    tokio::spawn(async move {
        match s.repo.finish_import(&job).await {
            Ok(moves) => crate::repo::finish_moves(&s, &moves).await,
            Err(e) => tracing::warn!(did = job.did, job = job.id, "import paused: {e}"),
        }
    });
}

/// Resumes an import job the resumer took over.
pub async fn resume_import(s: &AppState, job: &BulkJob) -> locus_core::CoreResult<()> {
    match s.repo.resume_import(job).await {
        Ok(moves) => {
            crate::repo::finish_moves(s, &moves).await;
            Ok(())
        }
        Err(ImportError::Core(e)) => Err(e),
        Err(ImportError::Invalid(m)) => Err(locus_core::CoreError::Storage(m)),
    }
}

#[derive(Deserialize)]
pub struct MissingParams {
    limit: Option<i64>,
    cursor: Option<String>,
}

/// `com.atproto.repo.listMissingBlobs`: blobs the account's records
/// reference that were never uploaded.
pub async fn list_missing_blobs(
    State(s): St,
    headers: HeaderMap,
    Query(p): Query<MissingParams>,
) -> XrpcResult<Json<Value>> {
    let authed = auth::authorization(&s, &headers, auth::STANDARD, &[], auth::any_scope).await?;
    let limit = p.limit.unwrap_or(500);
    if !(1..=1000).contains(&limit) {
        return Err(XrpcError::invalid_request(
            "limit must be between 1 and 1000",
        ));
    }
    let rows = s
        .store
        .list_missing_blobs(&authed.did, p.cursor.as_deref(), limit)
        .await?;
    let cursor = rows.last().map(|r| r.0.clone());
    let blobs: Vec<Value> = rows
        .into_iter()
        .map(|(cid, collection, rkey)| {
            json!({
                "cid": cid,
                "recordUri": format!("at://{}/{collection}/{rkey}", authed.did),
            })
        })
        .collect();
    let mut out = json!({ "blobs": blobs });
    if let Some(c) = cursor {
        out["cursor"] = json!(c);
    }
    Ok(Json(out))
}

/// `app.nearhorizon.locus.account.getImportStatus`: the account's latest
/// import job.
pub async fn get_import_status(State(s): St, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    let pds = format!("{}#atproto_pds", s.service_did);
    let authed = auth::authorization(&s, &headers, auth::STANDARD, &[], |p| {
        Ok(p.assert_rpc(locus_core::locus_nsid!("account.getImportStatus"), &pds)?)
    })
    .await?;
    import_status(&s, &authed.did).await.map(Json)
}

/// An account's latest import job, as `getImportStatus` reports it.
pub async fn import_status(s: &AppState, did: &str) -> XrpcResult<Value> {
    let Some(st) = s.store.latest_import(did).await? else {
        return Ok(json!({ "import": null }));
    };
    let mut out = json!({
        "id": st.id,
        "phase": st.phase,
        "done": matches!(st.phase.as_str(), "done" | "failed"),
        "progress": { "records": st.progress_done, "total": st.progress_total },
        "createdAt": locus_core::time::iso8601(st.created_at),
        "updatedAt": locus_core::time::iso8601(st.updated_at),
    });
    if let Some(e) = st.error {
        out["error"] = json!(e);
    }
    if let Some(n) = st.notes {
        out["notes"] = json!(n);
    }
    Ok(json!({ "import": out }))
}
