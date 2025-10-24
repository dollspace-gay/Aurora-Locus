//! Generated code for com.atproto.identity.resolveDid
//!
//! Resolves DID to DID document. Does not bi-directionally verify handle.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// DID to resolve.
    pub did: crate::types::Did,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    /// The complete DID document for the identity.
    #[serde(rename = "didDoc")]
    pub did_doc: serde_json::Value,
}

/// Error: DidNotFound
/// The DID resolution process confirmed that there is no current DID.
#[derive(Debug, Clone, thiserror::Error)]
#[error("DidNotFound")]
pub struct DidNotFoundError;

/// Error: DidDeactivated
/// The DID previously existed, but has been deactivated.
#[derive(Debug, Clone, thiserror::Error)]
#[error("DidDeactivated")]
pub struct DidDeactivatedError;

/// Resolves DID to DID document. Does not bi-directionally verify handle.
pub async fn resolve_did(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.identity.resolveDid");

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
