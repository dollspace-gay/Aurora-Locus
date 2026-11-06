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

use crate::auth;
use crate::error::{XrpcError, XrpcResult};
use crate::kryphocron as kr;
use crate::state::AppState;
use locus_kryphocron::nsid::AUDIENCE;

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

pub(crate) fn record_limit(s: &AppState) -> usize {
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
/// What an OAuth token must allow for a write (legacy tokens skip it).
pub(crate) type RepoCheck<'a> = &'a (
        dyn Fn(&locus_auth::scopes::Permissions) -> Result<(), locus_auth::scopes::ScopeMissing>
            + Sync
    );

pub(crate) async fn writer(
    s: &AppState,
    headers: &HeaderMap,
    repo: &str,
    check: RepoCheck<'_>,
) -> XrpcResult<Account> {
    let authed = auth::authorization(s, headers, auth::STANDARD, &[], auth::any_scope).await?;
    auth::require(&authed, check)?;
    let account = local_account(s, repo)
        .await?
        .ok_or_else(|| XrpcError::invalid_request(format!("Could not find repo: {repo}")))?;
    let account = auth::account_checked(s, &account.did, true).await?;
    if account.did != authed.did {
        return Err(XrpcError::auth_required("Authentication Required"));
    }
    Ok(account)
}

pub(crate) fn prepared(
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
        lenient_blobs: false,
    }
}

/// A create or update's record after kryphocron's step (host rules, gates,
/// the private-post pipeline); other collections pass through.
async fn kr_value(
    s: &Arc<AppState>,
    did: &str,
    collection: &str,
    rkey: &str,
    value: proto_blue_lex_data::LexValue,
) -> XrpcResult<proto_blue_lex_data::LexValue> {
    Ok(
        kr::prepare_write(s, did, collection, rkey, Some(value), kr::Op::Write)
            .await?
            .expect("a write keeps its record"),
    )
}

fn check_collection(collection: &str) -> XrpcResult<()> {
    proto_blue_syntax::Nsid::new(collection)
        .map(|_| ())
        .map_err(|_| XrpcError::invalid_request(format!("Invalid collection: {collection}")))
}

pub(crate) async fn commit(
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

/// Moves a blob's bytes to the store its state now calls for (after a
/// takedown, a scanner match or a release); a failure is left to the
/// placement job.
pub(crate) async fn place_blob(s: &AppState, did: &str, cid: &str) {
    let started = async {
        let mut tx = s.store.begin().await?;
        let epoch = tx.blob_backend().await?.epoch;
        let m = tx
            .place_blob(did, cid, crate::media_gate::gate_on(s), epoch)
            .await?;
        tx.commit().await?;
        Ok::<_, locus_core::CoreError>(m)
    }
    .await;
    match started {
        Ok(Some(m)) => finish_moves(s, &[m]).await,
        Ok(None) => {}
        Err(e) => tracing::warn!(did, cid, "blob placement failed, will retry: {e}"),
    }
}

fn commit_json(o: &CommitOutcome) -> Value {
    json!({ "cid": o.commit_cid.to_string_base32(), "rev": o.rev })
}

pub(crate) fn uri(did: &str, collection: &str, rkey: &str) -> String {
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
    let account = writer(&s, &headers, &input.repo, &|p| {
        p.assert_repo(&input.collection, "create")
    })
    .await?;
    crate::ratelimit::limit_repo_write(&s, &account.did, 3).await?;
    check_collection(&input.collection)?;
    if input.collection == AUDIENCE {
        // Audiences live in the private store: no commit.
        let w = crate::audience::generic_create(
            &s,
            &headers,
            &account.did,
            input.rkey.as_deref(),
            &input.record,
        )
        .await?;
        return Ok(Json(json!({"uri": w.uri, "cid": w.cid})));
    }
    let rkey = input.rkey.clone().unwrap_or_else(|| s.repo.tids().next());
    let value = kr_value(
        &s,
        &account.did,
        &input.collection,
        &rkey,
        records::parse_json(&input.record).map_err(invalid_record)?,
    )
    .await?;
    let rec = records::prepare_record(
        &input.collection,
        &rkey,
        value,
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
    let account = writer(&s, &headers, &input.repo, &|p| {
        p.assert_repo(&input.collection, "create")?;
        p.assert_repo(&input.collection, "update")
    })
    .await?;
    crate::ratelimit::limit_repo_write(&s, &account.did, 2).await?;
    check_collection(&input.collection)?;
    if input.collection == AUDIENCE {
        let swap = input.swap_record.as_ref().and_then(Value::as_str);
        let w = crate::audience::update(&s, &account.did, &input.rkey, &input.record, swap).await?;
        return Ok(Json(json!({"uri": w.uri, "cid": w.cid})));
    }
    let value = kr_value(
        &s,
        &account.did,
        &input.collection,
        &input.rkey,
        records::parse_json(&input.record).map_err(invalid_record)?,
    )
    .await?;
    let rec = records::prepare_record(
        &input.collection,
        &input.rkey,
        value,
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
    let account = writer(&s, &headers, &input.repo, &|p| {
        p.assert_repo(&input.collection, "delete")
    })
    .await?;
    crate::ratelimit::limit_repo_write(&s, &account.did, 1).await?;
    check_collection(&input.collection)?;
    if input.collection == AUDIENCE {
        crate::audience::delete(&s, &account.did, &input.rkey).await?;
        return Ok(Json(json!({})));
    }
    kr::prepare_write(
        &s,
        &account.did,
        &input.collection,
        &input.rkey,
        None,
        kr::Op::Delete,
    )
    .await?;
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

/// An `applyWrites` with an audience op: it stands alone **[deviation]**,
/// since a prism write can't roll back with a database transaction; no
/// commit, so the output has no `commit`.
async fn audience_write(
    s: &Arc<AppState>,
    headers: &HeaderMap,
    did: &str,
    input: &ApplyWritesInput,
) -> XrpcResult<Json<Value>> {
    let [w] = input.writes.as_slice() else {
        return Err(XrpcError::invalid_request(
            "An audience write must be the only write in applyWrites",
        ));
    };
    if input.swap_commit.is_some() {
        return Err(XrpcError::invalid_request(
            "swapCommit doesn't apply to audience writes",
        ));
    }
    let rkey = w["rkey"].as_str();
    let result = match w["$type"].as_str().unwrap_or_default() {
        "com.atproto.repo.applyWrites#create" => {
            let out = crate::audience::generic_create(s, headers, did, rkey, &w["value"]).await?;
            json!({"$type": "com.atproto.repo.applyWrites#createResult", "uri": out.uri, "cid": out.cid})
        }
        "com.atproto.repo.applyWrites#update" => {
            let rkey = rkey.ok_or_else(|| XrpcError::invalid_request("writes[0]: missing rkey"))?;
            let out = crate::audience::update(s, did, rkey, &w["value"], None).await?;
            json!({"$type": "com.atproto.repo.applyWrites#updateResult", "uri": out.uri, "cid": out.cid})
        }
        "com.atproto.repo.applyWrites#delete" => {
            let rkey = rkey.ok_or_else(|| XrpcError::invalid_request("writes[0]: missing rkey"))?;
            crate::audience::delete(s, did, rkey).await?;
            json!({"$type": "com.atproto.repo.applyWrites#deleteResult"})
        }
        other => {
            return Err(XrpcError::invalid_request(format!(
                "writes[0]: unknown write type {other:?}"
            )));
        }
    };
    Ok(Json(json!({"results": [result]})))
}

pub async fn apply_writes(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<ApplyWritesInput>,
) -> XrpcResult<Json<Value>> {
    let account = writer(&s, &headers, &input.repo, &|p| {
        for w in &input.writes {
            let action = match w["$type"].as_str() {
                Some("com.atproto.repo.applyWrites#create") => "create",
                Some("com.atproto.repo.applyWrites#update") => "update",
                Some("com.atproto.repo.applyWrites#delete") => "delete",
                _ => continue,
            };
            p.assert_repo(w["collection"].as_str().unwrap_or(""), action)?;
        }
        Ok(())
    })
    .await?;
    // As the reference: creates 3 points, updates 2, deletes 1.
    let points: i64 = input
        .writes
        .iter()
        .map(|w| match w["$type"].as_str() {
            Some("com.atproto.repo.applyWrites#create") => 3,
            Some("com.atproto.repo.applyWrites#update") => 2,
            Some("com.atproto.repo.applyWrites#delete") => 1,
            _ => 0,
        })
        .sum();
    crate::ratelimit::limit_repo_write(&s, &account.did, points).await?;
    if input.writes.len() > MAX_WRITES {
        return Err(XrpcError::invalid_request(format!(
            "Too many writes. Max: {MAX_WRITES}"
        )));
    }
    if input.writes.iter().any(|w| w["collection"] == AUDIENCE) {
        return audience_write(&s, &headers, &account.did, &input).await;
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
                let value = kr_value(&s, &account.did, collection, &rkey, value).await?;
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
                kr::prepare_write(&s, &account.did, collection, rkey, None, kr::Op::Delete).await?;
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
        Ok(Some(account)) if p.collection == AUDIENCE => {
            crate::audience::owner_record(&s, &headers, &account.did, &p.rkey)
                .await
                .map(Json)
                .into_response()
        }
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
    headers: HeaderMap,
    Query(p): Query<ListRecordsParams>,
) -> XrpcResult<Json<Value>> {
    let account = readable_account(&s, &p.repo)
        .await?
        .ok_or_else(|| XrpcError::invalid_request(format!("Could not find repo: {}", p.repo)))?;
    if p.collection == AUDIENCE {
        return crate::audience::owner_list(&s, &headers, &account.did)
            .await
            .map(Json);
    }
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
        let mime = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| {
                v.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
            })
            .unwrap_or_default();
        auth::authorization(&s, &headers, auth::STANDARD, &[], |p| {
            Ok(p.assert_blob(&mime)?)
        })
        .await?
        .did
    };
    let account = auth::account_checked(&s, &did, true).await?;
    crate::ratelimit::limit_ip(&s, &[crate::ratelimit::buckets::UPLOAD_BLOB]).await?;
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
    locus_core::metrics::inc("locus_blob_bytes_total", &[], written.size);

    // Scanned in temp storage, before it can be used.
    use crate::scanner::Scan;
    let scan = crate::scanner::scan_key(&s, &account.did, &cid, &mime, &temp).await;
    let scan_status = match &scan {
        Scan::NotConfigured => "none",
        Scan::Allowed => "allowed",
        Scan::Matched { .. } => "matched",
        Scan::Unavailable(why) => {
            tracing::warn!("the blob scanner is unavailable: {why}");
            if s.snapshot().text("blobs.scan_unavailable") == "refuse" {
                let mut tx = s.store.begin().await?;
                let backend = tx.blob_backend().await?;
                tx.queue_blob_delete(&temp, backend.epoch).await?;
                tx.commit().await?;
                return Err(XrpcError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "TemporarilyUnavailable",
                    "The file couldn't be checked; try again shortly",
                ));
            }
            "unscanned"
        }
    };

    // Metadata under the repo lock (serialized with commits for this account).
    let mut newly_matched = false;
    let gate_on = crate::media_gate::gate_on(&s);
    let result: XrpcResult<Vec<locus_store::blob::PendingMove>> = async {
        let (_lock, mut tx) = s.repo.locked_tx(&account.did).await.map_err(commit_error)?;
        let backend = tx.blob_backend().await?;
        if backend.paused || backend.epoch != s.blobs.epoch() {
            // Paused for a cutover, or the bytes went to a backend that's
            // no longer active (this node reloads).
            tx.queue_blob_delete(&temp, s.blobs.epoch()).await?;
            tx.commit().await?;
            if backend.epoch != s.blobs.epoch() {
                crate::keys::reload_soon(&s);
            }
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
        if let Scan::Matched { .. } = &scan {
            // Kept (never collected) for review; the upload is refused.
            tx.quarantine_match(&account.did, &cid).await?;
            tx.commit().await?;
            newly_matched = true;
            return Err(XrpcError::invalid_request("This file can't be uploaded"));
        }
        if fresh && scan_status != "none" {
            tx.set_scan_status(&account.did, &cid, scan_status).await?;
        }
        // Already referenced (records imported before their blobs, or an
        // active import's new tree): permanent at once.
        let mut moves = Vec::new();
        if fresh {
            moves.extend(
                tx.promote_upload(&account.did, &cid, gate_on, backend.epoch)
                    .await?,
            );
        }
        tx.commit().await?;
        Ok(moves)
    }
    .await;
    if let Err(e) = &result {
        tracing::debug!("upload metadata refused: {}", e.message);
    }
    if let (true, Scan::Matched { result: r, reason }) = (newly_matched, &scan) {
        crate::scanner::record_match(&s, &account.did, &cid, "upload", r, reason.as_deref())
            .await?;
        // Its bytes go to quarantine.
        place_blob(&s, &account.did, &cid).await;
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
) -> Response {
    gated_blob(&s, &headers, &p.did, &p.cid).await
}

/// The `/blob/<did>/<cid>` alias.
pub async fn get_blob_path(
    State(s): St,
    headers: HeaderMap,
    Path((did, cid)): Path<(String, String)>,
) -> Response {
    gated_blob(&s, &headers, &did, &cid).await
}

/// A blob response, through the media gate: while it's on, a private-only
/// blob goes only to those who may read it, and its responses and every
/// refusal (missing blobs included) are private and never cached.
async fn gated_blob(s: &Arc<AppState>, headers: &HeaderMap, did: &str, cid: &str) -> Response {
    let gate = crate::media_gate::gate_on(s);
    let mut res = match serve_blob(s, headers, did, cid, gate).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    };
    if gate && (res.extensions().get::<GatedBlob>().is_some() || !res.status().is_success()) {
        crate::media_gate::no_store(&mut res);
        let outcome = if res.status().is_success() {
            "served"
        } else {
            "refused"
        };
        locus_core::metrics::inc("locus_gated_fetches_total", &[("outcome", outcome)], 1);
    }
    res
}

/// Marks a response as a gated blob's.
#[derive(Debug, Clone, Copy)]
struct GatedBlob;

async fn serve_blob(
    s: &Arc<AppState>,
    headers: &HeaderMap,
    did: &str,
    cid: &str,
    gate: bool,
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
    let owner = auth::authorization(s, headers, auth::STANDARD, &[], auth::any_scope)
        .await
        .is_ok_and(|a| a.did == account.did);
    if !owner && !crate::admin::admin_reader(s, headers).await {
        unavailable(&account)?;
    }
    let not_found = || XrpcError::invalid_request("Blob not found");
    let row = s.store.blob(did, cid).await?.ok_or_else(not_found)?;
    if row.takedown_ref.is_some() || row.state == "quarantined" {
        return Err(not_found());
    }
    let gated = gate && s.store.blob_private_only(did, cid).await?;
    if gated && !owner {
        let reader = crate::media_gate::reader(s, headers, "com.atproto.sync.getBlob").await;
        if !crate::media_gate::may_read(s, &reader, did, cid).await? {
            // The same answer as for a missing blob.
            return Err(not_found());
        }
    }
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
    let reader = match s
        .blobs
        .read(&row.storage_key, partial.then_some((start, end)))
        .await
        .map_err(|e| XrpcError::internal(e.to_string()))?
    {
        Some(r) => r,
        None => {
            // A node whose store predates a backend flip: it reloads, and
            // the client retries against the new backend.
            if s.store.blob_backend().await?.epoch != s.blobs.epoch() {
                crate::keys::reload_soon(s);
                return Err(XrpcError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "TemporarilyUnavailable",
                    "Blob storage moved; try again shortly",
                ));
            }
            return Err(not_found());
        }
    };
    let len = if size == 0 { 0 } else { end - start + 1 };
    if reader.len != len {
        return Err(XrpcError::internal(format!(
            "stored copy of {cid} has {} bytes where {len} were expected",
            reader.len
        )));
    }
    let mut res = Response::new(Body::from_stream(reader.body));
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
    if gated {
        res.extensions_mut().insert(GatedBlob);
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
