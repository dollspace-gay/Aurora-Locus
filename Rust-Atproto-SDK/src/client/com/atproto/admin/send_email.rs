//! Generated code for com.atproto.admin.sendEmail
//!
//! Send email to a user's account email address.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    #[serde(rename = "senderDid")]
    pub sender_did: crate::types::Did,
    /// Additional comment by the sender that won't be used in the email itself but helpful to provide more context for moderators/reviewers
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(rename = "recipientDid")]
    pub recipient_did: crate::types::Did,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub sent: bool,
}

/// Send email to a user's account email address.
pub async fn send_email(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("com.atproto.admin.sendEmail").data(&input)?;

    client.request(req).await
}
