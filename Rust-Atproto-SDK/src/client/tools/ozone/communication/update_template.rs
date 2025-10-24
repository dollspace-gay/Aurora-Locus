//! Generated code for tools.ozone.communication.updateTemplate
//!
//! Administrative action to update an existing communication template. Allows passing partial fields to patch specific fields only.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// Content of the template, markdown supported, can contain variable placeholders.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "contentMarkdown")]
    pub content_markdown: Option<String>,
    /// Subject of the message, used in emails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// DID of the user who is updating the template.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "updatedBy")]
    pub updated_by: Option<crate::types::Did>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    /// ID of the template to be updated.
    pub id: String,
    /// Name of the template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Message language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Administrative action to update an existing communication template. Allows passing partial fields to patch specific fields only.
pub async fn update_template(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.communication.updateTemplate").data(&input)?;

    client.request(req).await
}
