//! Generated code for app.bsky.graph.listitem
//!
//! Record representing an account's inclusion on a specific list. The AppView will ignore duplicate listitem records.

use serde::{Deserialize, Serialize};

/// Record representing an account's inclusion on a specific list. The AppView will ignore duplicate listitem records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Listitem {
    /// Reference (AT-URI) to the list record (app.bsky.graph.list).
    pub list: crate::syntax::AtUri,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// The account which is included on the list.
    pub subject: crate::types::Did,
}

