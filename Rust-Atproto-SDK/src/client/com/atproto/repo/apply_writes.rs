//! Generated code for com.atproto.repo.applyWrites
//!
//! Apply a batch transaction of repository creates, updates, and deletes. Requires auth, implemented by PDS.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// If provided, the entire operation will fail if the current repo commit CID does not match this value. Used to prevent conflicting repo mutations.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "swapCommit")]
    pub swap_commit: Option<String>,
    pub writes: serde_json::Value,
    /// The handle or DID of the repo (aka, current account).
    pub repo: String,
    /// Can be set to 'false' to skip Lexicon schema validation of record data across all operations, 'true' to require it, or leave unset to validate only for known Lexicons.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validate: Option<bool>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<serde_json::Value>,
}

/// Apply a batch transaction of repository creates, updates, and deletes. Requires auth, implemented by PDS.
pub async fn apply_writes(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.repo.applyWrites").data(&input)?;

    client.request(req).await
}
