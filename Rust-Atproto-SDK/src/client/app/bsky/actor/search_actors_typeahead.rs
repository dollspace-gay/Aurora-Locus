//! Generated code for app.bsky.actor.searchActorsTypeahead
//!
//! Find actor suggestions for a prefix search term. Expected use is for auto-completion during text field entry. Does not require auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    /// DEPRECATED: use 'q' instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub term: Option<String>,
    /// Search query prefix; not a full query string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub actors: serde_json::Value,
}

/// Find actor suggestions for a prefix search term. Expected use is for auto-completion during text field entry. Does not require auth.
pub async fn search_actors_typeahead(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("app.bsky.actor.searchActorsTypeahead");

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
