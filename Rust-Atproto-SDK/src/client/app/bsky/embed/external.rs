//! Generated type definitions for app.bsky.embed.external

use serde::{Deserialize, Serialize};

/// A representation of some externally linked content (eg, a URL and 'card'), embedded in a Bluesky record (eg, a post).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Main {
    /// A representation of some externally linked content (eg, a URL and 'card'), embedded in a Bluesky record (eg, a post).
    pub external: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct External {
    pub description: String,
    pub uri: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb: Option<serde_json::Value>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct View {
    pub external: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewExternal {
    pub uri: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
    pub description: String,
}


