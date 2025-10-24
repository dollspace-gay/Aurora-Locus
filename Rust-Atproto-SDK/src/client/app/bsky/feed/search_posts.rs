//! Generated code for app.bsky.feed.searchPosts
//!
//! Find posts matching search criteria, returning views of those posts. Note that this API endpoint may require authentication (eg, not public) for some service providers and implementations.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// Filter to posts with URLs (facet links or embeds) linking to the given domain (hostname). Server may apply hostname normalization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Filter to posts which mention the given account. Handles are resolved to DID before query-time. Only matches rich-text facet mentions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mentions: Option<String>,
    /// Filter to posts in the given language. Expected to be based on post language field, though server may override language detection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// Filter to posts with the given tag (hashtag), based on rich-text facet or tag field. Do not include the hash (#) prefix. Multiple tags can be specified, with 'AND' matching.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<serde_json::Value>,
    /// Filter to posts by the given account. Handles are resolved to DID before query-time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Filter results for posts after the indicated datetime (inclusive). Expected to use 'sortAt' timestamp, which may not match 'createdAt'. Can be a datetime, or just an ISO date (YYYY-MM-DD).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// Optional pagination mechanism; may not necessarily allow scrolling through entire result set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Filter results for posts before the indicated datetime (not inclusive). Expected to use 'sortAt' timestamp, which may not match 'createdAt'. Can be a datetime, or just an ISO date (YYY-MM-DD).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    /// Filter to posts with links (facet links or embeds) pointing to this URL. Server may apply URL normalization or fuzzy matching.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Search query string; syntax, phrase, boolean, and faceting is unspecified, but Lucene query syntax is recommended.
    pub q: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    /// Specifies the ranking order of results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub posts: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Count of search hits. Optional, may be rounded/truncated, and may not be possible to paginate through all hits.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hitsTotal")]
    pub hits_total: Option<i64>,
}

/// Error: BadQueryString
#[derive(Debug, Clone, thiserror::Error)]
#[error("BadQueryString")]
pub struct BadQueryStringError;

/// Find posts matching search criteria, returning views of those posts. Note that this API endpoint may require authentication (eg, not public) for some service providers and implementations.
pub async fn search_posts(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("app.bsky.feed.searchPosts");

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
