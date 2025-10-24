//! Generated code for chat.bsky.actor.declaration
//!
//! A declaration of a Bluesky chat account.

use serde::{Deserialize, Serialize};

/// A declaration of a Bluesky chat account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Declaration {
    #[serde(rename = "allowIncoming")]
    pub allow_incoming: String,
}

