//! Generated code for app.bsky.feed.getPostThread
//!
//! Get posts in a thread. Does not require auth, but additional metadata and filtering will be applied for authed requests.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// How many levels of reply depth should be included in response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<i64>,
    /// How many levels of parent (and grandparent, etc) post to include.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "parentHeight")]
    pub parent_height: Option<i64>,
    /// Reference (AT-URI) to post record.
    pub uri: crate::syntax::AtUri,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threadgate: Option<serde_json::Value>,
    pub thread: serde_json::Value,
}

/// Error: NotFound
#[derive(Debug, Clone, thiserror::Error)]
#[error("NotFound")]
pub struct NotFoundError;

/// Get posts in a thread. Does not require auth, but additional metadata and filtering will be applied for authed requests.
pub async fn get_post_thread(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("app.bsky.feed.getPostThread");

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
