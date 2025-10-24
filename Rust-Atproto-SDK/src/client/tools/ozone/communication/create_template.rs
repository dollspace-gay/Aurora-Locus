//! Generated code for tools.ozone.communication.createTemplate
//!
//! Administrative action to create a new, re-usable communication (email for now) template.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Request input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    /// Subject of the message, used in emails.
    pub subject: String,
    /// Message language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// DID of the user who is creating the template.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdBy")]
    pub created_by: Option<crate::types::Did>,
    /// Content of the template, markdown supported, can contain variable placeholders.
    #[serde(rename = "contentMarkdown")]
    pub content_markdown: String,
    /// Name of the template.
    pub name: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
}

/// Administrative action to create a new, re-usable communication (email for now) template.
pub async fn create_template(
    client: &impl XrpcClient,
    input: Input,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let req = XrpcRequest::procedure("tools.ozone.communication.createTemplate").data(&input)?;

    client.request(req).await
}
