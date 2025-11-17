//! Synchronization API endpoints
//!
//! Implements com.atproto.sync.* endpoints for federation and repository export
//!
//! These endpoints enable:
//! - Repository synchronization across PDSs
//! - External crawlers and indexers
//! - Backup and migration
//! - ATProto spec compliance

use crate::{
    actor_store::car,
    api::{middleware, sync_helpers::{assert_repo_availability, get_repo_status}},
    context::AppContext,
    error::{PdsError, PdsResult},
};
use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};

/// Request parameters for getRepo
#[derive(Debug, Deserialize)]
pub struct GetRepoParams {
    /// DID of the repository
    pub did: String,
    /// Optional: commit CID to retrieve specific version (incremental sync)
    pub since: Option<String>,
}

/// Request parameters for getLatestCommit
#[derive(Debug, Deserialize)]
pub struct GetLatestCommitParams {
    /// DID of the repository
    pub did: String,
}

/// Response for getLatestCommit
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestCommitResponse {
    pub cid: String,
    pub rev: String,
}

/// Request parameters for getBlocks
#[derive(Debug, Deserialize)]
pub struct GetBlocksParams {
    /// DID of the repository
    pub did: String,
    /// List of CIDs to fetch
    pub cids: Vec<String>,
}

/// Request parameters for getBlob
#[derive(Debug, Deserialize)]
pub struct GetBlobParams {
    /// DID of the repository
    pub did: String,
    /// CID of the blob
    pub cid: String,
}

/// Request parameters for listBlobs
#[derive(Debug, Deserialize)]
pub struct ListBlobsParams {
    /// DID of the repository
    pub did: String,
    /// Optional: only return blobs since this timestamp
    pub since: Option<String>,
    /// Optional: maximum number of results (default: 100, max: 1000)
    pub limit: Option<i64>,
    /// Optional: cursor for pagination
    pub cursor: Option<String>,
}

/// Response for listBlobs
#[derive(Debug, Serialize)]
pub struct ListBlobsResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    pub cids: Vec<String>,
}

/// Request parameters for listRepos
#[derive(Debug, Deserialize)]
pub struct ListReposParams {
    /// Optional cursor for pagination
    pub cursor: Option<String>,
    /// Optional limit (default: 500, max: 1000)
    pub limit: Option<i64>,
}

/// Response for listRepos
#[derive(Debug, Serialize)]
pub struct ListReposResponse {
    pub repos: Vec<RepoInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Repository information
#[derive(Debug, Serialize)]
pub struct RepoInfo {
    pub did: String,
    pub head: String,
    pub rev: String,
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Request parameters for getRepoStatus
#[derive(Debug, Deserialize)]
pub struct GetRepoStatusParams {
    /// DID of the repository
    pub did: String,
}

/// Response for getRepoStatus
#[derive(Debug, Serialize)]
pub struct GetRepoStatusResponse {
    pub did: String,
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
}

/// Request parameters for getRecord
#[derive(Debug, Deserialize)]
pub struct GetRecordParams {
    /// DID of the repository
    pub did: String,
    /// Collection name
    pub collection: String,
    /// Record key
    pub rkey: String,
}

/// Get a repository as a CAR file export
///
/// Implements com.atproto.sync.getRepo
pub async fn get_repo(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Query(params): Query<GetRepoParams>,
) -> PdsResult<Response> {
    // Get authentication
    let auth = middleware::require_auth_unified(State(ctx.clone()), headers).await?;
    let auth_did = auth.did();

    // Check repository availability (admin support to be added in Phase 7)
    let is_admin_or_self = auth_did == params.did;
    assert_repo_availability(&ctx.account_manager, &params.did, is_admin_or_self).await?;

    // Export repository as CAR
    let car_bytes = car::export_repo_to_car(
        &ctx.actor_store,
        &params.did,
        params.since.as_deref(),
    )
    .await?;

    // Return CAR file as application/vnd.ipld.car
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.ipld.car")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}.car\"", params.did),
        )
        .body(Body::from(car_bytes))
        .unwrap())
}

/// Get the latest commit for a repository
///
/// Implements com.atproto.sync.getLatestCommit
pub async fn get_latest_commit(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Query(params): Query<GetLatestCommitParams>,
) -> PdsResult<Json<LatestCommitResponse>> {
    // Get authentication
    let auth = middleware::require_auth_unified(State(ctx.clone()), headers).await?;
    let auth_did = auth.did();

    // Check repository availability (admin support to be added in Phase 7)
    let is_admin_or_self = auth_did == params.did;
    assert_repo_availability(&ctx.account_manager, &params.did, is_admin_or_self).await?;

    // Get the repository root CID (latest commit)
    let repo_root = ctx.actor_store.get_repo_root(&params.did).await?;

    Ok(Json(LatestCommitResponse {
        cid: repo_root.cid,
        rev: repo_root.rev,
    }))
}

/// Get specific blocks from a repository
///
/// Implements com.atproto.sync.getBlocks
pub async fn get_blocks(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Query(params): Query<GetBlocksParams>,
) -> PdsResult<Response> {
    // Get authentication
    let auth = middleware::require_auth_unified(State(ctx.clone()), headers).await?;
    let auth_did = auth.did();

    // Check repository availability (admin support to be added in Phase 7)
    let is_admin_or_self = auth_did == params.did;
    assert_repo_availability(&ctx.account_manager, &params.did, is_admin_or_self).await?;

    // Fetch the requested blocks
    let block_data = ctx
        .actor_store
        .get_blocks_by_cids(&params.did, &params.cids)
        .await?;

    // Check if any blocks are missing
    if block_data.len() != params.cids.len() {
        let found_cids: Vec<&str> = block_data.iter().map(|(cid, _)| cid.as_str()).collect();
        let missing: Vec<&str> = params
            .cids
            .iter()
            .filter(|cid| !found_cids.contains(&cid.as_str()))
            .map(|s| s.as_str())
            .collect();
        return Err(PdsError::NotFound(format!(
            "Could not find cids: {:?}",
            missing
        )));
    }

    // Get repo root for CAR header
    let repo_root = ctx.actor_store.get_repo_root(&params.did).await?;

    // Convert blocks to CAR
    let car_bytes = car::blocks_to_car(block_data, Some(&repo_root.cid)).await?;

    // Return CAR file
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.ipld.car")
        .body(Body::from(car_bytes))
        .unwrap())
}

/// Get a blob by CID
///
/// Implements com.atproto.sync.getBlob
pub async fn get_blob(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Query(params): Query<GetBlobParams>,
) -> PdsResult<Response> {
    // Get authentication
    let auth = middleware::require_auth_unified(State(ctx.clone()), headers).await?;
    let auth_did = auth.did();

    // Check repository availability (admin support to be added in Phase 7)
    let is_admin_or_self = auth_did == params.did;
    assert_repo_availability(&ctx.account_manager, &params.did, is_admin_or_self).await?;

    // Get blob from blob store
    let (data, mime_type) = ctx
        .blob_store
        .get(&params.cid)
        .await?
        .ok_or_else(|| PdsError::NotFound("Blob not found".to_string()))?;

    // Return blob with security headers (matching Bluesky pattern)
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime_type.as_str()),
            (header::CONTENT_LENGTH, &data.len().to_string()),
            (header::HeaderName::from_static("x-content-type-options"), "nosniff"),
            (header::HeaderName::from_static("content-security-policy"), "default-src 'none'; sandbox"),
        ],
        data,
    )
        .into_response())
}

/// List blob CIDs in a repository
///
/// Implements com.atproto.sync.listBlobs
pub async fn list_blobs(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Query(params): Query<ListBlobsParams>,
) -> PdsResult<Json<ListBlobsResponse>> {
    // Get authentication
    let auth = middleware::require_auth_unified(State(ctx.clone()), headers).await?;
    let auth_did = auth.did();

    // Check repository availability (admin support to be added in Phase 7)
    let is_admin_or_self = auth_did == params.did;
    assert_repo_availability(&ctx.account_manager, &params.did, is_admin_or_self).await?;

    let limit = params.limit.unwrap_or(100).min(1000);

    // Get blob CIDs
    let cids = ctx
        .blob_store
        .list_blob_cids(
            &params.did,
            params.since.as_deref(),
            limit,
            params.cursor.as_deref(),
        )
        .await?;

    // Cursor is the last CID if we hit the limit
    let cursor = if cids.len() as i64 == limit {
        cids.last().cloned()
    } else {
        None
    };

    Ok(Json(ListBlobsResponse { cursor, cids }))
}

/// List all repositories on this PDS
///
/// Implements com.atproto.sync.listRepos
pub async fn list_repos(
    State(ctx): State<AppContext>,
    Query(params): Query<ListReposParams>,
) -> PdsResult<Json<ListReposResponse>> {
    let limit = params.limit.unwrap_or(500).min(1000);

    // Get list of all accounts with pagination
    let accounts = ctx
        .account_manager
        .list_accounts(params.cursor.as_deref(), limit)
        .await?;

    // Build repository info for each account
    let mut repos = Vec::new();
    for account in &accounts {
        // Get the repository root for this DID
        if let Ok(repo_root) = ctx.actor_store.get_repo_root(&account.did).await {
            let (active, status) = get_repo_status(
                account.takedown_ref.is_some(),
                account.deactivated_at.as_ref(),
            );

            repos.push(RepoInfo {
                did: account.did.clone(),
                head: repo_root.cid,
                rev: repo_root.rev,
                active,
                status,
            });
        }
    }

    // Determine next cursor
    let cursor = if repos.len() as i64 == limit {
        // There may be more results, return the last DID as cursor
        accounts.last().map(|a| a.did.clone())
    } else {
        None
    };

    Ok(Json(ListReposResponse { repos, cursor }))
}

/// Get repository status
///
/// Implements com.atproto.sync.getRepoStatus
pub async fn get_repo_status_endpoint(
    State(ctx): State<AppContext>,
    Query(params): Query<GetRepoStatusParams>,
) -> PdsResult<Json<GetRepoStatusResponse>> {
    // No auth required - this is public info about repo availability

    let account = ctx
        .account_manager
        .get_account(&params.did)
        .await?;

    let (active, status) = get_repo_status(
        account.takedown_ref.is_some(),
        account.deactivated_at.as_ref(),
    );

    let rev = if active {
        ctx.actor_store
            .get_repo_root(&params.did)
            .await
            .ok()
            .map(|root| root.rev)
    } else {
        None
    };

    Ok(Json(GetRepoStatusResponse {
        did: params.did,
        active,
        status,
        rev,
    }))
}

/// Get a specific record as CAR file
///
/// Implements com.atproto.sync.getRecord
pub async fn get_record(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Query(params): Query<GetRecordParams>,
) -> PdsResult<Response> {
    // Get authentication
    let auth = middleware::require_auth_unified(State(ctx.clone()), headers).await?;
    let auth_did = auth.did();

    // Check repository availability (admin support to be added in Phase 7)
    let is_admin_or_self = auth_did == params.did;
    assert_repo_availability(&ctx.account_manager, &params.did, is_admin_or_self).await?;

    // Export record as CAR
    let car_bytes = car::export_record_to_car(
        &ctx.actor_store,
        &params.did,
        &params.collection,
        &params.rkey,
    )
    .await?;

    // Return CAR file
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/vnd.ipld.car")],
        car_bytes,
    )
        .into_response())
}

/// Build sync API routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        .route("/xrpc/com.atproto.sync.getRepo", get(get_repo))
        .route(
            "/xrpc/com.atproto.sync.getLatestCommit",
            get(get_latest_commit),
        )
        .route("/xrpc/com.atproto.sync.getBlocks", get(get_blocks))
        .route("/xrpc/com.atproto.sync.getBlob", get(get_blob))
        .route("/xrpc/com.atproto.sync.listBlobs", get(list_blobs))
        .route("/xrpc/com.atproto.sync.listRepos", get(list_repos))
        .route(
            "/xrpc/com.atproto.sync.getRepoStatus",
            get(get_repo_status_endpoint),
        )
        .route("/xrpc/com.atproto.sync.getRecord", get(get_record))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_repo_params_deserialize() {
        let json = r#"{"did":"did:plc:test","since":"bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454"}"#;
        let params: GetRepoParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.did, "did:plc:test");
        assert!(params.since.is_some());
    }

    #[test]
    fn test_latest_commit_response_serialize() {
        let response = LatestCommitResponse {
            cid: "bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454".to_string(),
            rev: "3l4example".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("cid"));
        assert!(json.contains("rev"));
    }

    #[test]
    fn test_list_blobs_params_deserialize() {
        let json = r#"{"did":"did:plc:test","limit":50,"cursor":"bafyreiabc"}"#;
        let params: ListBlobsParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.did, "did:plc:test");
        assert_eq!(params.limit, Some(50));
        assert_eq!(params.cursor, Some("bafyreiabc".to_string()));
    }
}
