//! Generated type definitions for tools.ozone.team.defs

use serde::{Deserialize, Serialize};

/// Moderator role. Can perform most actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleModerator;

/// Verifier role. Only allowed to issue verifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleVerifier;

/// Admin role. Highest level of access, can perform all actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleAdmin;

/// Triage role. Mostly intended for monitoring and escalating issues.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleTriage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "lastUpdatedBy")]
    pub last_updated_by: Option<String>,
    pub did: crate::types::Did,
}


