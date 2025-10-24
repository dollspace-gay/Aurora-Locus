//! Generated code for app.bsky.actor.profile
//!
//! A declaration of a Bluesky account profile.

use serde::{Deserialize, Serialize};

/// A declaration of a Bluesky account profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(rename = "displayName")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(rename = "pinnedPost")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned_post: Option<serde_json::Value>,
    /// Self-label values, specific to the Bluesky application, on the overall account.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(rename = "joinedViaStarterPack")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub joined_via_starter_pack: Option<serde_json::Value>,
    /// Free-form profile description text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Larger horizontal image to display behind profile view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banner: Option<serde_json::Value>,
    /// Free-form pronouns text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
    #[serde(rename = "createdAt")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// Small image to be displayed next to posts from account. AKA, 'profile picture'
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<serde_json::Value>,
}

