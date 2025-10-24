//! Generated code for app.bsky.graph.list
//!
//! Record representing a list of accounts (actors). Scope includes both moderation-oriented lists and curration-oriented lists.

use serde::{Deserialize, Serialize};

/// Record representing a list of accounts (actors). Scope includes both moderation-oriented lists and curration-oriented lists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct List {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "descriptionFacets")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_facets: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// Display name for list; can not be empty.
    pub name: String,
    /// Defines the purpose of the list (aka, moderation-oriented or curration-oriented)
    pub purpose: serde_json::Value,
}

