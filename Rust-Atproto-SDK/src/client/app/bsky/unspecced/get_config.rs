//! Generated code for app.bsky.unspecced.getConfig
//!
//! Get miscellaneous runtime configuration.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "liveNow")]
    pub live_now: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "checkEmailConfirmed")]
    pub check_email_confirmed: Option<bool>,
}

/// Get miscellaneous runtime configuration.
pub async fn get_config(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::query("app.bsky.unspecced.getConfig");

    client.request(req).await
}
