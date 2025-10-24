//! Generated code for com.atproto.temp.dereferenceScope
//!
//! Allows finding the oauth permission scope from a reference

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// The scope reference (starts with 'ref:')
    pub scope: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    /// The full oauth permission scope
    pub scope: String,
}

/// Error: InvalidScopeReference
/// An invalid scope reference was provided.
#[derive(Debug, Clone, thiserror::Error)]
#[error("InvalidScopeReference")]
pub struct InvalidScopeReferenceError;

/// Allows finding the oauth permission scope from a reference
pub async fn dereference_scope(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.temp.dereferenceScope");

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
