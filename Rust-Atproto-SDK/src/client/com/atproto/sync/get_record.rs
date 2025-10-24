//! Generated code for com.atproto.sync.getRecord
//!
//! Get data blocks needed to prove the existence or non-existence of record in the current version of repo. Does not require auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// The DID of the repo.
    pub did: crate::types::Did,
    pub collection: String,
    /// Record Key
    pub rkey: String,
}

/// Error: RecordNotFound
#[derive(Debug, Clone, thiserror::Error)]
#[error("RecordNotFound")]
pub struct RecordNotFoundError;

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

/// Get data blocks needed to prove the existence or non-existence of record in the current version of repo. Does not require auth.
pub async fn get_record(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<()>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.sync.getRecord");

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
