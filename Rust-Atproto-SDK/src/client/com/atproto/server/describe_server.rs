//! Generated code for com.atproto.server.describeServer
//!
//! Describes the server's account creation requirements and capabilities. Implemented by PDS.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    /// If true, a phone verification token must be supplied to create an account on this instance.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "phoneVerificationRequired")]
    pub phone_verification_required: Option<bool>,
    /// Contact information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact: Option<serde_json::Value>,
    /// If true, an invite code must be supplied to create an account on this instance.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "inviteCodeRequired")]
    pub invite_code_required: Option<bool>,
    /// List of domain suffixes that can be used in account handles.
    #[serde(rename = "availableUserDomains")]
    pub available_user_domains: serde_json::Value,
    /// URLs of service policy documents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<serde_json::Value>,
    pub did: crate::types::Did,
}

/// Describes the server's account creation requirements and capabilities. Implemented by PDS.
pub async fn describe_server(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::query("com.atproto.server.describeServer");

    client.request(req).await
}
