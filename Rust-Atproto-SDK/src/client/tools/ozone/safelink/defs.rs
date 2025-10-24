//! Generated type definitions for tools.ozone.safelink.defs

use serde::{Deserialize, Serialize};

pub type ReasonType = String;

/// Input for creating a URL safety rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlRule {
    /// Optional comment about the decision
    /// Input for creating a URL safety rule
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// The URL or domain to apply the rule to
    /// Input for creating a URL safety rule
    pub url: String,
    /// Input for creating a URL safety rule
    pub reason: serde_json::Value,
    /// DID of the user added the rule.
    /// Input for creating a URL safety rule
    #[serde(rename = "createdBy")]
    pub created_by: crate::types::Did,
    /// Input for creating a URL safety rule
    pub pattern: serde_json::Value,
    /// Timestamp when the rule was last updated
    /// Input for creating a URL safety rule
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    /// Input for creating a URL safety rule
    pub action: serde_json::Value,
    /// Timestamp when the rule was created
    /// Input for creating a URL safety rule
    #[serde(rename = "createdAt")]
    pub created_at: String,
}


pub type EventType = String;

pub type ActionType = String;

/// An event for URL safety decisions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// The URL that this rule applies to
    /// An event for URL safety decisions
    pub url: String,
    /// Auto-incrementing row ID
    /// An event for URL safety decisions
    pub id: i64,
    /// DID of the user who created this rule
    /// An event for URL safety decisions
    #[serde(rename = "createdBy")]
    pub created_by: crate::types::Did,
    /// An event for URL safety decisions
    pub action: serde_json::Value,
    /// An event for URL safety decisions
    pub reason: serde_json::Value,
    /// An event for URL safety decisions
    pub pattern: serde_json::Value,
    /// An event for URL safety decisions
    #[serde(rename = "eventType")]
    pub event_type: serde_json::Value,
    /// An event for URL safety decisions
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// Optional comment about the decision
    /// An event for URL safety decisions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}


pub type PatternType = String;

