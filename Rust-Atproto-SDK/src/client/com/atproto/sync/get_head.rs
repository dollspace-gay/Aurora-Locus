//! Generated code for com.atproto.sync.getHead
//!
//! DEPRECATED - please use com.atproto.sync.getLatestCommit instead

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
    pub root: String,
}

/// Error: HeadNotFound
#[derive(Debug, Clone, thiserror::Error)]
#[error("HeadNotFound")]
pub struct HeadNotFoundError;

/// DEPRECATED - please use com.atproto.sync.getLatestCommit instead
pub async fn get_head(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.sync.getHead");

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
