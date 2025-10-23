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

/// Dummy signer for development
/// TODO: Replace with actual key signing using stored private keys
fn dummy_signer(_hash: &[u8; 32]) -> Result<Vec<u8>, atproto::repo::RepoError> {
    // Return a dummy 64-byte signature
    // In production, this should use the actor's private key
    Ok(vec![0u8; 64])
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

    // Create the record
    tracing::debug!("create_record: Calling repo_mgr.create_record");
    let (uri, cid, _rev) = repo_mgr
        .create_record(&req.collection, req.rkey.as_deref(), req.record, req.validate, dummy_signer)
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

    // Update the record
    let (cid, _rev) = repo_mgr
        .update_record(&req.collection, &req.rkey, req.record, req.validate, dummy_signer)
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

    // Delete the record
    repo_mgr
        .delete_record(&req.collection, &req.rkey, dummy_signer)
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

    // List records
    let records = repo_mgr
        .list_records(&query.collection, query.limit, query.cursor.as_deref())
        .await?;

    // Convert to response format and fetch labels
    let mut entries = Vec::new();
    for rec in records {
        let uri = rec.get("uri").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let cid = rec.get("cid").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let value = rec.get("value").cloned().unwrap_or(serde_json::Value::Null);

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
        cursor: None, // TODO: Implement cursor pagination
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

    // Get description
    let desc = repo_mgr.describe_repo().await?;

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

    // Apply batch atomically (includes validation)
    let (commit_cid, rev) = repo_mgr
        .apply_batch_writes(prepared, dummy_signer)
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
