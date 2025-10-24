//! Generated code for tools.ozone.safelink.updateRule
//!
//! Update an existing URL safety rule

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// Optional DID to credit as the creator. Only respected for admin_token authentication.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdBy")]
    pub created_by: Option<crate::types::Did>,
    pub reason: serde_json::Value,
    /// The URL or domain to update the rule for
    pub url: String,
    /// Optional comment about the update
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    pub action: serde_json::Value,
    pub pattern: serde_json::Value,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Update an existing URL safety rule
pub async fn update_rule(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.safelink.updateRule").data(&input)?;

    client.request(req).await
}
