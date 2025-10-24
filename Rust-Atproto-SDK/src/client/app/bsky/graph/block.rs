//! Generated code for app.bsky.graph.block
//!
//! Record declaring a 'block' relationship against another account. NOTE: blocks are public in Bluesky; see blog posts for details.

use serde::{Deserialize, Serialize};

/// Record declaring a 'block' relationship against another account. NOTE: blocks are public in Bluesky; see blog posts for details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    /// DID of the account to be blocked.
    pub subject: crate::types::Did,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

