//! Generated code for tools.ozone.safelink.removeRule
//!
//! Remove an existing URL safety rule

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// The URL or domain to remove the rule for
    pub url: String,
    /// Optional DID of the user. Only respected when using admin auth.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdBy")]
    pub created_by: Option<crate::types::Did>,
    pub pattern: serde_json::Value,
    /// Optional comment about why the rule is being removed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Remove an existing URL safety rule
pub async fn remove_rule(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.safelink.removeRule").data(&input)?;

    client.request(req).await
}
