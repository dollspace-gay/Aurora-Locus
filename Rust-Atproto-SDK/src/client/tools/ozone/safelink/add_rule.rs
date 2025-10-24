//! Generated code for tools.ozone.safelink.addRule
//!
//! Add a new URL safety rule

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub action: serde_json::Value,
    pub pattern: serde_json::Value,
    pub reason: serde_json::Value,
    /// Optional comment about the decision
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// The URL or domain to apply the rule to
    pub url: String,
    /// Author DID. Only respected when using admin auth
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdBy")]
    pub created_by: Option<crate::types::Did>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Add a new URL safety rule
pub async fn add_rule(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.safelink.addRule").data(&input)?;

    client.request(req).await
}
