//! Generated code for com.atproto.repo.describeRepo
//!
//! Get information about an account and repository, including the list of collections. Does not require auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// The handle or DID of the repo.
    pub repo: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    /// List of all the collections (NSIDs) for which this repo contains at least one record.
    pub collections: serde_json::Value,
    /// Indicates if handle is currently valid (resolves bi-directionally)
    #[serde(rename = "handleIsCorrect")]
    pub handle_is_correct: bool,
    pub did: crate::types::Did,
    pub handle: String,
    /// The complete DID document for this account.
    #[serde(rename = "didDoc")]
    pub did_doc: serde_json::Value,
}

/// Get information about an account and repository, including the list of collections. Does not require auth.
pub async fn describe_repo(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.repo.describeRepo");

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
