//! Generated code for app.bsky.unspecced.getSuggestionsSkeleton
//!
//! Get a skeleton of suggested actors. Intended to be called and then hydrated through app.bsky.actor.getSuggestions

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// DID of the account to get suggestions relative to. If not provided, suggestions will be based on the viewer.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "relativeToDid")]
    pub relative_to_did: Option<crate::types::Did>,
    /// DID of the account making the request (not included for public/unauthenticated queries). Used to boost followed accounts in ranking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer: Option<crate::types::Did>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub actors: serde_json::Value,
    /// Snowflake for this recommendation, use when submitting recommendation events.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "recId")]
    pub rec_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// DID of the account these suggestions are relative to. If this is returned undefined, suggestions are based on the viewer.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "relativeToDid")]
    pub relative_to_did: Option<crate::types::Did>,
}

/// Get a skeleton of suggested actors. Intended to be called and then hydrated through app.bsky.actor.getSuggestions
pub async fn get_suggestions_skeleton(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("app.bsky.unspecced.getSuggestionsSkeleton");

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
