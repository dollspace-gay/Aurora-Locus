//! Generated code for com.atproto.sync.getBlocks
//!
//! Get data blocks from a given repo, by CID. For example, intermediate MST nodes, or records. Does not require auth; implemented by PDS.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    pub cids: serde_json::Value,
    /// The DID of the repo.
    pub did: crate::types::Did,
}

/// Error: BlockNotFound
#[derive(Debug, Clone, thiserror::Error)]
#[error("BlockNotFound")]
pub struct BlockNotFoundError;

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

/// Get data blocks from a given repo, by CID. For example, intermediate MST nodes, or records. Does not require auth; implemented by PDS.
pub async fn get_blocks(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<()>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.sync.getBlocks");

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
