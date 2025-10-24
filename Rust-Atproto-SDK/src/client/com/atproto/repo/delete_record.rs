//! Generated code for com.atproto.repo.deleteRecord
//!
//! Delete a repository record, or ensure it doesn't exist. Requires auth, implemented by PDS.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// The NSID of the record collection.
    pub collection: String,
    /// Compare and swap with the previous record by CID.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "swapRecord")]
    pub swap_record: Option<String>,
    /// The Record Key.
    pub rkey: String,
    /// Compare and swap with the previous commit by CID.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "swapCommit")]
    pub swap_commit: Option<String>,
    /// The handle or DID of the repo (aka, current account).
    pub repo: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<serde_json::Value>,
}

/// Delete a repository record, or ensure it doesn't exist. Requires auth, implemented by PDS.
pub async fn delete_record(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.repo.deleteRecord").data(&input)?;

    client.request(req).await
}
