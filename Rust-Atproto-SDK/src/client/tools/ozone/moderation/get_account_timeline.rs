//! Generated code for tools.ozone.moderation.getAccountTimeline
//!
//! Get timeline of all available events of an account. This includes moderation events, account history and did history.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    pub did: crate::types::Did,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub timeline: serde_json::Value,
}

/// Error: RepoNotFound
#[derive(Debug, Clone, thiserror::Error)]
#[error("RepoNotFound")]
pub struct RepoNotFoundError;

/// Get timeline of all available events of an account. This includes moderation events, account history and did history.
pub async fn get_account_timeline(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("tools.ozone.moderation.getAccountTimeline");

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
