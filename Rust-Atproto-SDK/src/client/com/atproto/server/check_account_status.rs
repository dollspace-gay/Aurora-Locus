//! Generated code for com.atproto.server.checkAccountStatus
//!
//! Returns the status of an account, especially as pertaining to import or recovery. Can be called many times over the course of an account migration. Requires auth and can only be called pertaining to oneself.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(rename = "indexedRecords")]
    pub indexed_records: i64,
    #[serde(rename = "repoBlocks")]
    pub repo_blocks: i64,
    #[serde(rename = "privateStateValues")]
    pub private_state_values: i64,
    #[serde(rename = "expectedBlobs")]
    pub expected_blobs: i64,
    #[serde(rename = "importedBlobs")]
    pub imported_blobs: i64,
    #[serde(rename = "validDid")]
    pub valid_did: bool,
    #[serde(rename = "repoCommit")]
    pub repo_commit: String,
    #[serde(rename = "repoRev")]
    pub repo_rev: String,
    pub activated: bool,
}

/// Returns the status of an account, especially as pertaining to import or recovery. Can be called many times over the course of an account migration. Requires auth and can only be called pertaining to oneself.
pub async fn check_account_status(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::query("com.atproto.server.checkAccountStatus");

    client.request(req).await
}
