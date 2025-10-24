//! Generated code for tools.ozone.moderation.queryStatuses
//!
//! View moderation statuses of subjects (record or repo).

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// Search subjects where the associated record/account was updated before a given timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hostingUpdatedBefore")]
    pub hosting_updated_before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "sortField")]
    pub sort_field: Option<String>,
    /// If specified, only subjects that belong to an account that has at least this many reported records will be returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "minReportedRecordsCount")]
    pub min_reported_records_count: Option<i64>,
    /// Search subjects reported before a given timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "reportedBefore")]
    pub reported_before: Option<String>,
    /// Get subjects that were taken down
    #[serde(skip_serializing_if = "Option::is_none")]
    pub takendown: Option<bool>,
    /// All subjects, or subjects from given 'collections' param, belonging to the account specified in the 'subject' param will be returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "includeAllUserRecords")]
    pub include_all_user_records: Option<bool>,
    /// If specified, only subjects that belong to an account that has at least this many suspensions will be returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "minAccountSuspendCount")]
    pub min_account_suspend_count: Option<i64>,
    /// A seeder to shuffle/balance the queue items.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "queueSeed")]
    pub queue_seed: Option<String>,
    /// Index of the queue to fetch subjects from. Works only when queueCount value is specified.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "queueIndex")]
    pub queue_index: Option<i64>,
    /// The subject to get the status for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// By default, we don't include muted subjects in the results. Set this to true to include them.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "includeMuted")]
    pub include_muted: Option<bool>,
    /// If specified, subjects of the given type (account or record) will be returned. When this is set to 'account' the 'collections' parameter will be ignored. When includeAllUserRecords or subject is set, this will be ignored.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "subjectType")]
    pub subject_type: Option<String>,
    /// If specified, only subjects that belong to an account that has at least this many taken down records will be returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "minTakendownRecordsCount")]
    pub min_takendown_records_count: Option<i64>,
    /// Number of queues being used by moderators. Subjects will be split among all queues.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "queueCount")]
    pub queue_count: Option<i64>,
    /// Get subjects in unresolved appealed status
    #[serde(skip_serializing_if = "Option::is_none")]
    pub appealed: Option<bool>,
    /// Search subjects where the associated record/account was deleted after a given timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hostingDeletedAfter")]
    pub hosting_deleted_after: Option<String>,
    /// Search subjects by keyword from comments
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// If specified, only subjects that have priority score value above the given value will be returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "minPriorityScore")]
    pub min_priority_score: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Get all subject statuses that were reviewed by a specific moderator
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "lastReviewedBy")]
    pub last_reviewed_by: Option<crate::types::Did>,
    /// Search subjects where the associated record/account was updated after a given timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hostingUpdatedAfter")]
    pub hosting_updated_after: Option<String>,
    /// When set to true, only muted subjects and reporters will be returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "onlyMuted")]
    pub only_muted: Option<bool>,
    /// Search subjects reported after a given timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "reportedAfter")]
    pub reported_after: Option<String>,
    /// Specify when fetching subjects in a certain state
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "reviewState")]
    pub review_state: Option<String>,
    /// Search subjects reviewed after a given timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "reviewedAfter")]
    pub reviewed_after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<serde_json::Value>,
    /// If specified, subjects belonging to the given collections will be returned. When subjectType is set to 'account', this will be ignored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collections: Option<serde_json::Value>,
    /// If specified, only subjects with the given age assurance state will be returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ageAssuranceState")]
    pub age_assurance_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "excludeTags")]
    pub exclude_tags: Option<serde_json::Value>,
    /// Search subjects by the status of the associated record/account
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hostingStatuses")]
    pub hosting_statuses: Option<serde_json::Value>,
    /// Search subjects where the associated record/account was deleted before a given timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hostingDeletedBefore")]
    pub hosting_deleted_before: Option<String>,
    /// Search subjects reviewed before a given timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "reviewedBefore")]
    pub reviewed_before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ignoreSubjects")]
    pub ignore_subjects: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "sortDirection")]
    pub sort_direction: Option<String>,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    #[serde(rename = "subjectStatuses")]
    pub subject_statuses: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// View moderation statuses of subjects (record or repo).
pub async fn query_statuses(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("tools.ozone.moderation.queryStatuses");

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
