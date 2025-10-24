//! Generated type definitions for chat.bsky.actor.defs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileViewBasic {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub associated: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer: Option<serde_json::Value>,
    /// Set to true when the actor cannot actively participate in conversations
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "chatDisabled")]
    pub chat_disabled: Option<bool>,
    pub did: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<serde_json::Value>,
    pub handle: String,
}


