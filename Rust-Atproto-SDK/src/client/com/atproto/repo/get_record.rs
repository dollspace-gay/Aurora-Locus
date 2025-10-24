//! Generated code for com.atproto.repo.getRecord
//!
//! Get a single record from a repository. Does not require auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// The NSID of the record collection.
    pub collection: String,
    /// The CID of the version of the record. If not specified, then return the most recent version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    /// The handle or DID of the repo.
    pub repo: String,
    /// The Record Key.
    pub rkey: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub uri: crate::syntax::AtUri,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    pub value: serde_json::Value,
}

/// Error: RecordNotFound
#[derive(Debug, Clone, thiserror::Error)]
#[error("RecordNotFound")]
pub struct RecordNotFoundError;

/// Get a single record from a repository. Does not require auth.
pub async fn get_record(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.repo.getRecord");

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
