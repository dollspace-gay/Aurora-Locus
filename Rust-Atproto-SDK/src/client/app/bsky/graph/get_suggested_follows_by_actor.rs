//! Generated code for app.bsky.graph.getSuggestedFollowsByActor
//!
//! Enumerates follows similar to a given account (actor). Expected use is to recommend additional accounts immediately after following one account.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    pub actor: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub suggestions: serde_json::Value,
    /// Snowflake for this recommendation, use when submitting recommendation events.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "recId")]
    pub rec_id: Option<i64>,
    /// If true, response has fallen-back to generic results, and is not scoped using relativeToDid
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isFallback")]
    pub is_fallback: Option<bool>,
}

/// Enumerates follows similar to a given account (actor). Expected use is to recommend additional accounts immediately after following one account.
pub async fn get_suggested_follows_by_actor(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("app.bsky.graph.getSuggestedFollowsByActor");

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
