//! Generated code for tools.ozone.server.getConfig
//!
//! Get details about ozone's server configuration.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    /// The did of the verifier used for verification.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "verifierDid")]
    pub verifier_did: Option<crate::types::Did>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "blobDivert")]
    pub blob_divert: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pds: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub appview: Option<serde_json::Value>,
}

/// Get details about ozone's server configuration.
pub async fn get_config(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::query("tools.ozone.server.getConfig");

    client.request(req).await
}
