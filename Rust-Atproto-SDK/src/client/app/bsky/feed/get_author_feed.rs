//! Generated code for app.bsky.feed.getAuthorFeed
//!
//! Get a view of an actor's 'author feed' (post and reposts by the author). Does not require auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// Combinations of post/repost types to include in response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    pub actor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "includePins")]
    pub include_pins: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    pub feed: serde_json::Value,
}

/// Error: BlockedActor
#[derive(Debug, Clone, thiserror::Error)]
#[error("BlockedActor")]
pub struct BlockedActorError;

/// Error: BlockedByActor
#[derive(Debug, Clone, thiserror::Error)]
#[error("BlockedByActor")]
pub struct BlockedByActorError;

/// Get a view of an actor's 'author feed' (post and reposts by the author). Does not require auth.
pub async fn get_author_feed(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("app.bsky.feed.getAuthorFeed");

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
