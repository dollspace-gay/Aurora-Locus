/// com.atproto.repo.* endpoints
use crate::{
    actor_store::{RepositoryManager, WriteOp},
    api::{labels::LabelView, middleware},
    context::AppContext,
    error::{PdsError, PdsResult},
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

/// Build repository routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        .route("/xrpc/com.atproto.repo.createRecord", post(create_record))
        .route("/xrpc/com.atproto.repo.putRecord", post(put_record))
        .route("/xrpc/com.atproto.repo.deleteRecord", post(delete_record))
        .route("/xrpc/com.atproto.repo.getRecord", get(get_record))
        .route("/xrpc/com.atproto.repo.listRecords", get(list_records))
        .route("/xrpc/com.atproto.repo.describeRepo", get(describe_repo))
        .route("/xrpc/com.atproto.repo.applyWrites", post(apply_writes))
}

/// Request to create a record
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRecordRequest {
    repo: String, // DID or handle
    collection: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    rkey: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    validate: Option<bool>,
    record: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    swap_commit: Option<String>,
}

/// Response from creating a record
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateRecordResponse {
    uri: String,
    cid: String,
}

/// Request to update a record
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutRecordRequest {
    repo: String,
    collection: String,
    rkey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    validate: Option<bool>,
    record: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    swap_record: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    swap_commit: Option<String>,
}

/// Response from updating a record
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PutRecordResponse {
    uri: String,
    cid: String,
}

/// Request to delete a record
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteRecordRequest {
    repo: String,
    collection: String,
    rkey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    swap_record: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    swap_commit: Option<String>,
}

/// Query parameters for getRecord
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetRecordQuery {
    repo: String,
    collection: String,
    rkey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cid: Option<String>,
}

/// Response from getting a record
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetRecordResponse {
    uri: String,
    cid: String,
    value: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<Vec<LabelView>>,
}

/// Query parameters for listRecords
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListRecordsQuery {
    repo: String,
    collection: String,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reverse: Option<bool>,
}

fn default_limit() -> i64 {
    50
}

/// Record entry in list response
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordEntry {
    uri: String,
    cid: String,
    value: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<Vec<LabelView>>,
}

/// Response from listing records
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListRecordsResponse {
    records: Vec<RecordEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
}

/// Query parameters for describeRepo
#[derive(Debug, Deserialize)]
struct DescribeRepoQuery {
    repo: String,
}

/// Response from describing a repo
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DescribeRepoResponse {
    did: String,
    handle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    did_doc: Option<serde_json::Value>,
    collections: Vec<String>,
    handle_is_correct: bool,
}

/// Request to apply writes
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyWritesRequest {
    repo: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    validate: Option<bool>,
    writes: Vec<WriteOp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    swap_commit: Option<String>,
}

/// Creates a proper signing function using the repository's stored private key
///
/// Uses PlcSigner with the repo_signing_key from configuration to sign repository commits
fn create_repo_signer(
    repo_key_hex: &str,
) -> impl Fn(&[u8; 32]) -> Result<Vec<u8>, atproto::repo::RepoError> + '_ {
    move |hash: &[u8; 32]| {
        let signer = crate::crypto::plc::PlcSigner::from_hex(repo_key_hex)
            .map_err(|e| atproto::repo::RepoError::Signing(format!("Failed to create signer: {}", e)))?;
        Ok(signer.sign(hash))
    }
}

/// Create a new record
async fn create_record(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<CreateRecordRequest>,
) -> PdsResult<Json<CreateRecordResponse>> {
    tracing::info!("create_record: Starting for collection: {}", req.collection);

    // Require authentication
    let session = middleware::require_auth(State(ctx.clone()), headers).await
        .map_err(|e| {
            tracing::error!("create_record: Auth failed: {}", e);
            e
        })?;
    tracing::debug!("create_record: Authenticated as DID: {}", session.did);

    // Verify repo matches authenticated user
    if req.repo != session.did {
        tracing::error!("create_record: Repo mismatch - req: {}, session: {}", req.repo, session.did);
        return Err(PdsError::Authorization(
            "Cannot create record in another user's repo".to_string(),
        ));
    }

    // Create repository manager with sequencer
    tracing::debug!("create_record: Creating repository manager with sequencer");
    let repo_mgr = RepositoryManager::with_sequencer(
        session.did.clone(),
        (*ctx.actor_store).clone(),
        ctx.sequencer.clone(),
    );

    // Create signer from repo key
    let signer = create_repo_signer(&ctx.config.authentication.repo_signing_key);

    // Create the record
    tracing::debug!("create_record: Calling repo_mgr.create_record");
    let (uri, cid, _rev) = repo_mgr
        .create_record(&req.collection, req.rkey.as_deref(), req.record, req.validate, signer)
        .await
        .map_err(|e| {
            tracing::error!("create_record: Failed to create record: {}", e);
            e
        })?;

    tracing::info!("create_record: Successfully created record - URI: {}, CID: {}", uri, cid);
    Ok(Json(CreateRecordResponse { uri, cid }))
}

/// Update an existing record (or create if doesn't exist)
async fn put_record(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<PutRecordRequest>,
) -> PdsResult<Json<PutRecordResponse>> {
    // Require authentication
    let session = middleware::require_auth(State(ctx.clone()), headers).await?;

    // Verify repo matches authenticated user
    if req.repo != session.did {
        return Err(PdsError::Authorization(
            "Cannot update record in another user's repo".to_string(),
        ));
    }

    // Create repository manager
    let repo_mgr = RepositoryManager::with_sequencer(session.did.clone(), (*ctx.actor_store).clone(), ctx.sequencer.clone());

    // Create signer from repo key
    let signer = create_repo_signer(&ctx.config.authentication.repo_signing_key);

    // Update the record
    let (cid, _rev) = repo_mgr
        .update_record(&req.collection, &req.rkey, req.record, req.validate, signer)
        .await?;

    let uri = format!("at://{}/{}/{}", session.did, req.collection, req.rkey);

    Ok(Json(PutRecordResponse { uri, cid }))
}

/// Delete a record
async fn delete_record(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<DeleteRecordRequest>,
) -> PdsResult<Json<serde_json::Value>> {
    // Require authentication
    let session = middleware::require_auth(State(ctx.clone()), headers).await?;

    // Verify repo matches authenticated user
    if req.repo != session.did {
        return Err(PdsError::Authorization(
            "Cannot delete record from another user's repo".to_string(),
        ));
    }

    // Create repository manager
    let repo_mgr = RepositoryManager::with_sequencer(session.did.clone(), (*ctx.actor_store).clone(), ctx.sequencer.clone());

    // Create signer from repo key
    let signer = create_repo_signer(&ctx.config.authentication.repo_signing_key);

    // Delete the record
    repo_mgr
        .delete_record(&req.collection, &req.rkey, signer)
        .await?;

    Ok(Json(serde_json::json!({})))
}

/// Get a record
async fn get_record(
    State(ctx): State<AppContext>,
    Query(query): Query<GetRecordQuery>,
) -> PdsResult<Json<GetRecordResponse>> {
    // Get the DID (could be handle resolution in the future)
    let did = &query.repo;

    // Create repository manager
    let repo_mgr = RepositoryManager::new(did.clone(), (*ctx.actor_store).clone());

    // Get the record
    let uri = format!("at://{}/{}/{}", did, query.collection, query.rkey);
    let record = repo_mgr.get_record(&uri).await?;

    match record {
        Some(value) => {
            // Extract CID from the returned value
            let cid = value
                .get("cid")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            let record_value = value.get("value").cloned().unwrap_or(serde_json::Value::Null);

            // Fetch labels for this record
            let labels = ctx.label_manager.get_labels(&uri).await
                .ok()
                .map(|lbls| lbls.into_iter().map(LabelView::from).collect());

            Ok(Json(GetRecordResponse {
                uri,
                cid,
                value: record_value,
                labels,
            }))
        }
        None => Err(PdsError::NotFound(format!("Record not found: {}", uri))),
    }
}

/// List records in a collection
async fn list_records(
    State(ctx): State<AppContext>,
    Query(query): Query<ListRecordsQuery>,
) -> PdsResult<Json<ListRecordsResponse>> {
    // Get the DID
    let did = &query.repo;

    // Create repository manager
    let repo_mgr = RepositoryManager::new(did.clone(), (*ctx.actor_store).clone());

    // Fetch limit + 1 to determine if there are more records
    let fetch_limit = query.limit + 1;
    let records = repo_mgr
        .list_records(&query.collection, fetch_limit, query.cursor.as_deref())
        .await?;

    // Determine if we have more records and calculate cursor
    let has_more = records.len() as i64 > query.limit;
    let records_to_return = if has_more {
        &records[0..query.limit as usize]
    } else {
        &records[..]
    };

    // Convert to response format and fetch labels
    let mut entries = Vec::new();
    let mut next_cursor: Option<String> = None;

    for rec in records_to_return {
        let uri = rec.get("uri").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let cid = rec.get("cid").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let value = rec.get("value").cloned().unwrap_or(serde_json::Value::Null);

        // Extract rkey from URI for cursor (format: at://did/collection/rkey)
        if let Some(rkey) = uri.split('/').last() {
            next_cursor = Some(rkey.to_string());
        }

        // Fetch labels for this record
        let labels = ctx.label_manager.get_labels(&uri).await
            .ok()
            .map(|lbls| lbls.into_iter().map(LabelView::from).collect());

        entries.push(RecordEntry {
            uri,
            cid,
            value,
            labels,
        });
    }

    Ok(Json(ListRecordsResponse {
        records: entries,
        cursor: if has_more { next_cursor } else { None },
    }))
}

/// Describe a repository
async fn describe_repo(
    State(ctx): State<AppContext>,
    Query(query): Query<DescribeRepoQuery>,
) -> PdsResult<Json<DescribeRepoResponse>> {
    // Get the DID
    let did = &query.repo;

    // Create repository manager
    let repo_mgr = RepositoryManager::new(did.clone(), (*ctx.actor_store).clone());

    // Get description with account manager and identity resolver
    let desc = repo_mgr.describe_repo(
        Some(&ctx.account_manager),
        Some(&ctx.identity_resolver),
    ).await?;

    Ok(Json(DescribeRepoResponse {
        did: desc.get("did").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        handle: desc.get("handle").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        did_doc: desc.get("didDoc").cloned(),
        collections: desc
            .get("collections")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default(),
        handle_is_correct: desc
            .get("handleIsCorrect")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
    }))
}

/// Apply writes (batch operations with validation)
///
/// Performs atomic batch operations with:
/// - Full validation of all operations before execution
/// - Duplicate detection
/// - Size limit enforcement
/// - All-or-nothing atomicity
async fn apply_writes(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(req): Json<ApplyWritesRequest>,
) -> PdsResult<Json<serde_json::Value>> {
    // Require authentication
    let session = middleware::require_auth(State(ctx.clone()), headers).await?;

    // Verify repo matches authenticated user
    if req.repo != session.did {
        return Err(PdsError::Authorization(
            "Cannot apply writes to another user's repo".to_string(),
        ));
    }

    // Create repository manager
    let repo_mgr = RepositoryManager::with_sequencer(session.did.clone(), (*ctx.actor_store).clone(), ctx.sequencer.clone());

    // Prepare writes (converts to PreparedWrite format)
    let prepared = repo_mgr.prepare_writes(req.writes)?;

    tracing::info!(
        "Applying batch of {} operations for {}",
        prepared.len(),
        session.did
    );

    // Create signer from repo key
    let signer = create_repo_signer(&ctx.config.authentication.repo_signing_key);

    // Apply batch atomically (includes validation)
    let (commit_cid, rev) = repo_mgr
        .apply_batch_writes(prepared, signer)
        .await?;

    tracing::info!(
        "Successfully committed batch for {} (rev: {})",
        session.did,
        rev
    );

    Ok(Json(serde_json::json!({
        "commit": {
            "cid": commit_cid,
            "rev": rev,
        }
    })))
}
