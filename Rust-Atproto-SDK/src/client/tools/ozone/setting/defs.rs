//! Generated type definitions for tools.ozone.setting.defs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionSetting {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "managerRole")]
    pub manager_role: Option<String>,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
    pub scope: String,
    #[serde(rename = "createdBy")]
    pub created_by: crate::types::Did,
    pub did: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(rename = "lastUpdatedBy")]
    pub last_updated_by: crate::types::Did,
    pub value: serde_json::Value,
}


