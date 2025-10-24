//! Generated code for app.bsky.graph.follow
//!
//! Record declaring a social 'follow' relationship of another account. Duplicate follows will be ignored by the AppView.

use serde::{Deserialize, Serialize};

/// Record declaring a social 'follow' relationship of another account. Duplicate follows will be ignored by the AppView.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Follow {
    pub subject: crate::types::Did,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

