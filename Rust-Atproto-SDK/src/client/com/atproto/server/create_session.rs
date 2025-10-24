//! Generated code for com.atproto.server.createSession
//!
//! Create an authentication session.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// Handle or other identifier supported by the server for the authenticating user.
    pub identifier: String,
    pub password: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "authFactorToken")]
    pub auth_factor_token: Option<String>,
    /// When true, instead of throwing error for takendown accounts, a valid response with a narrow scoped token will be returned
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "allowTakendown")]
    pub allow_takendown: Option<bool>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub did: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(rename = "accessJwt")]
    pub access_jwt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "emailAuthFactor")]
    pub email_auth_factor: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "emailConfirmed")]
    pub email_confirmed: Option<bool>,
    /// If active=false, this optional field indicates a possible reason for why the account is not active. If active=false and no status is supplied, then the host makes no claim for why the repository is no longer being hosted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub handle: String,
    #[serde(rename = "refreshJwt")]
    pub refresh_jwt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "didDoc")]
    pub did_doc: Option<serde_json::Value>,
}

/// Create an authentication session.
pub async fn create_session(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.server.createSession").data(&input)?;

    client.request(req).await
}
