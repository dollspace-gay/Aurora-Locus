//! Generated code for app.bsky.graph.starterpack
//!
//! Record defining a starter pack of actors and feeds for new users.

use serde::{Deserialize, Serialize};

/// Record defining a starter pack of actors and feeds for new users.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Starterpack {
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// Display name for starter pack; can not be empty.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Reference (AT-URI) to the list record.
    pub list: crate::syntax::AtUri,
    #[serde(rename = "descriptionFacets")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_facets: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feeds: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedItem {
    pub uri: crate::syntax::AtUri,
}

