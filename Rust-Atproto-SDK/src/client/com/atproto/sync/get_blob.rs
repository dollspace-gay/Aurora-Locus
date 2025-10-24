//! Generated code for com.atproto.sync.getBlob
//!
//! Get a blob associated with a given account. Returns the full blob as originally uploaded. Does not require auth; implemented by PDS.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// The CID of the blob to fetch
    pub cid: String,
    /// The DID of the account.
    pub did: crate::types::Did,
}

/// Error: BlobNotFound
#[derive(Debug, Clone, thiserror::Error)]
#[error("BlobNotFound")]
pub struct BlobNotFoundError;

/// Error: RepoNotFound
#[derive(Debug, Clone, thiserror::Error)]
#[error("RepoNotFound")]
pub struct RepoNotFoundError;

/// Error: RepoTakendown
#[derive(Debug, Clone, thiserror::Error)]
#[error("RepoTakendown")]
pub struct RepoTakendownError;

/// Error: RepoSuspended
#[derive(Debug, Clone, thiserror::Error)]
#[error("RepoSuspended")]
pub struct RepoSuspendedError;

/// Error: RepoDeactivated
#[derive(Debug, Clone, thiserror::Error)]
#[error("RepoDeactivated")]
pub struct RepoDeactivatedError;

/// Get a blob associated with a given account. Returns the full blob as originally uploaded. Does not require auth; implemented by PDS.
pub async fn get_blob(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<()>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.sync.getBlob");

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
