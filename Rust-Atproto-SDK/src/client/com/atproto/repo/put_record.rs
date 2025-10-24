//! Generated code for com.atproto.repo.putRecord
//!
//! Write a repository record, creating or updating it as needed. Requires auth, implemented by PDS.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// Compare and swap with the previous commit by CID.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "swapCommit")]
    pub swap_commit: Option<String>,
    /// Compare and swap with the previous record by CID. WARNING: nullable and optional field; may cause problems with golang implementation
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "swapRecord")]
    pub swap_record: Option<String>,
    /// The NSID of the record collection.
    pub collection: String,
    /// The Record Key.
    pub rkey: String,
    /// Can be set to 'false' to skip Lexicon schema validation of record data, 'true' to require it, or leave unset to validate only for known Lexicons.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validate: Option<bool>,
    /// The handle or DID of the repo (aka, current account).
    pub repo: String,
    /// The record to write.
    pub record: serde_json::Value,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub uri: crate::syntax::AtUri,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "validationStatus")]
    pub validation_status: Option<String>,
    pub cid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<serde_json::Value>,
}

/// Write a repository record, creating or updating it as needed. Requires auth, implemented by PDS.
pub async fn put_record(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.repo.putRecord").data(&input)?;

    client.request(req).await
}
