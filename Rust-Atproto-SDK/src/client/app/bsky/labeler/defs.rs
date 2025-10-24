//! Generated type definitions for app.bsky.labeler.defs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelerPolicies {
    /// The label values which this labeler publishes. May include global or custom labels.
    #[serde(rename = "labelValues")]
    pub label_values: serde_json::Value,
    /// Label values created by this labeler and scoped exclusively to it. Labels defined here will override global label definitions for this labeler.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "labelValueDefinitions")]
    pub label_value_definitions: Option<serde_json::Value>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelerViewDetailed {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "likeCount")]
    pub like_count: Option<i64>,
    pub creator: serde_json::Value,
    pub policies: serde_json::Value,
    /// The set of subject types (account, record, etc) this service accepts reports on.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "subjectTypes")]
    pub subject_types: Option<serde_json::Value>,
    /// Set of record types (collection NSIDs) which can be reported to this service. If not defined (distinct from empty array), default is any record type.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "subjectCollections")]
    pub subject_collections: Option<serde_json::Value>,
    #[serde(rename = "indexedAt")]
    pub indexed_at: String,
    pub uri: crate::syntax::AtUri,
    pub cid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    /// The set of report reason 'codes' which are in-scope for this service to review and action. These usually align to policy categories. If not defined (distinct from empty array), all reason types are allowed.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "reasonTypes")]
    pub reason_types: Option<serde_json::Value>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelerView {
    #[serde(rename = "indexedAt")]
    pub indexed_at: String,
    pub uri: crate::syntax::AtUri,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "likeCount")]
    pub like_count: Option<i64>,
    pub cid: String,
    pub creator: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer: Option<serde_json::Value>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelerViewerState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub like: Option<crate::syntax::AtUri>,
}


