//! Generated type definitions for com.atproto.admin.defs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountView {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "invitedBy")]
    pub invited_by: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "threatSignatures")]
    pub threat_signatures: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "emailConfirmedAt")]
    pub email_confirmed_at: Option<String>,
    pub did: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "deactivatedAt")]
    pub deactivated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(rename = "indexedAt")]
    pub indexed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "relatedRecords")]
    pub related_records: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "inviteNote")]
    pub invite_note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invites: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "invitesDisabled")]
    pub invites_disabled: Option<bool>,
    pub handle: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoRef {
    pub did: crate::types::Did,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoBlobRef {
    pub cid: String,
    pub did: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "recordUri")]
    pub record_uri: Option<crate::syntax::AtUri>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreatSignature {
    pub property: String,
    pub value: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusAttr {
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ref")]
    pub r#ref: Option<String>,
}


