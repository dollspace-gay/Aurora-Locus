//! Generated type definitions for tools.ozone.moderation.defs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoDetails {
    pub width: i64,
    pub length: i64,
    pub height: i64,
}


/// Moderation event timeline event for a PLC create operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEventPlcCreate;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventViewDetail {
    pub subject: serde_json::Value,
    pub id: i64,
    pub event: serde_json::Value,
    #[serde(rename = "createdBy")]
    pub created_by: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "modTool")]
    pub mod_tool: Option<serde_json::Value>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "subjectBlobs")]
    pub subject_blobs: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordViewDetail {
    pub cid: String,
    pub repo: serde_json::Value,
    pub blobs: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    pub value: serde_json::Value,
    #[serde(rename = "indexedAt")]
    pub indexed_at: String,
    pub moderation: serde_json::Value,
    pub uri: crate::syntax::AtUri,
}


/// Add/Remove a tag on a subject
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventTag {
    /// Additional comment about added/removed tags.
    /// Add/Remove a tag on a subject
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Tags to be removed to the subject. Ignores a tag If it doesn't exist, won't be duplicated.
    /// Add/Remove a tag on a subject
    pub remove: serde_json::Value,
    /// Tags to be added to the subject. If already exists, won't be duplicated.
    /// Add/Remove a tag on a subject
    pub add: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountHosting {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "deactivatedAt")]
    pub deactivated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "deletedAt")]
    pub deleted_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "reactivatedAt")]
    pub reactivated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventAcknowledge {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// If true, all other reports on content authored by this account will be resolved (acknowledged).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "acknowledgeAccountSubjects")]
    pub acknowledge_account_subjects: Option<bool>,
}


/// Logs lifecycle event on a record subject. Normally captured by automod from the firehose and emitted to ozone for historical tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordEvent {
    /// Logs lifecycle event on a record subject. Normally captured by automod from the firehose and emitted to ozone for historical tracking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    /// Logs lifecycle event on a record subject. Normally captured by automod from the firehose and emitted to ozone for historical tracking.
    pub timestamp: String,
    /// Logs lifecycle event on a record subject. Normally captured by automod from the firehose and emitted to ozone for historical tracking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Logs lifecycle event on a record subject. Normally captured by automod from the firehose and emitted to ozone for historical tracking.
    pub op: String,
}


/// Moderator review status of a subject: Open. Indicates that the subject needs to be reviewed by a moderator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewOpen;

/// Age assurance status override by moderators. Only works on DID subjects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgeAssuranceOverrideEvent {
    /// Comment describing the reason for the override.
    /// Age assurance status override by moderators. Only works on DID subjects.
    pub comment: String,
    /// The status to be set for the user decided by a moderator, overriding whatever value the user had previously. Use reset to default to original state.
    /// Age assurance status override by moderators. Only works on DID subjects.
    pub status: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoViewDetail {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "inviteNote")]
    pub invite_note: Option<String>,
    #[serde(rename = "relatedRecords")]
    pub related_records: serde_json::Value,
    pub did: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "invitedBy")]
    pub invited_by: Option<serde_json::Value>,
    #[serde(rename = "indexedAt")]
    pub indexed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invites: Option<serde_json::Value>,
    pub handle: String,
    pub moderation: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "deactivatedAt")]
    pub deactivated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "invitesDisabled")]
    pub invites_disabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "emailConfirmedAt")]
    pub email_confirmed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "threatSignatures")]
    pub threat_signatures: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobView {
    pub size: i64,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moderation: Option<serde_json::Value>,
    pub cid: String,
}


/// View of a scheduled moderation action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledActionView {
    /// Serialized event object that will be propagated to the event when performed
    /// View of a scheduled moderation action
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "eventData")]
    pub event_data: Option<serde_json::Value>,
    /// Earliest time to execute the action (for randomized scheduling)
    /// View of a scheduled moderation action
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "executeAfter")]
    pub execute_after: Option<String>,
    /// Whether execution time should be randomized within the specified range
    /// View of a scheduled moderation action
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "randomizeExecution")]
    pub randomize_execution: Option<bool>,
    /// Subject DID for the action
    /// View of a scheduled moderation action
    pub did: crate::types::Did,
    /// Current status of the scheduled action
    /// View of a scheduled moderation action
    pub status: String,
    /// Auto-incrementing row ID
    /// View of a scheduled moderation action
    pub id: i64,
    /// When the scheduled action was created
    /// View of a scheduled moderation action
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// Type of action to be executed
    /// View of a scheduled moderation action
    pub action: String,
    /// Latest time to execute the action (for randomized scheduling)
    /// View of a scheduled moderation action
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "executeUntil")]
    pub execute_until: Option<String>,
    /// Reason for the last execution failure
    /// View of a scheduled moderation action
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "lastFailureReason")]
    pub last_failure_reason: Option<String>,
    /// Exact time to execute the action
    /// View of a scheduled moderation action
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "executeAt")]
    pub execute_at: Option<String>,
    /// When the action was last attempted to be executed
    /// View of a scheduled moderation action
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "lastExecutedAt")]
    pub last_executed_at: Option<String>,
    /// When the scheduled action was last updated
    /// View of a scheduled moderation action
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
    /// DID of the user who created this scheduled action
    /// View of a scheduled moderation action
    #[serde(rename = "createdBy")]
    pub created_by: crate::types::Did,
    /// ID of the moderation event created when action was successfully executed
    /// View of a scheduled moderation action
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "executionEventId")]
    pub execution_event_id: Option<i64>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoViewNotFound {
    pub did: crate::types::Did,
}


/// Age assurance info coming directly from users. Only works on DID subjects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgeAssuranceEvent {
    /// The date and time of this write operation.
    /// Age assurance info coming directly from users. Only works on DID subjects.
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// The status of the age assurance process.
    /// Age assurance info coming directly from users. Only works on DID subjects.
    pub status: String,
    /// The IP address used when initiating the AA flow.
    /// Age assurance info coming directly from users. Only works on DID subjects.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "initIp")]
    pub init_ip: Option<String>,
    /// The user agent used when initiating the AA flow.
    /// Age assurance info coming directly from users. Only works on DID subjects.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "initUa")]
    pub init_ua: Option<String>,
    /// The IP address used when completing the AA flow.
    /// Age assurance info coming directly from users. Only works on DID subjects.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "completeIp")]
    pub complete_ip: Option<String>,
    /// The unique identifier for this instance of the age assurance flow, in UUID format.
    /// Age assurance info coming directly from users. Only works on DID subjects.
    #[serde(rename = "attemptId")]
    pub attempt_id: String,
    /// The user agent used when completing the AA flow.
    /// Age assurance info coming directly from users. Only works on DID subjects.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "completeUa")]
    pub complete_ua: Option<String>,
}


/// Detailed view of a subject. For record subjects, the author's repo and profile will be returned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubjectView {
    /// Detailed view of a subject. For record subjects, the author's repo and profile will be returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<serde_json::Value>,
    /// Detailed view of a subject. For record subjects, the author's repo and profile will be returned.
    #[serde(rename = "type")]
    pub r#type: serde_json::Value,
    /// Detailed view of a subject. For record subjects, the author's repo and profile will be returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<serde_json::Value>,
    /// Detailed view of a subject. For record subjects, the author's repo and profile will be returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<serde_json::Value>,
    /// Detailed view of a subject. For record subjects, the author's repo and profile will be returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<serde_json::Value>,
    /// Detailed view of a subject. For record subjects, the author's repo and profile will be returned.
    pub subject: String,
}


/// Report a subject
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventReport {
    /// Report a subject
    #[serde(rename = "reportType")]
    pub report_type: serde_json::Value,
    /// Report a subject
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Set to true if the reporter was muted from reporting at the time of the event. These reports won't impact the reviewState of the subject.
    /// Report a subject
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isReporterMuted")]
    pub is_reporter_muted: Option<bool>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReporterStats {
    /// The total number of reports made by the user on accounts.
    #[serde(rename = "accountReportCount")]
    pub account_report_count: i64,
    /// The total number of reports made by the user on records.
    #[serde(rename = "recordReportCount")]
    pub record_report_count: i64,
    /// The total number of records reported by the user.
    #[serde(rename = "reportedRecordCount")]
    pub reported_record_count: i64,
    /// The total number of accounts labeled as a result of the user's reports.
    #[serde(rename = "labeledAccountCount")]
    pub labeled_account_count: i64,
    /// The total number of records labeled as a result of the user's reports.
    #[serde(rename = "labeledRecordCount")]
    pub labeled_record_count: i64,
    /// The total number of accounts reported by the user.
    #[serde(rename = "reportedAccountCount")]
    pub reported_account_count: i64,
    /// The total number of accounts taken down as a result of the user's reports.
    #[serde(rename = "takendownAccountCount")]
    pub takendown_account_count: i64,
    pub did: crate::types::Did,
    /// The total number of records taken down as a result of the user's reports.
    #[serde(rename = "takendownRecordCount")]
    pub takendown_record_count: i64,
}


/// Moderation event timeline event for a PLC tombstone operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEventPlcTombstone;

/// Logs identity related events on a repo subject. Normally captured by automod from the firehose and emitted to ozone for historical tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityEvent {
    /// Logs identity related events on a repo subject. Normally captured by automod from the firehose and emitted to ozone for historical tracking.
    pub timestamp: String,
    /// Logs identity related events on a repo subject. Normally captured by automod from the firehose and emitted to ozone for historical tracking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Logs identity related events on a repo subject. Normally captured by automod from the firehose and emitted to ozone for historical tracking.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "pdsHost")]
    pub pds_host: Option<String>,
    /// Logs identity related events on a repo subject. Normally captured by automod from the firehose and emitted to ozone for historical tracking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tombstone: Option<bool>,
    /// Logs identity related events on a repo subject. Normally captured by automod from the firehose and emitted to ozone for historical tracking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
}


/// Unmute action on a subject
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventUnmute {
    /// Describe reasoning behind the reversal.
    /// Unmute action on a subject
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}


/// Take down a subject permanently or temporarily
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventTakedown {
    /// Indicates how long the takedown should be in effect before automatically expiring.
    /// Take down a subject permanently or temporarily
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "durationInHours")]
    pub duration_in_hours: Option<i64>,
    /// Take down a subject permanently or temporarily
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// If true, all other reports on content authored by this account will be resolved (acknowledged).
    /// Take down a subject permanently or temporarily
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "acknowledgeAccountSubjects")]
    pub acknowledge_account_subjects: Option<bool>,
    /// Names/Keywords of the policies that drove the decision.
    /// Take down a subject permanently or temporarily
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policies: Option<serde_json::Value>,
}


pub type SubjectReviewState = String;

/// Moderation tool information for tracing the source of the action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModTool {
    /// Name/identifier of the source (e.g., 'automod', 'ozone/workspace')
    /// Moderation tool information for tracing the source of the action
    pub name: String,
    /// Additional arbitrary metadata about the source
    /// Moderation tool information for tracing the source of the action
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModerationDetail {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "subjectStatus")]
    pub subject_status: Option<serde_json::Value>,
}


/// Unmute incoming reports from an account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventUnmuteReporter {
    /// Describe reasoning behind the reversal.
    /// Unmute incoming reports from an account
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}


/// Statistics about a particular account subject
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountStats {
    /// Number of times the account was taken down
    /// Statistics about a particular account subject
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "takedownCount")]
    pub takedown_count: Option<i64>,
    /// Total number of appeals against a moderation action on the account
    /// Statistics about a particular account subject
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "appealCount")]
    pub appeal_count: Option<i64>,
    /// Number of times the account was escalated
    /// Statistics about a particular account subject
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "escalateCount")]
    pub escalate_count: Option<i64>,
    /// Total number of reports on the account
    /// Statistics about a particular account subject
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "reportCount")]
    pub report_count: Option<i64>,
    /// Number of times the account was suspended
    /// Statistics about a particular account subject
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "suspendCount")]
    pub suspend_count: Option<i64>,
}


/// Moderation event timeline event for generic PLC operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEventPlcOperation;

/// Add a comment to a subject. An empty comment will clear any previously set sticky comment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventComment {
    /// Make the comment persistent on the subject
    /// Add a comment to a subject. An empty comment will clear any previously set sticky comment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sticky: Option<bool>,
    /// Add a comment to a subject. An empty comment will clear any previously set sticky comment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}


/// Mute incoming reports on a subject
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventMute {
    /// Mute incoming reports on a subject
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Indicates how long the subject should remain muted.
    /// Mute incoming reports on a subject
    #[serde(rename = "durationInHours")]
    pub duration_in_hours: i64,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoView {
    pub handle: String,
    pub did: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "invitedBy")]
    pub invited_by: Option<serde_json::Value>,
    #[serde(rename = "indexedAt")]
    pub indexed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "invitesDisabled")]
    pub invites_disabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "inviteNote")]
    pub invite_note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "threatSignatures")]
    pub threat_signatures: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "deactivatedAt")]
    pub deactivated_at: Option<String>,
    pub moderation: serde_json::Value,
    #[serde(rename = "relatedRecords")]
    pub related_records: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordView {
    #[serde(rename = "indexedAt")]
    pub indexed_at: String,
    pub moderation: serde_json::Value,
    pub repo: serde_json::Value,
    #[serde(rename = "blobCids")]
    pub blob_cids: serde_json::Value,
    pub cid: String,
    pub uri: crate::syntax::AtUri,
    pub value: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordHosting {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "deletedAt")]
    pub deleted_at: Option<String>,
}


/// Keep a log of outgoing email to a user
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventEmail {
    /// Additional comment about the outgoing comm.
    /// Keep a log of outgoing email to a user
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// The content of the email sent to the user.
    /// Keep a log of outgoing email to a user
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// The subject line of the email sent to the user.
    /// Keep a log of outgoing email to a user
    #[serde(rename = "subjectLine")]
    pub subject_line: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubjectStatusView {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "subjectBlobCids")]
    pub subject_blob_cids: Option<serde_json::Value>,
    #[serde(rename = "reviewState")]
    pub review_state: serde_json::Value,
    /// Numeric value representing the level of priority. Higher score means higher priority.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "priorityScore")]
    pub priority_score: Option<i64>,
    /// Timestamp referencing the first moderation status impacting event was emitted on the subject
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "muteUntil")]
    pub mute_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "muteReportingUntil")]
    pub mute_reporting_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "suspendUntil")]
    pub suspend_until: Option<String>,
    /// Statistics related to the record subjects authored by the subject's account
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "recordsStats")]
    pub records_stats: Option<serde_json::Value>,
    /// Whether or not the last successful update to age assurance was made by the user or admin.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ageAssuranceUpdatedBy")]
    pub age_assurance_updated_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "lastReviewedAt")]
    pub last_reviewed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "lastReportedAt")]
    pub last_reported_at: Option<String>,
    pub subject: serde_json::Value,
    /// Statistics related to the account subject
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "accountStats")]
    pub account_stats: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hosting: Option<serde_json::Value>,
    /// Timestamp referencing when the author of the subject appealed a moderation action
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "lastAppealedAt")]
    pub last_appealed_at: Option<String>,
    /// Timestamp referencing when the last update was made to the moderation status of the subject
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "lastReviewedBy")]
    pub last_reviewed_by: Option<crate::types::Did>,
    /// True indicates that the a previously taken moderator action was appealed against, by the author of the content. False indicates last appeal was resolved by moderators.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub appealed: Option<bool>,
    /// Sticky comment on the subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Current age assurance state of the subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ageAssuranceState")]
    pub age_assurance_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "subjectRepoHandle")]
    pub subject_repo_handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub takendown: Option<bool>,
}


/// Moderator review status of a subject: Closed. Indicates that the subject was already reviewed and resolved by a moderator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewClosed;

/// Set priority score of the subject. Higher score means higher priority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventPriorityScore {
    /// Set priority score of the subject. Higher score means higher priority.
    pub score: i64,
    /// Set priority score of the subject. Higher score means higher priority.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}


/// Mute incoming reports from an account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventMuteReporter {
    /// Mute incoming reports from an account
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Indicates how long the account should remain muted. Falsy value here means a permanent mute.
    /// Mute incoming reports from an account
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "durationInHours")]
    pub duration_in_hours: Option<i64>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Moderation {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "subjectStatus")]
    pub subject_status: Option<serde_json::Value>,
}


/// Moderator review status of a subject: Unnecessary. Indicates that the subject does not need a review at the moment but there is probably some moderation related metadata available for it
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewNone;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventView {
    pub id: i64,
    #[serde(rename = "subjectBlobCids")]
    pub subject_blob_cids: serde_json::Value,
    #[serde(rename = "createdBy")]
    pub created_by: crate::types::Did,
    pub subject: serde_json::Value,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "subjectHandle")]
    pub subject_handle: Option<String>,
    pub event: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "creatorHandle")]
    pub creator_handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "modTool")]
    pub mod_tool: Option<serde_json::Value>,
}


/// Revert take down action on a subject
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventReverseTakedown {
    /// Describe reasoning behind the reversal.
    /// Revert take down action on a subject
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}


/// Divert a record's blobs to a 3rd party service for further scanning/tagging
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventDivert {
    /// Divert a record's blobs to a 3rd party service for further scanning/tagging
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}


/// Statistics about a set of record subject items
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordsStats {
    /// Number of item currently in "reviewOpen" or "reviewEscalated" state
    /// Statistics about a set of record subject items
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "pendingCount")]
    pub pending_count: Option<i64>,
    /// Number of items that were reported at least once
    /// Statistics about a set of record subject items
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "reportedCount")]
    pub reported_count: Option<i64>,
    /// Number of item currently in "reviewNone" or "reviewClosed" state
    /// Statistics about a set of record subject items
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "processedCount")]
    pub processed_count: Option<i64>,
    /// Total number of item in the set
    /// Statistics about a set of record subject items
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "subjectCount")]
    pub subject_count: Option<i64>,
    /// Cumulative sum of the number of reports on the items in the set
    /// Statistics about a set of record subject items
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "totalReports")]
    pub total_reports: Option<i64>,
    /// Number of items that were escalated at least once
    /// Statistics about a set of record subject items
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "escalatedCount")]
    pub escalated_count: Option<i64>,
    /// Number of items that were appealed at least once
    /// Statistics about a set of record subject items
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "appealedCount")]
    pub appealed_count: Option<i64>,
    /// Number of item currently taken down
    /// Statistics about a set of record subject items
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "takendownCount")]
    pub takendown_count: Option<i64>,
}


/// Moderator review status of a subject: Escalated. Indicates that the subject was escalated for review by a moderator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewEscalated;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageDetails {
    pub height: i64,
    pub width: i64,
}


/// Account credentials revocation by moderators. Only works on DID subjects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokeAccountCredentialsEvent {
    /// Comment describing the reason for the revocation.
    /// Account credentials revocation by moderators. Only works on DID subjects.
    pub comment: String,
}


/// Resolve appeal on a subject
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventResolveAppeal {
    /// Describe resolution.
    /// Resolve appeal on a subject
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordViewNotFound {
    pub uri: crate::syntax::AtUri,
}


/// Apply/Negate labels on a subject
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventLabel {
    /// Apply/Negate labels on a subject
    #[serde(rename = "negateLabelVals")]
    pub negate_label_vals: serde_json::Value,
    /// Indicates how long the label will remain on the subject. Only applies on labels that are being added.
    /// Apply/Negate labels on a subject
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "durationInHours")]
    pub duration_in_hours: Option<i64>,
    /// Apply/Negate labels on a subject
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Apply/Negate labels on a subject
    #[serde(rename = "createLabelVals")]
    pub create_label_vals: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModEventEscalate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}


/// Logs account status related events on a repo subject. Normally captured by automod from the firehose and emitted to ozone for historical tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountEvent {
    /// Logs account status related events on a repo subject. Normally captured by automod from the firehose and emitted to ozone for historical tracking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Logs account status related events on a repo subject. Normally captured by automod from the firehose and emitted to ozone for historical tracking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Logs account status related events on a repo subject. Normally captured by automod from the firehose and emitted to ozone for historical tracking.
    pub timestamp: String,
    /// Indicates that the account has a repository which can be fetched from the host that emitted this event.
    /// Logs account status related events on a repo subject. Normally captured by automod from the firehose and emitted to ozone for historical tracking.
    pub active: bool,
}


/// Logs a scheduled takedown action for an account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleTakedownEvent {
    /// Logs a scheduled takedown action for an account.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "executeAt")]
    pub execute_at: Option<String>,
    /// Logs a scheduled takedown action for an account.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Logs a scheduled takedown action for an account.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "executeUntil")]
    pub execute_until: Option<String>,
    /// Logs a scheduled takedown action for an account.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "executeAfter")]
    pub execute_after: Option<String>,
}


/// Logs cancellation of a scheduled takedown action for an account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelScheduledTakedownEvent {
    /// Logs cancellation of a scheduled takedown action for an account.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}


