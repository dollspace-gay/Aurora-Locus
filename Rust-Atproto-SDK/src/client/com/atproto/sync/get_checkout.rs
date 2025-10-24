//! Generated code for com.atproto.sync.getCheckout
//!
//! DEPRECATED - please use com.atproto.sync.getRepo instead

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// The DID of the repo.
    pub did: crate::types::Did,
}

/// DEPRECATED - please use com.atproto.sync.getRepo instead
pub async fn get_checkout(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<()>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.sync.getCheckout");

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
