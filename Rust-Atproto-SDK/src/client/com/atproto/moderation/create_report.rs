//! Generated code for com.atproto.moderation.createReport
//!
//! Submit a moderation report regarding an atproto account or record. Implemented by moderation services (with PDS proxying), and requires auth.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub subject: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "modTool")]
    pub mod_tool: Option<serde_json::Value>,
    /// Indicates the broad category of violation the report is for.
    #[serde(rename = "reasonType")]
    pub reason_type: serde_json::Value,
    /// Additional context about the content and violation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(rename = "reasonType")]
    pub reason_type: serde_json::Value,
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(rename = "reportedBy")]
    pub reported_by: crate::types::Did,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    pub subject: serde_json::Value,
}

/// Submit a moderation report regarding an atproto account or record. Implemented by moderation services (with PDS proxying), and requires auth.
pub async fn create_report(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.moderation.createReport").data(&input)?;

    client.request(req).await
}
