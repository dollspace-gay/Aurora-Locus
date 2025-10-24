//! Generated code for app.bsky.labeler.service
//!
//! A declaration of the existence of labeler service.

use serde::{Deserialize, Serialize};

/// A declaration of the existence of labeler service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    /// The set of report reason 'codes' which are in-scope for this service to review and action. These usually align to policy categories. If not defined (distinct from empty array), all reason types are allowed.
    #[serde(rename = "reasonTypes")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_types: Option<serde_json::Value>,
    pub policies: serde_json::Value,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// The set of subject types (account, record, etc) this service accepts reports on.
    #[serde(rename = "subjectTypes")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_types: Option<serde_json::Value>,
    /// Set of record types (collection NSIDs) which can be reported to this service. If not defined (distinct from empty array), default is any record type.
    #[serde(rename = "subjectCollections")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_collections: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
}

