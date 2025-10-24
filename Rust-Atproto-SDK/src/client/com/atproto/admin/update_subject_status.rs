//! Generated code for com.atproto.admin.updateSubjectStatus
//!
//! Update the service-specific admin status of a subject (account, record, or blob).

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub subject: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub takedown: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deactivated: Option<serde_json::Value>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub takedown: Option<serde_json::Value>,
    pub subject: serde_json::Value,
}

/// Update the service-specific admin status of a subject (account, record, or blob).
pub async fn update_subject_status(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.admin.updateSubjectStatus").data(&input)?;

    client.request(req).await
}
