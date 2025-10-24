//! Generated code for app.bsky.graph.getRelationships
//!
//! Enumerates public relationships between one account, and a list of other accounts. Does not require auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// Primary account requesting relationships for.
    pub actor: String,
    /// List of 'other' accounts to be related back to the primary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub others: Option<serde_json::Value>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<crate::types::Did>,
    pub relationships: serde_json::Value,
}

/// Error: ActorNotFound
/// the primary actor at-identifier could not be resolved
#[derive(Debug, Clone, thiserror::Error)]
#[error("ActorNotFound")]
pub struct ActorNotFoundError;

/// Enumerates public relationships between one account, and a list of other accounts. Does not require auth.
pub async fn get_relationships(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("app.bsky.graph.getRelationships");

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
