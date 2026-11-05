//! `com.atproto.repo.*`: record writes and reads, `uploadBlob`, and
//! `com.atproto.sync.getBlob`.

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use locus_repo::records::{self, PreparedRecord};
use locus_repo::{Action, CommitError, CommitOutcome, CommitRequest, PreparedWrite};
use locus_store::accounts::{Account, Status};
use proto_blue_lex_data::Cid;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::auth;
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

type St = State<Arc<AppState>>;

/// Most writes one `applyWrites` may carry, as the reference.
const MAX_WRITES: usize = 200;

fn commit_error(e: CommitError) -> XrpcError {
    match e {
        CommitError::InvalidSwap(m) => XrpcError::named("InvalidSwap", m),
        CommitError::InvalidRequest(m) => XrpcError::invalid_request(m),
        CommitError::RepoNotFound(did) => {
            XrpcError::invalid_request(format!("Could not find repo: {did}"))
        }
        CommitError::WriteBlocked => crate::account::write_blocked(),
        CommitError::Blob { name, message } => XrpcError::named(name, message),
        CommitError::Unchanged => XrpcError::invalid_request("no changes"),
        CommitError::InvalidMst(m) => {
            tracing::error!("commit refused by the MST check: {m}");
            XrpcError::internal("Internal Server Error")
        }
        CommitError::Core(e) => e.into(),
    }
}

fn record_limit(s: &AppState) -> usize {
    usize::try_from(s.snapshot().int("limits.record_size")).unwrap_or(usize::MAX)
}

fn invalid_record(e: records::InvalidRecord) -> XrpcError {
    XrpcError::invalid_request(e.0)
}

fn parse_cid(s: &str, what: &str) -> XrpcResult<Cid> {
    Cid::from_str_multibase(s)
        .map_err(|_| XrpcError::invalid_request(format!("Invalid {what}: {s}")))
}

/// The local account named by `repo` (a DID or handle).
async fn local_account(s: &AppState, repo: &str) -> XrpcResult<Option<Account>> {
    Ok(if repo.starts_with("did:") {
        s.store.account_by_did(repo).await?
    } else {
        s.store
            .account_by_handle(&repo.to_ascii_lowercase())
            .await?
    }
    .filter(|a| a.status != Status::Deleted))
}

/// A write's caller and target, as the reference: the token's account must
/// be `repo`, and active (not deactivated or taken down).
async fn writer(s: &AppState, headers: &HeaderMap, repo: &str) -> XrpcResult<Account> {
    let authed = auth::access(s, headers).await?;
    let account = local_account(s, repo)
        .await?
        .ok_or_else(|| XrpcError::invalid_request(format!("Could not find repo: {repo}")))?;
    let account = auth::account_checked(s, &account.did, true).await?;
    if account.did != authed.did {
        return Err(XrpcError::auth_required("Authentication Required"));
    }
    Ok(account)
}

fn prepared(
    action: Action,
    collection: &str,
    rkey: &str,
    rec: Option<&PreparedRecord>,
    swap: Option<Option<Cid>>,
) -> PreparedWrite {
    PreparedWrite {
        action,
        collection: collection.to_owned(),
        rkey: rkey.to_owned(),
        record: rec.map(|r| (r.cid.clone(), r.bytes.clone())),
        swap_record: swap,
        blobs: rec.map(|r| r.blobs.clone()).unwrap_or_default(),
        backlinks: rec
            .map(|r| records::backlinks(collection, &r.value))
            .unwrap_or_default(),
    }
}

fn check_collection(collection: &str) -> XrpcResult<()> {
    proto_blue_syntax::Nsid::new(collection)
        .map(|_| ())
        .map_err(|_| XrpcError::invalid_request(format!("Invalid collection: {collection}")))
}

async fn commit(
    s: &AppState,
    account: &Account,
    writes: Vec<PreparedWrite>,
    swap_commit: Option<&str>,
    dedupe: bool,
) -> XrpcResult<CommitOutcome> {
    let swap_commit = swap_commit
        .map(|c| parse_cid(c, "swapCommit"))
        .transpose()?;
    let key = s.signing_key(&account.did).await?;
    let outcome = s
        .repo
        .commit(CommitRequest {
            did: &account.did,
            writes,
            swap_commit,
            signer: &key,
            dedupe_backlinks: dedupe,
        })
        .await
        .map_err(commit_error)?;
    finish_moves(s, &outcome.moves).await;
    Ok(outcome)
}

/// Copies promoted blobs to their permanent keys now; a failure is left to
/// the mover job.
pub(crate) async fn finish_moves(s: &AppState, moves: &[locus_store::blob::PendingMove]) {
    for m in moves {
        if let Err(e) = locus_blob::finish_move(&s.store, &s.blobs, m).await {
            tracing::warn!(
                did = m.did,
                cid = m.cid,
                "blob move failed, will retry: {e}"
            );
        }
    }
}

fn commit_json(o: &CommitOutcome) -> Value {
    json!({ "cid": o.commit_cid.to_string_base32(), "rev": o.rev })
}

fn uri(did: &str, collection: &str, rkey: &str) -> String {
    format!("at://{did}/{collection}/{rkey}")
}

fn with_status(mut v: Value, status: Option<records::ValidationStatus>) -> Value {
    if let Some(st) = status {
        v["validationStatus"] = json!(st.as_str());
    }
    v
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRecordInput {
    repo: String,
    collection: String,
    rkey: Option<String>,
    validate: Option<bool>,
    record: Value,
    swap_commit: Option<String>,
}

pub async fn create_record(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<CreateRecordInput>,
) -> XrpcResult<Json<Value>> {
    let account = writer(&s, &headers, &input.repo).await?;
    check_collection(&input.collection)?;
    let rkey = input.rkey.clone().unwrap_or_else(|| s.repo.tids().next());
    let rec = records::prepare_record(
        &input.collection,
        &rkey,
        records::parse_json(&input.record).map_err(invalid_record)?,
        input.validate,
        record_limit(&s),
    )
    .map_err(invalid_record)?;
    let write = prepared(Action::Create, &input.collection, &rkey, Some(&rec), None);
    let dedupe = input.validate != Some(false);
    let outcome = commit(
        &s,
        &account,
        vec![write],
        input.swap_commit.as_deref(),
        dedupe,
    )
    .await?;
    Ok(Json(with_status(
        json!({
            "uri": uri(&account.did, &input.collection, &rkey),
            "cid": rec.cid.to_string_base32(),
            "commit": commit_json(&outcome),
        }),
        rec.status,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutRecordInput {
    repo: String,
    collection: String,
    rkey: String,
    validate: Option<bool>,
    record: Value,
    swap_record: Option<Value>,
    swap_commit: Option<String>,
}

/// `swapRecord` as the reference reads it: absent = no check, `null` = the
/// path must be empty, a CID = the path must hold it.
fn swap_record(v: Option<&Value>) -> XrpcResult<Option<Option<Cid>>> {
    match v {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(c)) => Ok(Some(Some(parse_cid(c, "swapRecord")?))),
        Some(_) => Err(XrpcError::invalid_request("Invalid swapRecord")),
    }
}

pub async fn put_record(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<PutRecordInput>,
) -> XrpcResult<Json<Value>> {
    let account = writer(&s, &headers, &input.repo).await?;
    check_collection(&input.collection)?;
    let rec = records::prepare_record(
        &input.collection,
        &input.rkey,
        records::parse_json(&input.record).map_err(invalid_record)?,
        input.validate,
        record_limit(&s),
    )
    .map_err(invalid_record)?;
    let swap = swap_record(input.swap_record.as_ref())?;
    let write = prepared(
        Action::Put,
        &input.collection,
        &input.rkey,
        Some(&rec),
        swap,
    );
    let mut out = json!({
        "uri": uri(&account.did, &input.collection, &input.rkey),
        "cid": rec.cid.to_string_base32(),
    });
    // Putting the record the path already holds commits nothing.
    match commit(
        &s,
        &account,
        vec![write],
        input.swap_commit.as_deref(),
        false,
    )
    .await
    {
        Ok(outcome) => out["commit"] = commit_json(&outcome),
        Err(e) if e.message == "no changes" => {}
        Err(e) => return Err(e),
    }
    Ok(Json(with_status(out, rec.status)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteRecordInput {
    repo: String,
    collection: String,
    rkey: String,
    swap_record: Option<String>,
    swap_commit: Option<String>,
}

pub async fn delete_record(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<DeleteRecordInput>,
) -> XrpcResult<Json<Value>> {
    let account = writer(&s, &headers, &input.repo).await?;
    check_collection(&input.collection)?;
    let swap = input
        .swap_record
        .as_deref()
        .map(|c| parse_cid(c, "swapRecord").map(Some))
        .transpose()?;
    let write = prepared(
        Action::DeleteIfPresent,
        &input.collection,
        &input.rkey,
        None,
        swap,
    );
    // Deleting a missing record commits nothing, as the reference.
    match commit(
        &s,
        &account,
        vec![write],
        input.swap_commit.as_deref(),
        false,
    )
    .await
    {
        Ok(outcome) => Ok(Json(json!({ "commit": commit_json(&outcome) }))),
        Err(e) if e.message == "no changes" => Ok(Json(json!({}))),
        Err(e) => Err(e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyWritesInput {
    repo: String,
    validate: Option<bool>,
    writes: Vec<Value>,
    swap_commit: Option<String>,
}

pub async fn apply_writes(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<ApplyWritesInput>,
) -> XrpcResult<Json<Value>> {
    let account = writer(&s, &headers, &input.repo).await?;
    if input.writes.len() > MAX_WRITES {
        return Err(XrpcError::invalid_request(format!(
            "Too many writes. Max: {MAX_WRITES}"
        )));
    }
    let mut writes = Vec::with_capacity(input.writes.len());
    let mut results = Vec::with_capacity(input.writes.len());
    for (i, w) in input.writes.iter().enumerate() {
        let kind = w["$type"].as_str().unwrap_or_default();
        let collection = w["collection"].as_str().ok_or_else(|| {
            XrpcError::invalid_request(format!("writes[{i}]: missing collection"))
        })?;
        check_collection(collection)?;
        let rkey_given = w["rkey"].as_str();
        match kind {
            "com.atproto.repo.applyWrites#create" | "com.atproto.repo.applyWrites#update" => {
                let create = kind.ends_with("#create");
                let rkey = match rkey_given {
                    Some(r) => r.to_owned(),
                    None if create => s.repo.tids().next(),
                    None => {
                        return Err(XrpcError::invalid_request(format!(
                            "writes[{i}]: missing rkey"
                        )));
                    }
                };
                let value = records::parse_json(&w["value"]).map_err(invalid_record)?;
                let rec = records::prepare_record(
                    collection,
                    &rkey,
                    value,
                    input.validate,
                    record_limit(&s),
                )
                .map_err(invalid_record)?;
                let action = if create {
                    Action::Create
                } else {
                    Action::Update
                };
                writes.push(prepared(action, collection, &rkey, Some(&rec), None));
                results.push(with_status(
                    json!({
                        "$type": if create { "com.atproto.repo.applyWrites#createResult" } else { "com.atproto.repo.applyWrites#updateResult" },
                        "uri": uri(&account.did, collection, &rkey),
                        "cid": rec.cid.to_string_base32(),
                    }),
                    rec.status,
                ));
            }
            "com.atproto.repo.applyWrites#delete" => {
                let rkey = rkey_given.ok_or_else(|| {
                    XrpcError::invalid_request(format!("writes[{i}]: missing rkey"))
                })?;
                writes.push(prepared(Action::Delete, collection, rkey, None, None));
                results.push(json!({ "$type": "com.atproto.repo.applyWrites#deleteResult" }));
            }
            other => {
                return Err(XrpcError::invalid_request(format!(
                    "writes[{i}]: unknown write type {other:?}"
                )));
            }
        }
    }
    let outcome = commit(&s, &account, writes, input.swap_commit.as_deref(), false).await?;
    Ok(Json(
        json!({ "commit": commit_json(&outcome), "results": results }),
    ))
}

/// A record's JSON value from its stored block.
async fn record_value(s: &AppState, did: &str, cid: &str) -> XrpcResult<Value> {
    let bytes = s
        .store
        .block(did, cid)
        .await?
        .ok_or_else(|| XrpcError::internal("a record's block is missing"))?;
    let value =
        proto_blue_lex_cbor::decode(&bytes).map_err(|e| XrpcError::internal(e.to_string()))?;
    Ok(proto_blue_lex_json::lex_to_json(&value))
}

/// The active local account for reads (an inactive one reads as absent;
/// the reference passes those reads on to the AppView).
async fn readable_account(s: &AppState, repo: &str) -> XrpcResult<Option<Account>> {
    Ok(local_account(s, repo)
        .await?
        .filter(|a| a.status == Status::Active))
}

#[derive(Deserialize)]
pub struct GetRecordParams {
    repo: String,
    collection: String,
    rkey: String,
    cid: Option<String>,
}

/// A repo hosted here is answered locally; any other goes to the AppView,
/// as the reference.
pub async fn get_record(
    State(s): St,
    uri_in: axum::http::Uri,
    headers: HeaderMap,
    Query(p): Query<GetRecordParams>,
) -> Response {
    match readable_account(&s, &p.repo).await {
        Ok(Some(account)) => local_record(&s, &account, &p).await.into_response(),
        Ok(None) => remote_record(&s, &uri_in, &headers)
            .await
            .unwrap_or_else(|r| r),
        Err(e) => e.into_response(),
    }
}

async fn remote_record(
    s: &AppState,
    uri_in: &axum::http::Uri,
    headers: &HeaderMap,
) -> Result<Response, Response> {
    use crate::pipethrough as pt;
    if pt::appview(s).is_none() {
        return Err(XrpcError::invalid_request("Could not locate record").into_response());
    }
    let lxm = "com.atproto.repo.getRecord";
    let target = pt::target(s, headers, lxm)
        .await
        .map_err(IntoResponse::into_response)?;
    let upstream = pt::send(
        s,
        pt::Outgoing {
            method: axum::http::Method::GET,
            lxm,
            path_and_query: uri_in
                .path_and_query()
                .map_or(uri_in.path(), |q| q.as_str()),
            headers,
            body: None,
            iss: None,
            aud: None,
            target: &target,
            accept_encoding: headers
                .get(header::ACCEPT_ENCODING)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
            strict: false,
        },
    )
    .await?;
    Ok(pt::stream_back(s, upstream))
}

async fn local_record(
    s: &AppState,
    account: &Account,
    p: &GetRecordParams,
) -> XrpcResult<Json<Value>> {
    let at = uri(&account.did, &p.collection, &p.rkey);
    let not_found = || XrpcError::named("RecordNotFound", format!("Could not locate record: {at}"));
    let Some((cid, takedown)) = s
        .store
        .record_entry(&account.did, &p.collection, &p.rkey)
        .await?
    else {
        return Err(not_found());
    };
    if takedown.is_some() || p.cid.as_deref().is_some_and(|c| c != cid) {
        return Err(not_found());
    }
    let value = record_value(s, &account.did, &cid).await?;
    Ok(Json(json!({ "uri": at, "cid": cid, "value": value })))
}

#[derive(Deserialize)]
pub struct ListRecordsParams {
    repo: String,
    collection: String,
    limit: Option<i64>,
    cursor: Option<String>,
    #[serde(default)]
    reverse: bool,
}

pub async fn list_records(
    State(s): St,
    Query(p): Query<ListRecordsParams>,
) -> XrpcResult<Json<Value>> {
    let account = readable_account(&s, &p.repo)
        .await?
        .ok_or_else(|| XrpcError::invalid_request(format!("Could not find repo: {}", p.repo)))?;
    let limit = p.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(XrpcError::invalid_request(
            "limit must be between 1 and 100",
        ));
    }
    let rows = s
        .store
        .list_records(
            &account.did,
            &p.collection,
            limit,
            p.cursor.as_deref(),
            p.reverse,
        )
        .await?;
    let mut records = Vec::with_capacity(rows.len());
    for (rkey, cid) in &rows {
        records.push(json!({
            "uri": uri(&account.did, &p.collection, rkey),
            "cid": cid,
            "value": record_value(&s, &account.did, cid).await?,
        }));
    }
    let mut out = json!({ "records": records });
    if let Some((rkey, _)) = rows.last() {
        out["cursor"] = json!(rkey);
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct DescribeRepoParams {
    repo: String,
}

pub async fn describe_repo(
    State(s): St,
    Query(p): Query<DescribeRepoParams>,
) -> XrpcResult<Json<Value>> {
    let account = local_account(&s, &p.repo).await?.ok_or_else(|| {
        XrpcError::named(
            "RepoNotFound",
            format!("Could not find repo for DID: {}", p.repo),
        )
    })?;
    unavailable(&account)?;
    let doc = s
        .resolve_did_doc(&account.did, false)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| XrpcError::invalid_request("Could not resolve DID"))?;
    let handle_is_correct = proto_blue_common::get_handle(&doc) == Some(account.handle.as_str());
    Ok(Json(json!({
        "handle": account.handle,
        "did": account.did,
        "didDoc": locus_identity::resolve::doc_to_json(&doc),
        "collections": s.store.collections(&account.did).await?,
        "handleIsCorrect": handle_is_correct,
    })))
}

/// The reference's `assertRepoAvailability` refusals for callers other
/// than the owner.
pub(crate) fn unavailable(account: &Account) -> XrpcResult<()> {
    match account.status {
        Status::Active => Ok(()),
        Status::Takendown => Err(XrpcError::named(
            "RepoTakendown",
            format!("Repo has been takendown: {}", account.did),
        )),
        Status::Suspended => Err(XrpcError::named(
            "RepoSuspended",
            format!("Repo has been suspended: {}", account.did),
        )),
        Status::Deactivated => Err(XrpcError::named(
            "RepoDeactivated",
            format!("Repo has been deactivated: {}", account.did),
        )),
        Status::Deleted => Err(XrpcError::named(
            "RepoNotFound",
            format!("Could not find repo for DID: {}", account.did),
        )),
    }
}

pub async fn upload_blob(State(s): St, headers: HeaderMap, body: Body) -> XrpcResult<Json<Value>> {
    // As the reference: user credentials, or service auth from the user.
    let did = if headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            locus_auth::service_jwt::parse(v.trim_start_matches("Bearer ").trim(), None, 0).is_ok()
        }) {
        auth::user_service_auth(&s, &headers, "com.atproto.repo.uploadBlob").await?
    } else {
        auth::access(&s, &headers).await?.did
    };
    let account = auth::account_checked(&s, &did, true).await?;
    let limit = u64::try_from(s.snapshot().int("blobs.upload_limit")).unwrap_or(u64::MAX);
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    // Bytes first, to a temp key; no transaction is open meanwhile.
    let temp = locus_store::blob::temp_key();
    let written = s
        .blobs
        .write_stream(&temp, body.into_data_stream(), limit)
        .await
        .map_err(|e| match e {
            locus_blob::BlobError::TooLarge { .. } => XrpcError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "BlobTooLarge",
                "This file is too large",
            ),
            other => XrpcError::internal(other.to_string()),
        })?;
    let cid = written.cid.to_string_base32();
    let mime = locus_blob::mime_type(&written.head, content_type.as_deref());
    let size = i64::try_from(written.size).unwrap_or(i64::MAX);

    // Metadata under the repo lock (serialized with commits for this account).
    let result: XrpcResult<Vec<locus_store::blob::PendingMove>> = async {
        let (_lock, mut tx) = s.repo.locked_tx(&account.did).await.map_err(commit_error)?;
        let backend = tx.blob_backend().await?;
        if backend.paused {
            tx.queue_blob_delete(&temp, backend.epoch).await?;
            tx.commit().await?;
            return Err(XrpcError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "TemporarilyUnavailable",
                "Blob storage is being moved; try again shortly",
            ));
        }
        let fresh = match tx.blob_for_update(&account.did, &cid).await? {
            None => {
                tx.insert_temp_blob(&account.did, &cid, &mime, size, &temp)
                    .await?;
                true
            }
            Some(row) if row.takedown_ref.is_some() || row.state == "quarantined" => {
                tx.queue_blob_delete(&temp, backend.epoch).await?;
                tx.commit().await?;
                return Err(XrpcError::invalid_request(
                    "Blob has been takendown, cannot re-upload",
                ));
            }
            Some(row) if row.state == "temp" => {
                if let Some(old) = tx.repoint_temp_blob(&account.did, &cid, &temp).await? {
                    tx.queue_blob_delete(&old, backend.epoch).await?;
                }
                true
            }
            Some(_) => {
                // Already permanent: keep its copy, drop this one.
                tx.queue_blob_delete(&temp, backend.epoch).await?;
                tx.restart_unreferenced(&account.did, &cid).await?;
                false
            }
        };
        // Already referenced (records imported before their blobs, or an
        // active import's new tree): permanent at once.
        let mut moves = Vec::new();
        if fresh {
            let import = tx.active_import_for_blob(&account.did, &cid).await?;
            if import.is_some() || tx.blob_has_records(&account.did, &cid).await? {
                let key = locus_store::blob::permanent_key(&account.did, &cid);
                if let Some(r) = tx
                    .reference_blob(&account.did, &cid, &key, backend.epoch)
                    .await?
                    && r.pending_move.as_deref() == Some(key.as_str())
                {
                    moves.push(locus_store::blob::PendingMove {
                        did: account.did.clone(),
                        cid: cid.clone(),
                        from: r.storage_key,
                        to: key,
                    });
                }
                if let Some(job) = import {
                    tx.mark_import_blob(&job, &cid, true).await?;
                }
            }
        }
        tx.commit().await?;
        Ok(moves)
    }
    .await;
    if let Err(e) = &result {
        tracing::debug!("upload metadata refused: {}", e.message);
    }
    finish_moves(&s, &result?).await;
    Ok(Json(json!({
        "blob": {
            "$type": "blob",
            "ref": { "$link": cid },
            "mimeType": mime,
            "size": size,
        }
    })))
}

#[derive(Deserialize)]
pub struct GetBlobParams {
    did: String,
    cid: String,
}

pub async fn get_blob(
    State(s): St,
    headers: HeaderMap,
    Query(p): Query<GetBlobParams>,
) -> XrpcResult<Response> {
    serve_blob(&s, &headers, &p.did, &p.cid).await
}

/// The `/blob/<did>/<cid>` alias.
pub async fn get_blob_path(
    State(s): St,
    headers: HeaderMap,
    Path((did, cid)): Path<(String, String)>,
) -> XrpcResult<Response> {
    serve_blob(&s, &headers, &did, &cid).await
}

async fn serve_blob(
    s: &AppState,
    headers: &HeaderMap,
    did: &str,
    cid: &str,
) -> XrpcResult<Response> {
    let account = s
        .store
        .account_by_did(did)
        .await?
        .filter(|a| a.status != Status::Deleted)
        .ok_or_else(|| {
            XrpcError::named(
                "RepoNotFound",
                format!("Could not find repo for DID: {did}"),
            )
        })?;
    // The owner bypasses the status refusals, as the reference.
    let owner = auth::access(s, headers)
        .await
        .is_ok_and(|a| a.did == account.did);
    if !owner {
        unavailable(&account)?;
    }
    let not_found = || XrpcError::invalid_request("Blob not found");
    let row = s.store.blob(did, cid).await?.ok_or_else(not_found)?;
    if row.takedown_ref.is_some() || row.state == "quarantined" {
        return Err(not_found());
    }
    let mut file = s
        .blobs
        .open(&row.storage_key)
        .await
        .map_err(|e| XrpcError::internal(e.to_string()))?
        .ok_or_else(not_found)?;
    let size = u64::try_from(row.size).unwrap_or(0);
    let (start, end) = match headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|r| parse_range(r, size))
    {
        Some(r) => r,
        None => (0, size.saturating_sub(1)),
    };
    let partial = start != 0 || end + 1 != size;
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(|e| XrpcError::internal(e.to_string()))?;
    let len = if size == 0 { 0 } else { end - start + 1 };
    let stream = tokio_util::io::ReaderStream::new(file.take(len));
    let mut res = Response::new(Body::from_stream(stream));
    *res.status_mut() = if partial {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let h = res.headers_mut();
    let hv = |v: &str| HeaderValue::from_str(v).unwrap_or_else(|_| HeaderValue::from_static(""));
    h.insert(header::CONTENT_TYPE, hv(&row.mime_type));
    h.insert(header::CONTENT_LENGTH, hv(&len.to_string()));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        hv(&format!("attachment; filename=\"{cid}\"")),
    );
    if partial {
        h.insert(
            header::CONTENT_RANGE,
            hv(&format!("bytes {start}-{end}/{size}")),
        );
    }
    Ok(res.into_response())
}

/// A single `bytes=` range within `size`, as (first, last) inclusive.
fn parse_range(header: &str, size: u64) -> Option<(u64, u64)> {
    let spec = header.strip_prefix("bytes=")?;
    if spec.contains(',') || size == 0 {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", suffix) => {
            let n: u64 = suffix.parse().ok()?;
            (size.saturating_sub(n), size - 1)
        }
        (from, "") => (from.parse().ok()?, size - 1),
        (from, to) => (from.parse().ok()?, to.parse::<u64>().ok()?.min(size - 1)),
    };
    (start <= end && end < size).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::parse_range;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-9", 100), Some((0, 9)));
        assert_eq!(parse_range("bytes=90-", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=50-500", 100), Some((50, 99)));
        assert_eq!(parse_range("bytes=200-300", 100), None);
        assert_eq!(parse_range("bytes=0-1,5-6", 100), None);
        assert_eq!(parse_range("items=0-1", 100), None);
    }
}
