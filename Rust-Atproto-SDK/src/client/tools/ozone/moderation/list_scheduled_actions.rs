//! Generated code for tools.ozone.moderation.listScheduledActions
//!
//! List scheduled moderation actions with optional filtering

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// Maximum number of results to return
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    /// Filter actions by status
    pub statuses: serde_json::Value,
    /// Cursor for pagination
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Filter actions scheduled to execute before this time
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "endsBefore")]
    pub ends_before: Option<String>,
    /// Filter actions for specific DID subjects
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subjects: Option<serde_json::Value>,
    /// Filter actions scheduled to execute after this time
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "startsAfter")]
    pub starts_after: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub actions: serde_json::Value,
    /// Cursor for next page of results
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// List scheduled moderation actions with optional filtering
pub async fn list_scheduled_actions(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.moderation.listScheduledActions").data(&input)?;

    client.request(req).await
}
