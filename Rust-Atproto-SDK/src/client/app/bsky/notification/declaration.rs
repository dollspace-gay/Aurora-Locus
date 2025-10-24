//! Generated code for app.bsky.notification.declaration
//!
//! A declaration of the user's choices related to notifications that can be produced by them.

use serde::{Deserialize, Serialize};

/// A declaration of the user's choices related to notifications that can be produced by them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Declaration {
    /// A declaration of the user's preference for allowing activity subscriptions from other users. Absence of a record implies 'followers'.
    #[serde(rename = "allowSubscriptions")]
    pub allow_subscriptions: String,
}

