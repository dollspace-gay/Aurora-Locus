//! Generated code for tools.ozone.moderation.queryEvents
//!
//! List moderation events related to a subject.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// Sort direction for the events. Defaults to descending order of created at timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "sortDirection")]
    pub sort_direction: Option<String>,
    /// Retrieve events created after a given timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdAfter")]
    pub created_after: Option<String>,
    /// The types of events (fully qualified string in the format of tools.ozone.moderation.defs#modEvent<name>) to filter by. If not specified, all events are returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub types: Option<serde_json::Value>,
    /// If specified, only events where all of these tags were added are returned
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "addedTags")]
    pub added_tags: Option<serde_json::Value>,
    /// If specified, only events where all of these tags were removed are returned
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "removedTags")]
    pub removed_tags: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "reportTypes")]
    pub report_types: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policies: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    /// If specified, only events where the modTool name matches any of the given values are returned
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "modTool")]
    pub mod_tool: Option<serde_json::Value>,
    /// If specified, only events where the subject is of the given type (account or record) will be returned. When this is set to 'account' the 'collections' parameter will be ignored. When includeAllUserRecords or subject is set, this will be ignored.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "subjectType")]
    pub subject_type: Option<String>,
    /// If true, events on all record types (posts, lists, profile etc.) or records from given 'collections' param, owned by the did are returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "includeAllUserRecords")]
    pub include_all_user_records: Option<bool>,
    /// If specified, only events where the batchId matches the given value are returned
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "batchId")]
    pub batch_id: Option<String>,
    /// If specified, only events where the age assurance state matches the given value are returned
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ageAssuranceState")]
    pub age_assurance_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// If specified, only events where all of these labels were removed are returned
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "removedLabels")]
    pub removed_labels: Option<serde_json::Value>,
    /// If specified, only events where all of these labels were added are returned
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "addedLabels")]
    pub added_labels: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdBy")]
    pub created_by: Option<crate::types::Did>,
    /// If specified, only events where the subject belongs to the given collections will be returned. When subjectType is set to 'account', this will be ignored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collections: Option<serde_json::Value>,
    /// If specified, only events with comments containing the keyword are returned. Apply || separator to use multiple keywords and match using OR condition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// If true, only events with comments are returned
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hasComment")]
    pub has_comment: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Retrieve events created before a given timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdBefore")]
    pub created_before: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    pub events: serde_json::Value,
}

/// List moderation events related to a subject.
pub async fn query_events(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("tools.ozone.moderation.queryEvents");

    // Add query parameters
    let params_json = serde_json::to_value(&params)
        .map_err(XrpcError::Serialization)?;

    if let Some(obj) = params_json.as_object() {
        for (key, value) in obj {
            if let Some(s) = value.as_str() {
                req.params.insert(key.clone(), s.to_string());
            } else {
                req.params.insert(key.clone(), value.to_string());
            }
        }
    }

    client.request(req).await
}
