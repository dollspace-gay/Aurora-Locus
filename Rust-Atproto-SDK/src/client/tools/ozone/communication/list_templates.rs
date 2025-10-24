//! Generated code for tools.ozone.communication.listTemplates
//!
//! Get list of all communication templates.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(rename = "communicationTemplates")]
    pub communication_templates: serde_json::Value,
}

/// Get list of all communication templates.
pub async fn list_templates(
    client: &impl XrpcClient,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::query("tools.ozone.communication.listTemplates");

    client.request(req).await
}
