//! Generated code for com.atproto.server.refreshSession
//!
//! Refresh an authentication session. Requires auth using the 'refreshJwt' (not the 'accessJwt').

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub did: crate::types::Did,
    /// Hosting status of the account. If not specified, then assume 'active'.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(rename = "refreshJwt")]
    pub refresh_jwt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "didDoc")]
    pub did_doc: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(rename = "accessJwt")]
    pub access_jwt: String,
    pub handle: String,
}

/// Refresh an authentication session. Requires auth using the 'refreshJwt' (not the 'accessJwt').
pub async fn refresh_session(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.refreshSession");

    client.request(req).await
}
