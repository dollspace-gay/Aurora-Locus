//! Generated code for app.bsky.feed.getRepostedBy
//!
//! Get a list of reposts for a given post.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    /// Reference (AT-URI) of post record
    pub uri: crate::syntax::AtUri,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// If supplied, filters to reposts of specific version (by CID) of the post record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub uri: crate::syntax::AtUri,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    #[serde(rename = "repostedBy")]
    pub reposted_by: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Get a list of reposts for a given post.
pub async fn get_reposted_by(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("app.bsky.feed.getRepostedBy");

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
