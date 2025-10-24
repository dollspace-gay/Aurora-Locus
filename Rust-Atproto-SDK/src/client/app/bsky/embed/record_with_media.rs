//! Generated type definitions for app.bsky.embed.recordWithMedia

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Main {
    pub record: serde_json::Value,
    pub media: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct View {
    pub record: serde_json::Value,
    pub media: serde_json::Value,
}


