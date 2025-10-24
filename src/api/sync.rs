/// Synchronization API endpoints
///
/// Implements com.atproto.sync.* endpoints for federation and repository export

use crate::{
    car::CarEncoder,
    context::AppContext,
    error::{PdsError, PdsResult},
};
use libipld::Cid;
use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::Response,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Request parameters for getRepo
#[derive(Debug, Deserialize)]
pub struct GetRepoParams {
    /// DID of the repository
    pub did: String,
    /// Optional: commit CID to retrieve specific version
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
}

/// Get a repository as a CAR file export
///
/// Implements com.atproto.sync.getRepo
pub async fn get_repo(
    State(ctx): State<AppContext>,
    Query(params): Query<GetRepoParams>,
) -> PdsResult<Response> {
    // Validate DID exists
    if !ctx.actor_store.exists(&params.did).await {
        return Err(PdsError::NotFound(format!(
            "Repository not found for DID: {}",
            params.did
        )));
    }

    // Get the repository root CID
    let repo_root = ctx.actor_store.get_repo_root(&params.did).await?;
    let root_cid = Cid::from_str(&repo_root.cid)
        .map_err(|e| PdsError::Internal(format!("Invalid root CID: {}", e)))?;

    // Create CAR encoder
    let mut encoder = CarEncoder::new(&root_cid)?;

    // Get all blocks for this repository
    let block_data = ctx.actor_store.get_all_blocks(&params.did).await?;

    // Convert to (Cid, Vec<u8>) format
    let blocks: Vec<(Cid, Vec<u8>)> = block_data
        .into_iter()
        .filter_map(|(cid_str, content)| {
            Cid::from_str(&cid_str).ok().map(|cid| (cid, content))
        })
        .collect();

    encoder.add_blocks(blocks)?;

    let car_bytes = encoder.finalize();

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
    Query(params): Query<GetLatestCommitParams>,
) -> PdsResult<Json<LatestCommitResponse>> {
    // Validate DID exists
    if !ctx.actor_store.exists(&params.did).await {
        return Err(PdsError::NotFound(format!(
            "Repository not found for DID: {}",
            params.did
        )));
    }

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
    Query(params): Query<GetBlocksParams>,
) -> PdsResult<Response> {
    // Validate DID exists
    if !ctx.actor_store.exists(&params.did).await {
        return Err(PdsError::NotFound(format!(
            "Repository not found for DID: {}",
            params.did
        )));
    }

    // Validate CIDs
    let cids: Result<Vec<Cid>, _> = params
        .cids
        .iter()
        .map(|s| Cid::from_str(s))
        .collect();

    let cids = cids.map_err(|e| PdsError::Validation(format!("Invalid CID: {}", e)))?;

    // Get the repository root for the CAR header
    let repo_root = ctx.actor_store.get_repo_root(&params.did).await?;
    let root_cid = Cid::from_str(&repo_root.cid)
        .map_err(|e| PdsError::Internal(format!("Invalid root CID: {}", e)))?;

    // Create CAR encoder
    let mut encoder = CarEncoder::new(&root_cid)?;

    // Fetch the requested blocks from repo_block table
    let cid_strings: Vec<String> = cids.iter().map(|c| c.to_string()).collect();
    let block_data = ctx
        .actor_store
        .get_blocks_by_cids(&params.did, &cid_strings)
        .await?;

    // Convert to (Cid, Vec<u8>) format
    let blocks: Vec<(Cid, Vec<u8>)> = block_data
        .into_iter()
        .filter_map(|(cid_str, content)| {
            Cid::from_str(&cid_str).ok().map(|cid| (cid, content))
        })
        .collect();

    encoder.add_blocks(blocks)?;

    let car_bytes = encoder.finalize();

    // Return CAR file
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.ipld.car")
        .body(Body::from(car_bytes))
        .unwrap())
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
            repos.push(RepoInfo {
                did: account.did.clone(),
                head: repo_root.cid,
                rev: repo_root.rev,
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

/// Build sync API routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        .route(
            "/xrpc/com.atproto.sync.getRepo",
            get(get_repo),
        )
        .route(
            "/xrpc/com.atproto.sync.getLatestCommit",
            get(get_latest_commit),
        )
        .route(
            "/xrpc/com.atproto.sync.getBlocks",
            get(get_blocks),
        )
        .route(
            "/xrpc/com.atproto.sync.listRepos",
            get(list_repos),
        )
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
}
