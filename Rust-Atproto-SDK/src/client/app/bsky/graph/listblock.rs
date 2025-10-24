//! Generated code for app.bsky.graph.listblock
//!
//! Record representing a block relationship against an entire an entire list of accounts (actors).

use serde::{Deserialize, Serialize};

/// Record representing a block relationship against an entire an entire list of accounts (actors).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Listblock {
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// Reference (AT-URI) to the mod list record.
    pub subject: crate::syntax::AtUri,
}

