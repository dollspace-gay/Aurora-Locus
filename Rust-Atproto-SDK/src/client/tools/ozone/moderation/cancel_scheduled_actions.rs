//! Generated code for tools.ozone.moderation.cancelScheduledActions
//!
//! Cancel all pending scheduled moderation actions for specified subjects

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// Array of DID subjects to cancel scheduled actions for
    pub subjects: serde_json::Value,
    /// Optional comment describing the reason for cancellation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Cancel all pending scheduled moderation actions for specified subjects
pub async fn cancel_scheduled_actions(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.moderation.cancelScheduledActions").data(&input)?;

    client.request(req).await
}
