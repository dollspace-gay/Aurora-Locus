//! Generated code for app.bsky.feed.getActorLikes
//!
//! Get a list of posts liked by an actor. Requires auth, actor must be the requesting account.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    pub actor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub feed: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Error: BlockedActor
#[derive(Debug, Clone, thiserror::Error)]
#[error("BlockedActor")]
pub struct BlockedActorError;

/// Error: BlockedByActor
#[derive(Debug, Clone, thiserror::Error)]
#[error("BlockedByActor")]
pub struct BlockedByActorError;

/// Get a list of posts liked by an actor. Requires auth, actor must be the requesting account.
pub async fn get_actor_likes(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("app.bsky.feed.getActorLikes");

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
