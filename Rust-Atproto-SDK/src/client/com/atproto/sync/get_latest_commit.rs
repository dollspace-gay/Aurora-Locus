//! Generated code for com.atproto.sync.getLatestCommit
//!
//! Get the current commit CID & revision of the specified repo. Does not require auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// The DID of the repo.
    pub did: crate::types::Did,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub cid: String,
    pub rev: String,
}

/// Error: RepoNotFound
#[derive(Debug, Clone, thiserror::Error)]
#[error("RepoNotFound")]
pub struct RepoNotFoundError;

/// Error: RepoTakendown
#[derive(Debug, Clone, thiserror::Error)]
#[error("RepoTakendown")]
pub struct RepoTakendownError;

/// Error: RepoSuspended
#[derive(Debug, Clone, thiserror::Error)]
#[error("RepoSuspended")]
pub struct RepoSuspendedError;

/// Error: RepoDeactivated
#[derive(Debug, Clone, thiserror::Error)]
#[error("RepoDeactivated")]
pub struct RepoDeactivatedError;

/// Get the current commit CID & revision of the specified repo. Does not require auth.
pub async fn get_latest_commit(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.sync.getLatestCommit");

    // Add query parameters
    let params_json = serde_json::to_value(&params)
        .map_err(XrpcError::Serialization)?;

    if let Some(obj) = params_json.as_object() {
        for (key, value) in obj {
            if let Some(s) = value.as_str() {
                req.params.insert(key.clone(), s.to_string());
            } else {
                req.params.insert(key.clone(), value.to_string());
            }
        }
    }

    client.request(req).await
}
