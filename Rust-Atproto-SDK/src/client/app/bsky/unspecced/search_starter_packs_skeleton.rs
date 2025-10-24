//! Generated code for app.bsky.unspecced.searchStarterPacksSkeleton
//!
//! Backend Starter Pack search, returns only skeleton.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// Search query string; syntax, phrase, boolean, and faceting is unspecified, but Lucene query syntax is recommended.
    pub q: String,
    /// DID of the account making the request (not included for public/unauthenticated queries).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer: Option<crate::types::Did>,
    /// Optional pagination mechanism; may not necessarily allow scrolling through entire result set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Count of search hits. Optional, may be rounded/truncated, and may not be possible to paginate through all hits.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hitsTotal")]
    pub hits_total: Option<i64>,
    #[serde(rename = "starterPacks")]
    pub starter_packs: serde_json::Value,
}

/// Error: BadQueryString
#[derive(Debug, Clone, thiserror::Error)]
#[error("BadQueryString")]
pub struct BadQueryStringError;

/// Backend Starter Pack search, returns only skeleton.
pub async fn search_starter_packs_skeleton(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("app.bsky.unspecced.searchStarterPacksSkeleton");

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
