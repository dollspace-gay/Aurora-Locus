//! Generated type definitions for com.atproto.server.defs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteCodeUse {
    #[serde(rename = "usedBy")]
    pub used_by: crate::types::Did,
    #[serde(rename = "usedAt")]
    pub used_at: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteCode {
    #[serde(rename = "createdBy")]
    pub created_by: String,
    #[serde(rename = "forAccount")]
    pub for_account: String,
    pub available: i64,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    pub uses: serde_json::Value,
    pub disabled: bool,
    pub code: String,
}


