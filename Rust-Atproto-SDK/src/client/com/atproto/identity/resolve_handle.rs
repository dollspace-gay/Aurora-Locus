//! Generated code for com.atproto.identity.resolveHandle
//!
//! Resolves an atproto handle (hostname) to a DID. Does not necessarily bi-directionally verify against the the DID document.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// The handle to resolve.
    pub handle: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub did: crate::types::Did,
}

/// Error: HandleNotFound
/// The resolution process confirmed that the handle does not resolve to any DID.
#[derive(Debug, Clone, thiserror::Error)]
#[error("HandleNotFound")]
pub struct HandleNotFoundError;

/// Resolves an atproto handle (hostname) to a DID. Does not necessarily bi-directionally verify against the the DID document.
pub async fn resolve_handle(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.identity.resolveHandle");

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
