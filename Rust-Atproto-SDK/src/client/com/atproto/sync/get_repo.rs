//! Generated code for com.atproto.sync.getRepo
//!
//! Download a repository export as CAR file. Optionally only a 'diff' since a previous revision. Does not require auth; implemented by PDS.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// The revision ('rev') of the repo to create a diff from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// The DID of the repo.
    pub did: crate::types::Did,
}

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

/// Download a repository export as CAR file. Optionally only a 'diff' since a previous revision. Does not require auth; implemented by PDS.
pub async fn get_repo(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<()>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.sync.getRepo");

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
