//! Generated type definitions for tools.ozone.communication.defs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateView {
    /// Name of the template.
    pub name: String,
    /// Message language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    pub disabled: bool,
    /// Subject of the message, used in emails.
    #[serde(rename = "contentMarkdown")]
    pub content_markdown: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// DID of the user who last updated the template.
    #[serde(rename = "lastUpdatedBy")]
    pub last_updated_by: crate::types::Did,
    /// Content of the template, can contain markdown and variable placeholders.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub id: String,
}


