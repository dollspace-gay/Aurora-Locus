//! Generated code for com.atproto.identity.resolveIdentity
//!
//! Resolves an identity (DID or Handle) to a full identity (DID document and verified handle).

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// Handle or DID to resolve.
    pub identifier: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Error: HandleNotFound
/// The resolution process confirmed that the handle does not resolve to any DID.
#[derive(Debug, Clone, thiserror::Error)]
#[error("HandleNotFound")]
pub struct HandleNotFoundError;

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

/// Resolves an identity (DID or Handle) to a full identity (DID document and verified handle).
pub async fn resolve_identity(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.identity.resolveIdentity");

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
