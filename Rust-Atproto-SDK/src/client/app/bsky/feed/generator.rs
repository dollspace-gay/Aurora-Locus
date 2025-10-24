//! Generated code for app.bsky.feed.generator
//!
//! Record declaring of the existence of a feed generator, and containing metadata about it. The record can exist in any repository.

use serde::{Deserialize, Serialize};

/// Record declaring of the existence of a feed generator, and containing metadata about it. The record can exist in any repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Generator {
    pub did: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<serde_json::Value>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "descriptionFacets")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_facets: Option<serde_json::Value>,
    /// Declaration that a feed accepts feedback interactions from a client through app.bsky.feed.sendInteractions
    #[serde(rename = "acceptsInteractions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepts_interactions: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: String,
    /// Self-label values
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    #[serde(rename = "contentMode")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_mode: Option<String>,
}

