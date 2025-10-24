//! Generated code for com.atproto.server.getSession
//!
//! Get information about the current auth session. Requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    /// If active=false, this optional field indicates a possible reason for why the account is not active. If active=false and no status is supplied, then the host makes no claim for why the repository is no longer being hosted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "emailConfirmed")]
    pub email_confirmed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "emailAuthFactor")]
    pub email_auth_factor: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub handle: String,
    pub did: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "didDoc")]
    pub did_doc: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
}

/// Get information about the current auth session. Requires auth.
pub async fn get_session(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::query("com.atproto.server.getSession");

    client.request(req).await
}
