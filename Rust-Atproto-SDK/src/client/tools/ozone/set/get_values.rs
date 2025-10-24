//! Generated code for tools.ozone.set.getValues
//!
//! Get a specific set and its values

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    pub name: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub set: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    pub values: serde_json::Value,
}

/// Error: SetNotFound
/// set with the given name does not exist
#[derive(Debug, Clone, thiserror::Error)]
#[error("SetNotFound")]
pub struct SetNotFoundError;

/// Get a specific set and its values
pub async fn get_values(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("tools.ozone.set.getValues");

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
