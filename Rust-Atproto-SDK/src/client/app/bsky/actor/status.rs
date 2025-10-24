//! Generated code for app.bsky.actor.status
//!
//! A declaration of a Bluesky account status.

use serde::{Deserialize, Serialize};

/// A declaration of a Bluesky account status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// The duration of the status in minutes. Applications can choose to impose minimum and maximum limits.
    #[serde(rename = "durationMinutes")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_minutes: Option<i64>,
    /// The status for the account.
    pub status: String,
    /// An optional embed associated with the status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed: Option<serde_json::Value>,
}

