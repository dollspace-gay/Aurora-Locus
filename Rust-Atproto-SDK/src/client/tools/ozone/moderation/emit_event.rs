//! Generated code for tools.ozone.moderation.emitEvent
//!
//! Take a moderation action on an actor.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub subject: serde_json::Value,
    pub event: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "subjectBlobCids")]
    pub subject_blob_cids: Option<serde_json::Value>,
    #[serde(rename = "createdBy")]
    pub created_by: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "modTool")]
    pub mod_tool: Option<serde_json::Value>,
    /// An optional external ID for the event, used to deduplicate events from external systems. Fails when an event of same type with the same external ID exists for the same subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "externalId")]
    pub external_id: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Take a moderation action on an actor.
pub async fn emit_event(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.moderation.emitEvent").data(&input)?;

    client.request(req).await
}
