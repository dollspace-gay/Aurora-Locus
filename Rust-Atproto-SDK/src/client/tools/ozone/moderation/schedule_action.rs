//! Generated code for tools.ozone.moderation.scheduleAction
//!
//! Schedule a moderation action to be executed at a future time

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub scheduling: serde_json::Value,
    #[serde(rename = "createdBy")]
    pub created_by: crate::types::Did,
    /// Array of DID subjects to schedule the action for
    pub subjects: serde_json::Value,
    /// This will be propagated to the moderation event when it is applied
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "modTool")]
    pub mod_tool: Option<serde_json::Value>,
    pub action: serde_json::Value,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Schedule a moderation action to be executed at a future time
pub async fn schedule_action(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.moderation.scheduleAction").data(&input)?;

    client.request(req).await
}
