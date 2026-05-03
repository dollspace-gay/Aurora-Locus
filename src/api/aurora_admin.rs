//! Admin-tier action surface under `tools.aurora.admin.*`.
//!
//! Implements Phase 3.5 (chainlink #102) per the
//! [design doc](../../docs/AURORA_ADMIN_UI_DESIGN.md) §8.1 and §8.6.
//!
//! - `emitEvent` — unified moderation action surface (§8.1)
//! - `triggerPasswordReset` — admin-initiated user-mediated reset (§8.6)
//! - `batchTakedownAccounts`, `batchSuspendAccounts`, `batchRestoreAccounts`,
//!   `batchTakedownRecords`, `batchApplyLabel`, `batchRemoveLabel` (§8.8–§8.13)
//!
//! `emitEvent` is a discriminated-action procedure that subsumes the
//! per-action endpoints (`takedownAccount`, `suspendAccount`, etc.)
//! while those remain live for protocol compatibility per §9.2's
//! "per-action endpoints stay live for protocol-compatibility but
//! the UI consumes `emitEvent` exclusively post-3.5" note.
//!
//! Snapshot capture (the `snapshot_capture` flag on `EmitEventInput`)
//! is accepted but is a no-op until Phase 3.8 lands the snapshot
//! infrastructure (see §9.3 cross-phase dependencies). The output
//! type carries `snapshot_id: Option<String>` for forward-compat;
//! 3.5 always returns `None` there.
//!
//! Auth: `AdminModeration` scope at the namespace middleware level
//! (per Phase 2.2 substrate). Within-tier role checks happen at the
//! handler — Moderator+ for content actions, Admin+ for account-
//! infrastructure actions (delete, password reset).

use crate::{
    admin::{
        appeals::{AppealManager, AppealStatus},
        audit_chain::{self, AppendEntryParams, AuditEntry},
        defs::{AuroraAdminError, CursorPosition, PaginatedResponse, PaginationParams, Subject},
        events::{LogEventParams, ModerationEventLogger, ModerationEventType},
        moderation::{ApplyActionParams, ModerationAction},
        reports::ReportStatus,
    },
    auth::AdminAuthContext,
    error::PdsError,
    AppContext,
};
use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// ===========================================================================
// Wire-format types
// ===========================================================================

/// Input for `tools.aurora.admin.emitEvent`. Per design doc §8.1.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmitEventInput {
    pub action: ModEventAction,
    pub subject: Subject,
    pub rationale: String,
    /// Whether to capture a snapshot of the subject's pre-action state.
    /// No-op in Phase 3.5; honored once snapshot infrastructure ships
    /// in Phase 3.8 (§9.3 cross-phase dependency).
    #[serde(default = "default_true")]
    pub snapshot_capture: bool,
    /// Action-specific options (e.g. `{"durationDays": 7}` for
    /// SuspendAccount, `{"reason": "csam", "legalReference": "..."}`
    /// for QuarantineBlob). Per-action interpretation documented in
    /// the dispatch matrix below.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

fn default_true() -> bool {
    true
}

/// Output for `tools.aurora.admin.emitEvent`. Per design doc §8.1.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmitEventOutput {
    pub event_id: String,
    /// `None` until Phase 3.8 audit chain ships.
    pub audit_entry_id: Option<String>,
    /// `None` in Phase 3.5; populated when snapshot infrastructure
    /// lands in Phase 3.8.
    pub snapshot_id: Option<String>,
    /// Event ids of actions cascaded server-side. The canonical
    /// example is appeal-approval triggering an automatic reversal
    /// of the original moderation action — see §8.14.
    pub cascading_actions: Vec<String>,
}

/// Discriminated action enum. Per design doc §8.1's `ModEventAction`.
/// Wire format: `{"kind": "TakedownAccount"}` for unit variants;
/// `{"kind": "ApplyLabel", "val": "spam", "neg": false}` for variants
/// with inline data.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind")]
pub enum ModEventAction {
    TakedownAccount,
    SuspendAccount,
    RestoreAccount,
    DeleteAccount,
    ApplyLabel {
        val: String,
        #[serde(default)]
        neg: bool,
    },
    RemoveLabel {
        val: String,
    },
    TakedownRecord,
    QuarantineBlob,
    RestoreBlob,
    DeleteBlob,
    ResolveReport {
        #[serde(rename = "reportId")]
        report_id: i64,
        resolution: ReportResolution,
    },
    DismissReport {
        #[serde(rename = "reportId")]
        report_id: i64,
    },
    ResolveAppeal {
        #[serde(rename = "appealId")]
        appeal_id: i64,
        resolution: AppealResolutionDecision,
    },
    EscalateAppeal {
        #[serde(rename = "appealId")]
        appeal_id: i64,
    },
    SendEmail {
        #[serde(default)]
        template: Option<String>,
        subject: String,
        body: String,
    },
    UpdateSubjectStatus {
        status: SubjectStatusValue,
    },
}

/// Outcome of a report review. Used by `ResolveReport`.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportResolution {
    Resolved,
    Acknowledged,
    Escalated,
}

impl ReportResolution {
    fn as_db_status(self) -> ReportStatus {
        match self {
            Self::Resolved => ReportStatus::Resolved,
            Self::Acknowledged => ReportStatus::Acknowledged,
            Self::Escalated => ReportStatus::Escalated,
        }
    }

    fn as_resolution_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::Acknowledged => "acknowledged",
            Self::Escalated => "escalated",
        }
    }
}

/// Outcome of an appeal review. Used by `ResolveAppeal`.
/// `Approve` triggers cascade: original action reverses atomically
/// (see §8.14 + §9.3).
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppealResolutionDecision {
    Approve,
    Deny,
}

/// Status set via `UpdateSubjectStatus`. Currently mirrors the
/// existing `com.atproto.admin.updateSubjectStatus` shape on the
/// account dimension.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubjectStatusValue {
    Takedown,
    Deactivated,
    Active,
}

// ===========================================================================
// Helpers
// ===========================================================================

fn validation(msg: impl Into<String>) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": "InvalidEvent", "message": msg.into()})),
    )
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": "Internal", "message": e.to_string()})),
    )
}

fn forbidden(reason: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": AuroraAdminError::PermissionDenied.code(),
            "message": reason,
        })),
    )
}

/// Subject → DID extractor for actions that target an account.
/// Returns `None` for non-Repo subjects.
fn require_repo_did(subject: &Subject) -> Result<&str, (StatusCode, Json<serde_json::Value>)> {
    match subject {
        Subject::Repo { did } => Ok(did.as_str()),
        _ => Err(validation(
            "action requires a Repo subject (did:plc:...) but got a Record or Blob subject",
        )),
    }
}

/// Subject → (URI, optional CID) for actions targeting a record or
/// label-able subject. Records use `(uri, Some(cid))`; account labels
/// can target a Repo subject's DID-as-URI form.
fn subject_uri_cid(
    subject: &Subject,
) -> Result<(String, Option<String>), (StatusCode, Json<serde_json::Value>)> {
    match subject {
        Subject::Record { uri, cid } => Ok((uri.clone(), Some(cid.clone()))),
        Subject::Repo { did } => Ok((format!("at://{}", did), None)),
        Subject::Blob { did, cid, .. } => Ok((format!("at://{}", did), Some(cid.clone()))),
    }
}

/// Subject → CID for blob-targeting actions.
fn require_blob_cid(subject: &Subject) -> Result<&str, (StatusCode, Json<serde_json::Value>)> {
    match subject {
        Subject::Blob { cid, .. } => Ok(cid.as_str()),
        _ => Err(validation(
            "action requires a Blob subject ($type=com.atproto.admin.defs#repoBlobRef)",
        )),
    }
}

/// Bridge `Subject` → flat columns for `moderation_event` insertion.
fn subject_columns(subject: &Subject) -> (Option<&str>, Option<&str>, Option<&str>) {
    match subject {
        Subject::Repo { did } => (Some(did.as_str()), None, None),
        Subject::Record { uri, cid } => (None, Some(uri.as_str()), Some(cid.as_str())),
        Subject::Blob { did, cid, .. } => (Some(did.as_str()), None, Some(cid.as_str())),
    }
}

/// Validate operator role against action requirements (§8.1 step 1).
/// Account-infrastructure actions (delete, password reset) require
/// Admin+; content actions accept Moderator+.
fn check_role(
    auth: &AdminAuthContext,
    action: &ModEventAction,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    let admin_required = matches!(action, ModEventAction::DeleteAccount);
    let needed = if admin_required {
        Role::Admin
    } else {
        Role::Moderator
    };
    if auth.role.can_act_as(needed) {
        Ok(())
    } else {
        Err(forbidden(&format!(
            "action requires {:?}+ role; caller has {:?}",
            needed, auth.role
        )))
    }
}

/// Map `Subject` → the moderation_event row's actor-perspective event
/// type for the dispatched action. Used so `emitEvent`'s audit
/// breadcrumb matches the per-action endpoints' `event_type`.
fn event_type_for(action: &ModEventAction) -> ModerationEventType {
    use ModEventAction as A;
    match action {
        A::TakedownAccount => ModerationEventType::AccountTakedown,
        A::SuspendAccount => ModerationEventType::AccountSuspend,
        A::RestoreAccount => ModerationEventType::AccountRestore,
        A::DeleteAccount => ModerationEventType::AccountTakedown,
        A::ApplyLabel { .. } => ModerationEventType::LabelCreate,
        A::RemoveLabel { .. } => ModerationEventType::LabelRemove,
        A::TakedownRecord => ModerationEventType::AccountTakedown,
        A::QuarantineBlob => ModerationEventType::BlobQuarantine,
        A::RestoreBlob => ModerationEventType::BlobRestore,
        A::DeleteBlob => ModerationEventType::BlobQuarantine,
        A::ResolveReport { .. } | A::DismissReport { .. } => ModerationEventType::ReportReview,
        A::ResolveAppeal { .. } | A::EscalateAppeal { .. } => ModerationEventType::AppealReview,
        A::SendEmail { .. } => ModerationEventType::AccountWarn,
        A::UpdateSubjectStatus { .. } => ModerationEventType::AccountTakedown,
    }
}

// ===========================================================================
// emitEvent — §8.1
// ===========================================================================

/// `tools.aurora.admin.emitEvent` — unified action surface.
///
/// Dispatch matrix:
///
/// | Action variant         | Subject required | Manager called                        |
/// |------------------------|------------------|---------------------------------------|
/// | TakedownAccount        | Repo             | moderation_manager.apply_action       |
/// | SuspendAccount         | Repo             | moderation_manager.apply_action       |
/// | RestoreAccount         | Repo             | moderation_manager.apply_action       |
/// | DeleteAccount          | Repo             | account_manager.delete_account_perm.. |
/// | ApplyLabel             | any              | label_manager.apply_label             |
/// | RemoveLabel            | any              | label_manager.remove_label            |
/// | TakedownRecord         | Record           | label_manager + moderation_event log  |
/// | QuarantineBlob         | Blob             | blob_quarantine.quarantine_blob       |
/// | RestoreBlob            | Blob             | blob_quarantine.restore_blob          |
/// | DeleteBlob             | Blob             | blob_store.delete                     |
/// | ResolveReport          | any              | report_manager.update_status          |
/// | DismissReport          | any              | report_manager.update_status (resolved with "dismissed") |
/// | ResolveAppeal (approve)| any              | AppealManager.update_status + cascade |
/// | ResolveAppeal (deny)   | any              | AppealManager.update_status           |
/// | EscalateAppeal         | any              | AppealManager.update_status           |
/// | SendEmail              | Repo             | mailer.send_admin_email (best-effort) |
/// | UpdateSubjectStatus    | Repo             | moderation_manager.apply_action       |
pub async fn emit_event(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<EmitEventInput>,
) -> Result<Json<EmitEventOutput>, (StatusCode, Json<serde_json::Value>)> {
    // Step 1: role check
    check_role(&auth, &input.action)?;

    // Step 2: rationale must be non-empty after trim (§8.1)
    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }

    let metadata = input.metadata.clone();

    // Step 3 (§8.1): capture snapshot before the action lands. Phase
    // 3.8 makes this real; pre-3.8 callers passing snapshot_capture=
    // true got None back, post-3.8 they get a real snapshot id.
    let snapshot_id = if input.snapshot_capture {
        audit_chain::capture_snapshot(&ctx.account_db, &input.subject)
            .await
            .map_err(internal_pds)?
    } else {
        None
    };

    let cascading_actions = dispatch_action(&ctx, &auth, &input).await?;

    // Step 6 (§8.1): emit to moderation_event log so subscribers and
    // queryEvents see this action.
    let event_log = ModerationEventLogger::new(ctx.account_db.clone());
    let event_type = event_type_for(&input.action);
    let (subject_did, subject_uri, subject_cid) = subject_columns(&input.subject);
    let details = build_event_details(&input, &metadata);
    let event = event_log
        .log_event(LogEventParams {
            event_type,
            actor_did: &auth.did,
            subject_did,
            subject_uri,
            subject_cid,
            details: details.clone(),
            meta: metadata,
        })
        .await
        .map_err(internal)?;

    // Step 5 (§8.1): write audit chain entry referencing snapshot +
    // event. Hash linkage gives tamper-evident replay.
    let action_str = action_kind_str(&input.action);
    let audit_entry_id = audit_chain::append_entry(
        &ctx.account_db,
        AppendEntryParams {
            actor_did: &auth.did,
            action: action_str,
            subject: Some(&input.subject),
            rationale: &input.rationale,
            snapshot_id,
            event_id: Some(event.id),
            cascade_subjects: &[],
        },
    )
    .await
    .map_err(internal_pds)?;

    Ok(Json(EmitEventOutput {
        event_id: event.id.to_string(),
        audit_entry_id: Some(audit_entry_id.to_string()),
        snapshot_id: snapshot_id.map(|id| id.to_string()),
        cascading_actions,
    }))
}

/// Build the `details` JSON payload that lands in `moderation_event.details`.
/// Captures the operator's rationale plus any action-specific metadata so
/// downstream consumers (queryEvents, audit chain) can reconstruct intent.
fn build_event_details(input: &EmitEventInput, metadata: &Option<serde_json::Value>) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("rationale".to_string(), serde_json::Value::String(input.rationale.clone()));
    obj.insert(
        "action".to_string(),
        serde_json::to_value(action_kind_str(&input.action)).unwrap_or(serde_json::Value::Null),
    );
    if let Some(m) = metadata {
        obj.insert("metadata".to_string(), m.clone());
    }
    serde_json::Value::Object(obj)
}

fn action_kind_str(action: &ModEventAction) -> &'static str {
    use ModEventAction as A;
    match action {
        A::TakedownAccount => "TakedownAccount",
        A::SuspendAccount => "SuspendAccount",
        A::RestoreAccount => "RestoreAccount",
        A::DeleteAccount => "DeleteAccount",
        A::ApplyLabel { .. } => "ApplyLabel",
        A::RemoveLabel { .. } => "RemoveLabel",
        A::TakedownRecord => "TakedownRecord",
        A::QuarantineBlob => "QuarantineBlob",
        A::RestoreBlob => "RestoreBlob",
        A::DeleteBlob => "DeleteBlob",
        A::ResolveReport { .. } => "ResolveReport",
        A::DismissReport { .. } => "DismissReport",
        A::ResolveAppeal { .. } => "ResolveAppeal",
        A::EscalateAppeal { .. } => "EscalateAppeal",
        A::SendEmail { .. } => "SendEmail",
        A::UpdateSubjectStatus { .. } => "UpdateSubjectStatus",
    }
}

/// Dispatch the action to the appropriate manager. Returns the list
/// of cascading event IDs (empty for non-cascading actions; non-empty
/// for ResolveAppeal[Approve] which triggers an automatic reversal).
async fn dispatch_action(
    ctx: &AppContext,
    auth: &AdminAuthContext,
    input: &EmitEventInput,
) -> Result<Vec<String>, (StatusCode, Json<serde_json::Value>)> {
    use ModEventAction as A;
    match &input.action {
        A::TakedownAccount => {
            let did = require_repo_did(&input.subject)?;
            ctx.moderation_manager
                .apply_action(ApplyActionParams {
                    did,
                    action: ModerationAction::Takedown,
                    reason: &input.rationale,
                    moderated_by: &auth.did,
                    expires_in: None,
                    report_id: None,
                    notes: None,
                })
                .await
                .map_err(internal)?;
            Ok(Vec::new())
        }
        A::SuspendAccount => {
            let did = require_repo_did(&input.subject)?;
            // duration_days from metadata, optional
            let expires_in = input
                .metadata
                .as_ref()
                .and_then(|m| m.get("durationDays"))
                .and_then(|v| v.as_i64())
                .map(chrono::Duration::days);
            ctx.moderation_manager
                .apply_action(ApplyActionParams {
                    did,
                    action: ModerationAction::Suspend,
                    reason: &input.rationale,
                    moderated_by: &auth.did,
                    expires_in,
                    report_id: None,
                    notes: None,
                })
                .await
                .map_err(internal)?;
            Ok(Vec::new())
        }
        A::RestoreAccount => {
            let did = require_repo_did(&input.subject)?;
            ctx.moderation_manager
                .apply_action(ApplyActionParams {
                    did,
                    action: ModerationAction::Restore,
                    reason: &input.rationale,
                    moderated_by: &auth.did,
                    expires_in: None,
                    report_id: None,
                    notes: None,
                })
                .await
                .map_err(internal)?;
            Ok(Vec::new())
        }
        A::DeleteAccount => {
            let did = require_repo_did(&input.subject)?;
            ctx.account_manager
                .delete_account_permanent(did)
                .await
                .map_err(internal)?;
            Ok(Vec::new())
        }
        A::ApplyLabel { val, neg: _neg } => {
            let (uri, cid) = subject_uri_cid(&input.subject)?;
            ctx.label_manager
                .apply_label(&uri, cid.as_deref(), val, &auth.did, None)
                .await
                .map_err(internal)?;
            Ok(Vec::new())
        }
        A::RemoveLabel { val } => {
            let (uri, cid) = subject_uri_cid(&input.subject)?;
            ctx.label_manager
                .remove_label(&uri, cid.as_deref(), val, &auth.did)
                .await
                .map_err(internal)?;
            Ok(Vec::new())
        }
        A::TakedownRecord => {
            // Record takedown is implemented as a `!takedown` self-label
            // applied to the record URI — same approach the existing
            // updateSubjectStatus handler uses for record takedown.
            let (uri, cid) = match &input.subject {
                Subject::Record { uri, cid } => (uri.clone(), Some(cid.clone())),
                _ => {
                    return Err(validation(
                        "TakedownRecord requires a Record subject ($type=com.atproto.repo.strongRef)",
                    ));
                }
            };
            ctx.label_manager
                .apply_label(&uri, cid.as_deref(), "!takedown", &auth.did, None)
                .await
                .map_err(internal)?;
            Ok(Vec::new())
        }
        A::QuarantineBlob => {
            let cid = require_blob_cid(&input.subject)?;
            // Optional metadata: reason ("dmca"|"csam"|"tos"|"legal"|
            // "malware"|"other"), legalReference. Default reason:
            // "other" — operator's rationale carries the actual
            // explanation.
            use crate::blob_store::quarantine::{BlobQuarantine, QuarantineReason};
            use std::str::FromStr;
            let reason = input
                .metadata
                .as_ref()
                .and_then(|m| m.get("reason"))
                .and_then(|v| v.as_str())
                .and_then(|s| QuarantineReason::from_str(s).ok())
                .unwrap_or(QuarantineReason::Other);
            let legal_reference = input
                .metadata
                .as_ref()
                .and_then(|m| m.get("legalReference"))
                .and_then(|v| v.as_str());
            let quarantine = BlobQuarantine::new(ctx.account_db.clone());
            quarantine
                .quarantine_blob(cid, reason, Some(input.rationale.as_str()), &auth.did, legal_reference)
                .await
                .map_err(internal)?;
            Ok(Vec::new())
        }
        A::RestoreBlob => {
            let cid = require_blob_cid(&input.subject)?;
            use crate::blob_store::quarantine::BlobQuarantine;
            let quarantine = BlobQuarantine::new(ctx.account_db.clone());
            quarantine
                .restore_blob(cid, &auth.did)
                .await
                .map_err(internal)?;
            Ok(Vec::new())
        }
        A::DeleteBlob => {
            let cid = require_blob_cid(&input.subject)?;
            ctx.blob_store.delete(cid).await.map_err(internal)?;
            Ok(Vec::new())
        }
        A::ResolveReport { report_id, resolution } => {
            ctx.report_manager
                .update_status(
                    *report_id,
                    resolution.as_db_status(),
                    &auth.did,
                    Some(resolution.as_resolution_str()),
                )
                .await
                .map_err(internal)?;
            Ok(Vec::new())
        }
        A::DismissReport { report_id } => {
            // ReportStatus has no Dismissed variant; map to Resolved with
            // an explicit "dismissed" resolution string per §8.1's enum
            // (DismissReport is semantically a resolved-as-dismissed).
            ctx.report_manager
                .update_status(*report_id, ReportStatus::Resolved, &auth.did, Some("dismissed"))
                .await
                .map_err(internal)?;
            Ok(Vec::new())
        }
        A::ResolveAppeal { appeal_id, resolution } => {
            let mgr = AppealManager::new(ctx.account_db.clone());
            // Look up the appeal so we can cascade if approve.
            let appeal = mgr
                .get_appeal(*appeal_id)
                .await
                .map_err(internal)?
                .ok_or_else(|| -> (StatusCode, Json<serde_json::Value>) {
                    AuroraAdminError::AppealNotFound.into()
                })?;
            let new_status = match resolution {
                AppealResolutionDecision::Approve => AppealStatus::Approved,
                AppealResolutionDecision::Deny => AppealStatus::Denied,
            };
            mgr.update_status(
                *appeal_id,
                new_status,
                &auth.did,
                Some(input.rationale.as_str()),
                None,
            )
            .await
            .map_err(internal)?;

            // Cascade: approve + reversible original action → reverse it.
            let mut cascade = Vec::new();
            if matches!(resolution, AppealResolutionDecision::Approve) {
                if let Some(mod_id) = appeal.moderation_id {
                    // Reverse the original moderation action atomically.
                    if let Err(e) = ctx
                        .moderation_manager
                        .reverse_action(
                            mod_id,
                            &auth.did,
                            &format!("appeal {} approved: {}", appeal_id, input.rationale),
                        )
                        .await
                    {
                        tracing::warn!(
                            "appeal {} approved but reversal of moderation {} failed: {}",
                            appeal_id, mod_id, e
                        );
                    } else {
                        // Log the cascade as a separate moderation event
                        // so audit/queryEvents see both pieces.
                        let cascade_event = ModerationEventLogger::new(ctx.account_db.clone())
                            .log_event(LogEventParams {
                                event_type: ModerationEventType::AccountRestore,
                                actor_did: &auth.did,
                                subject_did: appeal.appellant_did.as_str().into(),
                                subject_uri: None,
                                subject_cid: None,
                                details: serde_json::json!({
                                    "rationale": format!(
                                        "cascade from appeal {} approval", appeal_id
                                    ),
                                    "action": "RestoreAccount",
                                    "cascadeOf": appeal_id,
                                }),
                                meta: None,
                            })
                            .await
                            .map_err(internal)?;
                        cascade.push(cascade_event.id.to_string());
                    }
                }
            }
            Ok(cascade)
        }
        A::EscalateAppeal { appeal_id } => {
            let mgr = AppealManager::new(ctx.account_db.clone());
            mgr.update_status(
                *appeal_id,
                AppealStatus::Escalated,
                &auth.did,
                None,
                Some(input.rationale.as_str()),
            )
            .await
            .map_err(internal)?;
            Ok(Vec::new())
        }
        A::SendEmail { template, subject, body } => {
            let did = require_repo_did(&input.subject)?;
            let account = ctx
                .account_manager
                .get_account(did)
                .await
                .map_err(|_| validation("recipient account not found"))?;
            let email = account.email.as_deref().unwrap_or("");
            if email.is_empty() {
                return Err(validation("recipient account has no email on file"));
            }
            let _ = template; // template selection deferred to mailer enhancement
            if ctx.mailer.is_configured() {
                ctx.mailer
                    .send_admin_email(email, subject, body)
                    .await
                    .map_err(internal)?;
            } else {
                tracing::warn!(
                    "SendEmail: mailer not configured; event logged but no email sent to {}",
                    did
                );
            }
            Ok(Vec::new())
        }
        A::UpdateSubjectStatus { status } => {
            let did = require_repo_did(&input.subject)?;
            let action = match status {
                SubjectStatusValue::Takedown => ModerationAction::Takedown,
                SubjectStatusValue::Active => ModerationAction::Restore,
                SubjectStatusValue::Deactivated => ModerationAction::Suspend,
            };
            ctx.moderation_manager
                .apply_action(ApplyActionParams {
                    did,
                    action,
                    reason: &input.rationale,
                    moderated_by: &auth.did,
                    expires_in: None,
                    report_id: None,
                    notes: None,
                })
                .await
                .map_err(internal)?;
            Ok(Vec::new())
        }
    }
}

// ===========================================================================
// Batch endpoints — §8.8–§8.13
// ===========================================================================
//
// All six batch endpoints follow the same atomic pattern:
//   1. Validate batch size (1..=50) per design doc hard cap
//   2. Validate role
//   3. Begin transaction on account_db
//   4. INSERT one moderation_event row per batch (single audit semantic;
//      subject columns NULL since the batch references many subjects;
//      full subject list lives in details JSON)
//   5. INSERT per-subject rows (account_moderation or label) within tx
//   6. Commit transaction (atomicity boundary)
//   7. Best-effort side-effect updates outside tx (takedown_ref,
//      session purge) — failures logged, do not roll back the audit
//      record. This matches existing single-subject takedown_account's
//      "Don't fail the whole operation" pattern in admin.rs.
//
// `batchRemoveLabel` is the only endpoint with non-atomic-failure
// semantics: subjects without the label get reported in `skipped`
// rather than failing the whole batch (per design doc §8.13).

const MAX_BATCH_SIZE: usize = 50;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotRef {
    /// Subject the snapshot was captured for. Always present so the
    /// UI can map snapshots back to their subjects in the response.
    pub subject: Subject,
    /// Snapshot id; `None` until Phase 3.8 snapshot infrastructure
    /// ships. The field is in the wire shape now so consumers don't
    /// need a wire-format break when 3.8 lands.
    pub snapshot_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchAccountsInput {
    pub dids: Vec<String>,
    pub rationale: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchAccountsOutput {
    pub event_id: String,
    pub audit_entry_id: Option<String>,
    pub affected_count: u32,
    pub snapshots: Vec<SnapshotRef>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchRecordsInput {
    pub uris: Vec<String>,
    pub rationale: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchLabelInput {
    pub subjects: Vec<Subject>,
    pub label_val: String,
    #[serde(default)]
    pub label_neg: bool,
    pub rationale: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchLabelOutput {
    pub event_id: String,
    pub audit_entry_id: Option<String>,
    pub affected_count: u32,
    pub snapshots: Vec<SnapshotRef>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchRemoveLabelOutput {
    pub event_id: String,
    pub audit_entry_id: Option<String>,
    pub affected_count: u32,
    /// Subjects that didn't have the label — reported transparently
    /// rather than failing the batch (§8.13 non-atomic-failure rule).
    pub skipped: Vec<Subject>,
    pub snapshots: Vec<SnapshotRef>,
}

fn validate_batch_size<T>(items: &[T]) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if items.is_empty() {
        return Err(validation("batch must contain at least one subject"));
    }
    if items.len() > MAX_BATCH_SIZE {
        return Err(validation(format!(
            "batch size {} exceeds limit of {}",
            items.len(),
            MAX_BATCH_SIZE
        )));
    }
    Ok(())
}

fn check_moderator_role(
    auth: &AdminAuthContext,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if auth.role.can_act_as(Role::Moderator) {
        Ok(())
    } else {
        Err(forbidden(&format!(
            "batch action requires Moderator+ role; caller has {:?}",
            auth.role
        )))
    }
}

/// Insert per-subject account_moderation rows + one moderation_event
/// row in a single transaction. Returns the event id.
async fn insert_batch_account_moderations(
    ctx: &AppContext,
    actor_did: &str,
    action_db_str: &str,
    event_type: ModerationEventType,
    rationale: &str,
    dids: &[String],
) -> Result<i64, (StatusCode, Json<serde_json::Value>)> {
    let mut tx = ctx.account_db.begin().await.map_err(internal)?;
    let now = chrono::Utc::now().to_rfc3339();
    for did in dids {
        sqlx::query(
            "INSERT INTO account_moderation \
             (did, action, reason, moderated_by, moderated_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(did)
        .bind(action_db_str)
        .bind(rationale)
        .bind(actor_did)
        .bind(&now)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    }
    let details = serde_json::json!({
        "rationale": rationale,
        "action": event_type.as_str(),
        "batch": true,
        "subjects": dids,
    });
    let event_id: i64 = sqlx::query_scalar(
        "INSERT INTO moderation_event \
         (event_type, actor_did, subject_did, subject_uri, subject_cid, details, created_at, meta) \
         VALUES ($1, $2, NULL, NULL, NULL, $3, $4, NULL) \
         RETURNING id",
    )
    .bind(event_type.as_str())
    .bind(actor_did)
    .bind(details.to_string())
    .bind(&now)
    .fetch_one(&mut *tx)
    .await
    .map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    Ok(event_id)
}

fn snapshots_for_dids(dids: &[String]) -> Vec<SnapshotRef> {
    dids.iter()
        .map(|d| SnapshotRef {
            subject: Subject::Repo { did: d.clone() },
            snapshot_id: None,
        })
        .collect()
}

/// `tools.aurora.admin.batchTakedownAccounts` (§8.8).
pub async fn batch_takedown_accounts(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<BatchAccountsInput>,
) -> Result<Json<BatchAccountsOutput>, (StatusCode, Json<serde_json::Value>)> {
    check_moderator_role(&auth)?;
    validate_batch_size(&input.dids)?;
    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }
    let event_id = insert_batch_account_moderations(
        &ctx,
        &auth.did,
        "takedown",
        ModerationEventType::AccountTakedown,
        &input.rationale,
        &input.dids,
    )
    .await?;
    // Best-effort actor-table update + session purge per DID.
    for did in &input.dids {
        let takedown_ref = format!("batch_event_{}", event_id);
        if let Err(e) = ctx.account_manager.takedown_account(did, &takedown_ref).await {
            tracing::warn!("batch takedown side-effect failed for {}: {}", did, e);
        }
    }
    Ok(Json(BatchAccountsOutput {
        event_id: event_id.to_string(),
        audit_entry_id: None,
        affected_count: input.dids.len() as u32,
        snapshots: snapshots_for_dids(&input.dids),
    }))
}

/// `tools.aurora.admin.batchSuspendAccounts` (§8.9).
pub async fn batch_suspend_accounts(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<BatchAccountsInput>,
) -> Result<Json<BatchAccountsOutput>, (StatusCode, Json<serde_json::Value>)> {
    check_moderator_role(&auth)?;
    validate_batch_size(&input.dids)?;
    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }
    let event_id = insert_batch_account_moderations(
        &ctx,
        &auth.did,
        "suspend",
        ModerationEventType::AccountSuspend,
        &input.rationale,
        &input.dids,
    )
    .await?;
    Ok(Json(BatchAccountsOutput {
        event_id: event_id.to_string(),
        audit_entry_id: None,
        affected_count: input.dids.len() as u32,
        snapshots: snapshots_for_dids(&input.dids),
    }))
}

/// `tools.aurora.admin.batchRestoreAccounts` (§8.10).
pub async fn batch_restore_accounts(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<BatchAccountsInput>,
) -> Result<Json<BatchAccountsOutput>, (StatusCode, Json<serde_json::Value>)> {
    check_moderator_role(&auth)?;
    validate_batch_size(&input.dids)?;
    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }
    let event_id = insert_batch_account_moderations(
        &ctx,
        &auth.did,
        "restore",
        ModerationEventType::AccountRestore,
        &input.rationale,
        &input.dids,
    )
    .await?;
    // Restore side-effect: clear takedown_ref.
    let mut tx = ctx.account_db.begin().await.map_err(internal)?;
    for did in &input.dids {
        if let Err(e) = sqlx::query("UPDATE actor SET takedown_ref = NULL WHERE did = $1")
            .bind(did)
            .execute(&mut *tx)
            .await
        {
            tracing::warn!("batch restore side-effect failed for {}: {}", did, e);
        }
    }
    tx.commit().await.map_err(internal)?;
    Ok(Json(BatchAccountsOutput {
        event_id: event_id.to_string(),
        audit_entry_id: None,
        affected_count: input.dids.len() as u32,
        snapshots: snapshots_for_dids(&input.dids),
    }))
}

/// `tools.aurora.admin.batchTakedownRecords` (§8.11).
pub async fn batch_takedown_records(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<BatchRecordsInput>,
) -> Result<Json<BatchAccountsOutput>, (StatusCode, Json<serde_json::Value>)> {
    check_moderator_role(&auth)?;
    validate_batch_size(&input.uris)?;
    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }
    // Records are taken down via !takedown self-label. Single tx
    // covers all label inserts + the moderation_event row.
    let mut tx = ctx.account_db.begin().await.map_err(internal)?;
    let now = chrono::Utc::now().to_rfc3339();
    let server_did = format!("did:web:{}", ctx.config.service.hostname);
    for uri in &input.uris {
        sqlx::query(
            "INSERT INTO label (uri, cid, val, neg, src, created_at, created_by) \
             VALUES ($1, NULL, $2, FALSE, $3, $4, $5)",
        )
        .bind(uri)
        .bind("!takedown")
        .bind(&server_did)
        .bind(&now)
        .bind(&auth.did)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    }
    let details = serde_json::json!({
        "rationale": input.rationale,
        "action": "TakedownRecord",
        "batch": true,
        "subjects": input.uris,
    });
    let event_id: i64 = sqlx::query_scalar(
        "INSERT INTO moderation_event \
         (event_type, actor_did, subject_did, subject_uri, subject_cid, details, created_at, meta) \
         VALUES ($1, $2, NULL, NULL, NULL, $3, $4, NULL) \
         RETURNING id",
    )
    .bind(ModerationEventType::AccountTakedown.as_str())
    .bind(&auth.did)
    .bind(details.to_string())
    .bind(&now)
    .fetch_one(&mut *tx)
    .await
    .map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    let snapshots = input
        .uris
        .iter()
        .map(|uri| SnapshotRef {
            subject: Subject::Record {
                uri: uri.clone(),
                cid: String::new(),
            },
            snapshot_id: None,
        })
        .collect();
    Ok(Json(BatchAccountsOutput {
        event_id: event_id.to_string(),
        audit_entry_id: None,
        affected_count: input.uris.len() as u32,
        snapshots,
    }))
}

/// `tools.aurora.admin.batchApplyLabel` (§8.12).
pub async fn batch_apply_label(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<BatchLabelInput>,
) -> Result<Json<BatchLabelOutput>, (StatusCode, Json<serde_json::Value>)> {
    check_moderator_role(&auth)?;
    validate_batch_size(&input.subjects)?;
    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }
    if input.label_val.trim().is_empty() {
        return Err(validation("label_val is required and must be non-empty"));
    }
    let mut tx = ctx.account_db.begin().await.map_err(internal)?;
    let now = chrono::Utc::now().to_rfc3339();
    let server_did = format!("did:web:{}", ctx.config.service.hostname);
    for subject in &input.subjects {
        let (uri, cid) = match subject {
            Subject::Record { uri, cid } => (uri.clone(), Some(cid.clone())),
            Subject::Repo { did } => (format!("at://{}", did), None),
            Subject::Blob { did, cid, .. } => (format!("at://{}", did), Some(cid.clone())),
        };
        sqlx::query(
            "INSERT INTO label (uri, cid, val, neg, src, created_at, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&uri)
        .bind(&cid)
        .bind(&input.label_val)
        .bind(input.label_neg)
        .bind(&server_did)
        .bind(&now)
        .bind(&auth.did)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    }
    let subject_jsons: Vec<serde_json::Value> = input
        .subjects
        .iter()
        .map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null))
        .collect();
    let details = serde_json::json!({
        "rationale": input.rationale,
        "action": "ApplyLabel",
        "batch": true,
        "labelVal": input.label_val,
        "labelNeg": input.label_neg,
        "subjects": subject_jsons,
    });
    let event_id: i64 = sqlx::query_scalar(
        "INSERT INTO moderation_event \
         (event_type, actor_did, subject_did, subject_uri, subject_cid, details, created_at, meta) \
         VALUES ($1, $2, NULL, NULL, NULL, $3, $4, NULL) \
         RETURNING id",
    )
    .bind(ModerationEventType::LabelCreate.as_str())
    .bind(&auth.did)
    .bind(details.to_string())
    .bind(&now)
    .fetch_one(&mut *tx)
    .await
    .map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    let snapshots = input
        .subjects
        .iter()
        .map(|s| SnapshotRef {
            subject: s.clone(),
            snapshot_id: None,
        })
        .collect();
    Ok(Json(BatchLabelOutput {
        event_id: event_id.to_string(),
        audit_entry_id: None,
        affected_count: input.subjects.len() as u32,
        snapshots,
    }))
}

/// `tools.aurora.admin.batchRemoveLabel` (§8.13).
///
/// Differs from the other batch endpoints: subjects without the
/// label go into `skipped` rather than failing the batch.
pub async fn batch_remove_label(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<BatchLabelInput>,
) -> Result<Json<BatchRemoveLabelOutput>, (StatusCode, Json<serde_json::Value>)> {
    check_moderator_role(&auth)?;
    validate_batch_size(&input.subjects)?;
    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }
    if input.label_val.trim().is_empty() {
        return Err(validation("label_val is required and must be non-empty"));
    }
    // First-pass: detect which subjects currently have the label.
    let server_did = format!("did:web:{}", ctx.config.service.hostname);
    let mut applied_subjects = Vec::new();
    let mut skipped = Vec::new();
    for subject in &input.subjects {
        let (uri, cid) = match subject {
            Subject::Record { uri, cid } => (uri.clone(), Some(cid.clone())),
            Subject::Repo { did } => (format!("at://{}", did), None),
            Subject::Blob { did, cid, .. } => (format!("at://{}", did), Some(cid.clone())),
        };
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM label \
             WHERE uri = $1 AND val = $2 AND neg = FALSE \
               AND ($3::text IS NULL OR cid = $3)",
        )
        .bind(&uri)
        .bind(&input.label_val)
        .bind(&cid)
        .fetch_one(&ctx.account_db)
        .await
        .map_err(internal)?;
        if count > 0 {
            applied_subjects.push((subject.clone(), uri, cid));
        } else {
            skipped.push(subject.clone());
        }
    }
    // Atomic write of negative labels + event for the applied subset.
    let mut tx = ctx.account_db.begin().await.map_err(internal)?;
    let now = chrono::Utc::now().to_rfc3339();
    for (_subject, uri, cid) in &applied_subjects {
        sqlx::query(
            "INSERT INTO label (uri, cid, val, neg, src, created_at, created_by) \
             VALUES ($1, $2, $3, TRUE, $4, $5, $6)",
        )
        .bind(uri)
        .bind(cid)
        .bind(&input.label_val)
        .bind(&server_did)
        .bind(&now)
        .bind(&auth.did)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    }
    let subject_jsons: Vec<serde_json::Value> = applied_subjects
        .iter()
        .map(|(s, _, _)| serde_json::to_value(s).unwrap_or(serde_json::Value::Null))
        .collect();
    let skipped_jsons: Vec<serde_json::Value> = skipped
        .iter()
        .map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null))
        .collect();
    let details = serde_json::json!({
        "rationale": input.rationale,
        "action": "RemoveLabel",
        "batch": true,
        "labelVal": input.label_val,
        "subjects": subject_jsons,
        "skipped": skipped_jsons,
    });
    let event_id: i64 = sqlx::query_scalar(
        "INSERT INTO moderation_event \
         (event_type, actor_did, subject_did, subject_uri, subject_cid, details, created_at, meta) \
         VALUES ($1, $2, NULL, NULL, NULL, $3, $4, NULL) \
         RETURNING id",
    )
    .bind(ModerationEventType::LabelRemove.as_str())
    .bind(&auth.did)
    .bind(details.to_string())
    .bind(&now)
    .fetch_one(&mut *tx)
    .await
    .map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    let snapshots = applied_subjects
        .iter()
        .map(|(s, _, _)| SnapshotRef {
            subject: s.clone(),
            snapshot_id: None,
        })
        .collect();
    Ok(Json(BatchRemoveLabelOutput {
        event_id: event_id.to_string(),
        audit_entry_id: None,
        affected_count: applied_subjects.len() as u32,
        skipped,
        snapshots,
    }))
}

// ===========================================================================
// triggerPasswordReset — §8.6
// ===========================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerPasswordResetInput {
    pub did: String,
    pub rationale: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerPasswordResetOutput {
    pub reset_email_sent: bool,
    /// Format: "e****@example.com" — first character + asterisks + @
    /// + domain. Confirms the right email was used without exposing
    /// full PII to the operator session.
    pub masked_email: String,
    pub audit_entry_id: String,
}

/// Mask an email: first character + asterisks + "@domain".
/// `evan@example.com` → `e****@example.com`.
fn mask_email(email: &str) -> String {
    if let Some(at_idx) = email.find('@') {
        let (local, domain) = email.split_at(at_idx);
        let first = local.chars().next().unwrap_or('e');
        format!("{}****{}", first, domain)
    } else {
        // No @ — mask conservatively.
        "****".to_string()
    }
}

/// `tools.aurora.admin.triggerPasswordReset` (§8.6).
pub async fn trigger_password_reset(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<TriggerPasswordResetInput>,
) -> Result<Json<TriggerPasswordResetOutput>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::Admin) {
        return Err(forbidden(&format!(
            "triggerPasswordReset requires Admin+ role; caller has {:?}",
            auth.role
        )));
    }
    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }
    // Look up account by DID to get the email + handle.
    let account = ctx
        .account_manager
        .get_account(&input.did)
        .await
        .map_err(|_| validation("account not found"))?;
    let email = account
        .email
        .clone()
        .ok_or_else(|| validation("account has no email on file"))?;
    let handle = account.handle.clone().unwrap_or_else(|| input.did.clone());
    // Generate token via existing infrastructure (uses handle-or-email
    // identifier; we pass handle).
    let (token, _email_returned) = ctx
        .account_manager
        .generate_password_reset_token(&handle)
        .await
        .map_err(internal)?;
    // Send the email if mailer is configured (best-effort per existing
    // request_password_reset pattern in src/api/server.rs).
    let mut email_sent = false;
    if ctx.mailer.is_configured() {
        let base_url = ctx.service_url();
        match ctx
            .mailer
            .send_password_reset_email(&email, &handle, &token, &base_url)
            .await
        {
            Ok(()) => {
                email_sent = true;
            }
            Err(e) => {
                tracing::warn!(
                    "triggerPasswordReset: token generated but email failed for {}: {}",
                    input.did,
                    e
                );
            }
        }
    } else {
        tracing::warn!(
            "triggerPasswordReset: mailer not configured; token generated but no email sent for {}",
            input.did
        );
    }
    // Audit log entry per §8.6 step 5.
    let _ = ctx
        .admin_role_manager
        .log_action(
            &auth.did,
            "account.trigger_password_reset",
            Some(&input.did),
            Some(&input.rationale),
            None,
        )
        .await;
    // Look up the audit entry id we just wrote so we can return it.
    let audit_entry_id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM admin_audit_log \
         WHERE admin_did = $1 AND action = $2 AND subject_did = $3 \
         ORDER BY id DESC LIMIT 1",
    )
    .bind(&auth.did)
    .bind("account.trigger_password_reset")
    .bind(&input.did)
    .fetch_optional(&ctx.account_db)
    .await
    .map_err(internal)?;
    Ok(Json(TriggerPasswordResetOutput {
        reset_email_sent: email_sent,
        masked_email: mask_email(&email),
        audit_entry_id: audit_entry_id.map(|i| i.to_string()).unwrap_or_default(),
    }))
}

// ===========================================================================
// getQueueStats — §8.3 (Phase 3.7)
// ===========================================================================
//
// Counts of items in moderation queue states. Powers the bell badge
// and Dashboard moderation stat cards. Per §8.3 the design doc allows
// ~30s server-side caching; v0.2 ships fresh-per-request and revisits
// caching when load justifies it.

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetQueueStatsOutput {
    pub open_reports: i64,
    pub pending_appeals: i64,
    pub under_review_reports: i64,
    pub under_review_appeals: i64,
    /// Sum of items needing operator decision. Canonical value the
    /// sidebar bell badge displays.
    pub queue_attention_total: i64,
    pub average_age_open_reports_seconds: i64,
    pub oldest_open_report_age_seconds: i64,
}

pub async fn get_queue_stats(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<GetQueueStatsOutput>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::Moderator) {
        return Err(forbidden(&format!(
            "getQueueStats requires Moderator+ role; caller has {:?}",
            auth.role
        )));
    }

    let open_reports: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM report WHERE status = 'open'")
            .fetch_one(&ctx.account_db)
            .await
            .map_err(internal)?;
    let under_review_reports: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM report WHERE status = 'acknowledged'")
            .fetch_one(&ctx.account_db)
            .await
            .map_err(internal)?;
    let pending_appeals: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM appeal WHERE status = 'pending'")
            .fetch_one(&ctx.account_db)
            .await
            .map_err(internal)?;
    let under_review_appeals: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM appeal WHERE status = 'under_review'")
            .fetch_one(&ctx.account_db)
            .await
            .map_err(internal)?;

    // Age stats — compute in Rust from RFC3339 timestamps to stay
    // cross-backend-portable (SQLite + Postgres date arithmetic
    // diverge enough to make a portable SQL aggregate awkward).
    let open_report_times: Vec<String> = sqlx::query_scalar(
        "SELECT reported_at FROM report WHERE status = 'open' ORDER BY reported_at ASC",
    )
    .fetch_all(&ctx.account_db)
    .await
    .map_err(internal)?;
    let now = chrono::Utc::now();
    let mut total_age_secs: i64 = 0;
    let mut oldest_age_secs: i64 = 0;
    let mut count: i64 = 0;
    for ts_str in &open_report_times {
        if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(ts_str) {
            let age = (now - ts.with_timezone(&chrono::Utc)).num_seconds().max(0);
            total_age_secs += age;
            if age > oldest_age_secs {
                oldest_age_secs = age;
            }
            count += 1;
        }
    }
    let avg_age = if count > 0 { total_age_secs / count } else { 0 };

    let queue_attention_total =
        open_reports + pending_appeals + under_review_reports + under_review_appeals;

    Ok(Json(GetQueueStatsOutput {
        open_reports,
        pending_appeals,
        under_review_reports,
        under_review_appeals,
        queue_attention_total,
        average_age_open_reports_seconds: avg_age,
        oldest_open_report_age_seconds: oldest_age_secs,
    }))
}

// ===========================================================================
// getModerationMetrics — §8.2 (Phase 3.7)
// ===========================================================================
//
// Aggregate moderation metrics for dashboard widgets and time-series
// charts. Per §8.2: time-series + aggregate + delta vs previous-range-
// of-same-length. v0.2 computes fresh per request; the §8.2 5-min
// cache is left to Phase 3.7+ optimization work if profiling justifies.

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    Hour,
    Day,
    Week,
    Month,
}

impl Granularity {
    fn bucket_secs(self) -> i64 {
        match self {
            Granularity::Hour => 3600,
            Granularity::Day => 86_400,
            Granularity::Week => 7 * 86_400,
            Granularity::Month => 30 * 86_400,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MetricType {
    ReportsFiled,
    ReportsResolved,
    AppealsFiled,
    AppealsResolved,
    ActionsTaken,
    ActiveModerators,
    AverageTimeToResolution,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetModerationMetricsInput {
    /// ISO8601 RFC3339 timestamp lower bound.
    pub start: String,
    /// ISO8601 RFC3339 timestamp upper bound.
    pub end: String,
    pub granularity: Granularity,
    /// Subset of metrics to return. Empty list returns all metrics.
    #[serde(default)]
    pub metrics: Vec<MetricType>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataPoint {
    /// Bucket start, RFC3339.
    pub t: String,
    pub v: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeltaInfo {
    pub previous_aggregate: f64,
    pub change_absolute: f64,
    pub change_percent: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricSeries {
    pub metric: MetricType,
    pub points: Vec<DataPoint>,
    pub aggregate: f64,
    pub delta: Option<DeltaInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetModerationMetricsOutput {
    pub start: String,
    pub end: String,
    pub granularity: Granularity,
    pub series: Vec<MetricSeries>,
}

/// Compute one metric over a closed range. Buckets are aligned to
/// `start + n * bucket_secs`. Returns (points, aggregate).
async fn compute_metric(
    ctx: &AppContext,
    metric: MetricType,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
    granularity: Granularity,
) -> Result<(Vec<DataPoint>, f64), PdsError> {
    let (table, time_col, where_extra): (&str, &str, &str) = match metric {
        MetricType::ReportsFiled => ("report", "reported_at", ""),
        MetricType::ReportsResolved => ("report", "reviewed_at", "AND status = 'resolved'"),
        MetricType::AppealsFiled => ("appeal", "submitted_at", ""),
        MetricType::AppealsResolved => (
            "appeal",
            "reviewed_at",
            "AND status IN ('approved', 'denied')",
        ),
        MetricType::ActionsTaken => ("moderation_event", "created_at", ""),
        MetricType::ActiveModerators => ("moderation_event", "created_at", ""),
        MetricType::AverageTimeToResolution => ("report", "reviewed_at", "AND status = 'resolved'"),
    };
    // Special-cased active-moderators metric: count distinct actor_did
    // per bucket rather than rows.
    let select_clause = if metric == MetricType::ActiveModerators {
        "actor_did".to_string()
    } else if metric == MetricType::AverageTimeToResolution {
        "reported_at, reviewed_at".to_string()
    } else {
        format!("{}", time_col)
    };
    let sql = format!(
        "SELECT {} FROM {} WHERE {} >= $1 AND {} < $2 {}",
        select_clause, table, time_col, time_col, where_extra
    );
    let bucket_secs = granularity.bucket_secs();
    let bucket_count = ((end - start).num_seconds().max(0) / bucket_secs).max(1) as usize;
    let mut bucket_values: Vec<f64> = vec![0.0; bucket_count];
    let mut bucket_distinct: Vec<HashSet<String>> = vec![HashSet::new(); bucket_count];
    let mut ttr_total: f64 = 0.0;
    let mut ttr_count: f64 = 0.0;

    let rows = sqlx::query(&sql)
        .bind(start.to_rfc3339())
        .bind(end.to_rfc3339())
        .fetch_all(&ctx.account_db)
        .await?;
    use sqlx::Row as _;
    for row in &rows {
        match metric {
            MetricType::ActiveModerators => {
                if let Ok(actor) = row.try_get::<String, _>("actor_did") {
                    // Bucket by current time approximation — for this
                    // metric we count distinct actors over the range
                    // and treat the entire range as one bucket. Per-
                    // bucket distinct-actor counts would need created_at
                    // in the SELECT; keep the implementation simple and
                    // ship the whole-range distinct-count for v0.2.
                    bucket_distinct[0].insert(actor);
                }
            }
            MetricType::AverageTimeToResolution => {
                let reported: Option<String> = row.try_get("reported_at").ok();
                let reviewed: Option<String> = row.try_get("reviewed_at").ok();
                if let (Some(rs), Some(vs)) = (reported, reviewed) {
                    if let (Ok(r), Ok(v)) = (
                        chrono::DateTime::parse_from_rfc3339(&rs),
                        chrono::DateTime::parse_from_rfc3339(&vs),
                    ) {
                        let secs = (v.with_timezone(&chrono::Utc)
                            - r.with_timezone(&chrono::Utc))
                        .num_seconds() as f64;
                        if secs >= 0.0 {
                            ttr_total += secs;
                            ttr_count += 1.0;
                        }
                    }
                }
            }
            _ => {
                let ts_str: String = row.try_get(time_col).map_err(PdsError::Database)?;
                if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&ts_str) {
                    let secs_since = (ts.with_timezone(&chrono::Utc) - start).num_seconds();
                    let idx = (secs_since / bucket_secs) as usize;
                    if idx < bucket_count {
                        bucket_values[idx] += 1.0;
                    }
                }
            }
        }
    }

    let aggregate: f64 = match metric {
        MetricType::ActiveModerators => {
            // Aggregate = total distinct actors over the whole range.
            let mut all = HashSet::new();
            for s in &bucket_distinct {
                all.extend(s.iter().cloned());
            }
            all.len() as f64
        }
        MetricType::AverageTimeToResolution => {
            if ttr_count > 0.0 {
                ttr_total / ttr_count
            } else {
                0.0
            }
        }
        _ => bucket_values.iter().sum(),
    };

    let points: Vec<DataPoint> = (0..bucket_count)
        .map(|i| {
            let t = start + chrono::Duration::seconds((i as i64) * bucket_secs);
            let v = match metric {
                MetricType::ActiveModerators => bucket_distinct[i].len() as f64,
                MetricType::AverageTimeToResolution => {
                    // Time-to-resolution doesn't bucket meaningfully;
                    // emit aggregate at start bucket.
                    if i == 0 {
                        aggregate
                    } else {
                        0.0
                    }
                }
                _ => bucket_values[i],
            };
            DataPoint {
                t: t.to_rfc3339(),
                v,
            }
        })
        .collect();
    Ok((points, aggregate))
}

pub async fn get_moderation_metrics(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<GetModerationMetricsInput>,
) -> Result<Json<GetModerationMetricsOutput>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::Moderator) {
        return Err(forbidden(&format!(
            "getModerationMetrics requires Moderator+ role; caller has {:?}",
            auth.role
        )));
    }
    let start = chrono::DateTime::parse_from_rfc3339(&input.start)
        .map_err(|e| validation(format!("invalid start timestamp: {}", e)))?
        .with_timezone(&chrono::Utc);
    let end = chrono::DateTime::parse_from_rfc3339(&input.end)
        .map_err(|e| validation(format!("invalid end timestamp: {}", e)))?
        .with_timezone(&chrono::Utc);
    if end <= start {
        return Err(validation("end must be after start"));
    }
    let range_len = end - start;
    let prev_start = start - range_len;
    let prev_end = start;

    let metrics_to_run: Vec<MetricType> = if input.metrics.is_empty() {
        vec![
            MetricType::ReportsFiled,
            MetricType::ReportsResolved,
            MetricType::AppealsFiled,
            MetricType::AppealsResolved,
            MetricType::ActionsTaken,
            MetricType::ActiveModerators,
            MetricType::AverageTimeToResolution,
        ]
    } else {
        input.metrics.clone()
    };

    let mut series = Vec::with_capacity(metrics_to_run.len());
    for metric in metrics_to_run {
        let (points, aggregate) = compute_metric(&ctx, metric, start, end, input.granularity)
            .await
            .map_err(internal_pds)?;
        let (_, prev_aggregate) =
            compute_metric(&ctx, metric, prev_start, prev_end, input.granularity)
                .await
                .map_err(internal_pds)?;
        let delta = if prev_aggregate == 0.0 && aggregate == 0.0 {
            None
        } else {
            let change_absolute = aggregate - prev_aggregate;
            let change_percent = if prev_aggregate.abs() > f64::EPSILON {
                (change_absolute / prev_aggregate) * 100.0
            } else {
                100.0
            };
            Some(DeltaInfo {
                previous_aggregate: prev_aggregate,
                change_absolute,
                change_percent,
            })
        };
        series.push(MetricSeries {
            metric,
            points,
            aggregate,
            delta,
        });
    }

    Ok(Json(GetModerationMetricsOutput {
        start: start.to_rfc3339(),
        end: end.to_rfc3339(),
        granularity: input.granularity,
        series,
    }))
}

fn internal_pds(e: PdsError) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": "Internal", "message": e.to_string()})),
    )
}

// ===========================================================================
// exportAccountForensic — §8.7 (Phase 3.8)
// ===========================================================================
//
// Streamed TAR bundle with chain-of-custody headers. v0.2 ships the
// metadata-bearing pieces (account state, moderation history, audit
// entries, manifest) inside the TAR. Repository CAR + raw blob bytes
// are noted in the manifest as "deferred" and shipped in v0.3 — the
// streaming-CAR + bounded-blob-stream story is non-trivial under
// AnyPool's transactional constraints and the milestone "Forensic
// export modal works end-to-end including chain integration" speaks
// to the chain integration which is fully wired here.

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportAccountForensicInput {
    pub did: String,
    pub rationale: String,
    #[serde(default = "default_true")]
    pub include_repo: bool,
    #[serde(default = "default_true")]
    pub include_blobs: bool,
    #[serde(default = "default_true")]
    pub include_moderation_history: bool,
    #[serde(default)]
    pub include_account_metadata: bool,
    #[serde(default)]
    pub include_audit_chain: bool,
}

pub async fn export_account_forensic(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<ExportAccountForensicInput>,
) -> Result<axum::response::Response, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    use axum::body::Body;
    use axum::http::header;
    use axum::response::IntoResponse;

    if !auth.role.can_act_as(Role::Admin) {
        return Err(forbidden(&format!(
            "exportAccountForensic requires Admin+ role; caller has {:?}",
            auth.role
        )));
    }
    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }
    // §8.7: SuperAdmin-only parameter gate
    if (input.include_account_metadata || input.include_audit_chain)
        && !auth.role.can_act_as(Role::SuperAdmin)
    {
        return Err(forbidden(
            "include_account_metadata and include_audit_chain require SuperAdmin role",
        ));
    }

    let started_at = chrono::Utc::now();

    // Account state (gated by include_account_metadata for sensitive fields)
    let account = ctx
        .account_manager
        .get_account(&input.did)
        .await
        .map_err(|_| validation("account not found"))?;
    let mut account_state = serde_json::json!({
        "did": account.did,
        "handle": account.handle,
        "createdAt": account.created_at.to_rfc3339(),
        "takedownRef": account.takedown_ref,
        "deactivatedAt": account.deactivated_at.map(|dt| dt.to_rfc3339()),
    });
    if input.include_account_metadata {
        account_state["email"] = serde_json::Value::String(
            account.email.clone().unwrap_or_default(),
        );
        account_state["emailConfirmedAt"] = serde_json::Value::String(
            account
                .email_confirmed_at
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
        );
        account_state["invitesDisabled"] = serde_json::Value::Bool(
            account.invites_disabled.unwrap_or(false),
        );
    }

    // Moderation history
    let mod_history: serde_json::Value = if input.include_moderation_history {
        let rows = sqlx::query(
            "SELECT id, action, reason, moderated_by, moderated_at, expires_at, \
                    reversed, reversed_at \
             FROM account_moderation WHERE did = $1 ORDER BY moderated_at DESC",
        )
        .bind(&input.did)
        .fetch_all(&ctx.account_db)
        .await
        .map_err(internal)?;
        use sqlx::Row as _;
        let entries: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.try_get::<i64, _>("id").unwrap_or(0),
                    "action": r.try_get::<String, _>("action").unwrap_or_default(),
                    "reason": r.try_get::<String, _>("reason").unwrap_or_default(),
                    "moderatedBy": r.try_get::<String, _>("moderated_by").unwrap_or_default(),
                    "moderatedAt": r.try_get::<String, _>("moderated_at").unwrap_or_default(),
                    "expiresAt": r.try_get::<Option<String>, _>("expires_at").ok().flatten(),
                    "reversed": crate::db::read_bool(r, "reversed").unwrap_or(false),
                    "reversedAt": r.try_get::<Option<String>, _>("reversed_at").ok().flatten(),
                })
            })
            .collect();
        serde_json::Value::Array(entries)
    } else {
        serde_json::Value::Null
    };

    // Audit chain entries (SuperAdmin-gated)
    let audit_entries: serde_json::Value = if input.include_audit_chain {
        let rows = sqlx::query(
            "SELECT id, sequence, created_at, actor_did, action, subject_did, \
                    rationale, snapshot_id, event_id, current_hash, previous_hash \
             FROM audit_chain_entry WHERE subject_did = $1 ORDER BY sequence ASC",
        )
        .bind(&input.did)
        .fetch_all(&ctx.account_db)
        .await
        .map_err(internal)?;
        use sqlx::Row as _;
        let entries: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.try_get::<i64, _>("id").unwrap_or(0),
                    "sequence": r.try_get::<i64, _>("sequence").unwrap_or(0),
                    "createdAt": r.try_get::<String, _>("created_at").unwrap_or_default(),
                    "actorDid": r.try_get::<String, _>("actor_did").unwrap_or_default(),
                    "action": r.try_get::<String, _>("action").unwrap_or_default(),
                    "rationale": r.try_get::<String, _>("rationale").unwrap_or_default(),
                    "snapshotId": r.try_get::<Option<i64>, _>("snapshot_id").ok().flatten(),
                    "eventId": r.try_get::<Option<i64>, _>("event_id").ok().flatten(),
                    "currentHash": r.try_get::<String, _>("current_hash").unwrap_or_default(),
                    "previousHash": r.try_get::<Option<String>, _>("previous_hash").ok().flatten(),
                })
            })
            .collect();
        serde_json::Value::Array(entries)
    } else {
        serde_json::Value::Null
    };

    // Bundle pieces serialized
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    files.push((
        "account-state.json".to_string(),
        serde_json::to_vec_pretty(&account_state).map_err(|e| internal(e))?,
    ));
    if input.include_moderation_history {
        files.push((
            "moderation-history.json".to_string(),
            serde_json::to_vec_pretty(&mod_history).map_err(|e| internal(e))?,
        ));
    }
    if input.include_audit_chain {
        files.push((
            "audit-entries.json".to_string(),
            serde_json::to_vec_pretty(&audit_entries).map_err(|e| internal(e))?,
        ));
    }

    // Manifest with per-file hashes. include_repo / include_blobs
    // accepted but their contents land in v0.3 (streaming CAR +
    // bounded-blob serialization is non-trivial under AnyPool); the
    // manifest records the deferral so consumers know the bundle is
    // metadata-only for v0.2.
    use sha2::{Digest, Sha256};
    let mut file_hashes: serde_json::Map<String, serde_json::Value> = Default::default();
    for (name, bytes) in &files {
        let mut h = Sha256::new();
        h.update(bytes);
        file_hashes.insert(name.clone(), serde_json::Value::String(hex::encode(h.finalize())));
    }
    let manifest = serde_json::json!({
        "did": input.did,
        "exportedAt": started_at.to_rfc3339(),
        "exportedBy": auth.did,
        "rationale": input.rationale,
        "parameters": {
            "includeRepo": input.include_repo,
            "includeBlobs": input.include_blobs,
            "includeModerationHistory": input.include_moderation_history,
            "includeAccountMetadata": input.include_account_metadata,
            "includeAuditChain": input.include_audit_chain,
        },
        "fileHashes": file_hashes,
        "deferredContents": {
            "repoCar": input.include_repo,
            "blobs": input.include_blobs,
            "note": "v0.2 forensic bundles ship metadata only; CAR + blob streaming lands in v0.3"
        },
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| internal(e))?;
    let mut manifest_hasher = Sha256::new();
    manifest_hasher.update(&manifest_bytes);
    let bundle_hash = hex::encode(manifest_hasher.finalize());

    // Audit chain entry for the export itself per §8.7 step 6.
    let subject = Subject::Repo {
        did: input.did.clone(),
    };
    let snapshot_id =
        audit_chain::capture_snapshot(&ctx.account_db, &subject)
            .await
            .map_err(internal_pds)?;
    let audit_entry_id = audit_chain::append_entry(
        &ctx.account_db,
        AppendEntryParams {
            actor_did: &auth.did,
            action: "ForensicExport",
            subject: Some(&subject),
            rationale: &format!("{} (bundle hash: {})", input.rationale, bundle_hash),
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
        },
    )
    .await
    .map_err(internal_pds)?;

    // audit-trail.json — this export's own chain entry, always included.
    let trail = serde_json::json!({
        "auditEntryId": audit_entry_id,
        "bundleHash": bundle_hash,
        "exportedAt": started_at.to_rfc3339(),
    });
    files.push((
        "audit-trail.json".to_string(),
        serde_json::to_vec_pretty(&trail).map_err(|e| internal(e))?,
    ));

    // TAR assembly
    let mut tar_buf: Vec<u8> = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_buf);
        // Manifest first
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(started_at.timestamp() as u64);
        header.set_cksum();
        builder
            .append_data(&mut header, "manifest.json", &manifest_bytes[..])
            .map_err(|e| internal(e))?;
        for (name, bytes) in &files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(started_at.timestamp() as u64);
            header.set_cksum();
            builder
                .append_data(&mut header, name.as_str(), &bytes[..])
                .map_err(|e| internal(e))?;
        }
        builder.finish().map_err(|e| internal(e))?;
    }

    let filename = format!(
        "forensic-export-{}-{}.tar",
        input.did.replace(':', "_"),
        started_at.format("%Y%m%dT%H%M%SZ")
    );
    let mut response = (StatusCode::OK, tar_buf).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        "application/x-tar".parse().expect("static header value"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}\"", filename)
            .parse()
            .expect("ASCII filename"),
    );
    headers.insert(
        "X-Aurora-Audit-Entry-Id",
        audit_entry_id.to_string().parse().expect("numeric id"),
    );
    headers.insert(
        "X-Aurora-Bundle-Hash",
        bundle_hash.parse().expect("hex string"),
    );
    let _ = Body::empty(); // import-side-effect placeholder to avoid unused
    Ok(response)
}

// ===========================================================================
// getAuditTrail — §8.4 (Phase 3.8)
// ===========================================================================
//
// Hash-chained audit log query. Cursor-paginated newest-first. Each
// entry carries a `verified` flag computed by re-hashing the entry's
// stored fields and comparing to current_hash; a divergent recompute
// surfaces tampering at query time.
//
// Pre-Phase-3.8 events have no chain entry; per §8.4 they show up
// with current_hash="pre-chain" sentinel and verified=false. v0.2's
// Audit page displays both surfaces in a unified feed (per §3.4
// "the audit page is a 'unified' surface"), with the
// `getAuditLog`-derived rows clearly marked unverified.

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetAuditTrailParams {
    #[serde(default)]
    pub actor_did: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub subject_did: Option<String>,
    #[serde(default)]
    pub subject_uri: Option<String>,
    #[serde(default)]
    pub after_created: Option<String>,
    #[serde(default)]
    pub before_created: Option<String>,
    #[serde(flatten)]
    pub pagination: PaginationParams,
}

pub async fn get_audit_trail(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    axum::extract::Query(params): axum::extract::Query<GetAuditTrailParams>,
) -> Result<Json<PaginatedResponse<AuditEntry>>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::Moderator) {
        return Err(forbidden(&format!(
            "getAuditTrail requires Moderator+ role; caller has {:?}",
            auth.role
        )));
    }
    let limit = params.pagination.effective_limit() as i64;
    let cursor = params.pagination.decode_cursor().map_err(|_| {
        let e = AuroraAdminError::OutdatedCursor;
        (e.http_status(), Json(serde_json::json!({"error": e.code()})))
    })?;

    let mut clauses: Vec<&'static str> = Vec::new();
    let mut binds: Vec<String> = Vec::new();
    if let Some(a) = &params.actor_did {
        clauses.push("actor_did = ?");
        binds.push(a.clone());
    }
    if let Some(a) = &params.action {
        clauses.push("action = ?");
        binds.push(a.clone());
    }
    if let Some(s) = &params.subject_did {
        clauses.push("subject_did = ?");
        binds.push(s.clone());
    }
    if let Some(s) = &params.subject_uri {
        clauses.push("subject_uri = ?");
        binds.push(s.clone());
    }
    if let Some(a) = &params.after_created {
        clauses.push("created_at >= ?");
        binds.push(a.clone());
    }
    if let Some(b) = &params.before_created {
        clauses.push("created_at <= ?");
        binds.push(b.clone());
    }
    if let Some(c) = &cursor {
        clauses.push("(created_at < ? OR (created_at = ? AND id < ?))");
        binds.push(c.after_created.to_rfc3339());
        binds.push(c.after_created.to_rfc3339());
    }

    // Renumber `?` → `$N` for Postgres compatibility (mirrors
    // aurora_moderator's renumber_placeholders pattern).
    let mut idx = 1usize;
    let clauses_pg: Vec<String> = clauses
        .iter()
        .map(|clause| {
            let mut out = String::with_capacity(clause.len() + 8);
            for c in clause.chars() {
                if c == '?' {
                    out.push_str(&format!("${}", idx));
                    idx += 1;
                } else {
                    out.push(c);
                }
            }
            out
        })
        .collect();

    let where_sql = if clauses_pg.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses_pg.join(" AND "))
    };
    let limit_idx = binds.len() + if cursor.is_some() { 2 } else { 1 };
    let sql = format!(
        "SELECT id, sequence, created_at, actor_did, action, subject_did, subject_uri, \
                subject_cid, rationale, snapshot_id, event_id, current_hash, previous_hash, \
                cascade_subjects \
         FROM audit_chain_entry{} \
         ORDER BY created_at DESC, id DESC \
         LIMIT ${}",
        where_sql, limit_idx
    );

    let mut q = sqlx::query(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    if let Some(c) = &cursor {
        q = q.bind(c.after_id);
    }
    q = q.bind(limit + 1);

    let rows = q.fetch_all(&ctx.account_db).await.map_err(internal)?;
    let has_more = rows.len() as i64 > limit;
    let page_rows: Vec<_> = rows.into_iter().take(limit as usize).collect();

    use sqlx::Row as _;
    let mut items = Vec::with_capacity(page_rows.len());
    let mut last_at = None;
    let mut last_id = None;
    for row in page_rows {
        let id: i64 = row.try_get("id").map_err(internal)?;
        let sequence: i64 = row.try_get("sequence").map_err(internal)?;
        let created_at_str: String = row.try_get("created_at").map_err(internal)?;
        let timestamp = chrono::DateTime::parse_from_rfc3339(&created_at_str)
            .map_err(|e| internal(e))?
            .with_timezone(&chrono::Utc);
        let actor_did: String = row.try_get("actor_did").map_err(internal)?;
        let action: String = row.try_get("action").map_err(internal)?;
        let subject_did: Option<String> = row.try_get("subject_did").ok().flatten();
        let subject_uri: Option<String> = row.try_get("subject_uri").ok().flatten();
        let subject_cid: Option<String> = row.try_get("subject_cid").ok().flatten();
        let rationale: String = row.try_get("rationale").map_err(internal)?;
        let snapshot_id: Option<i64> = row.try_get("snapshot_id").ok().flatten();
        let event_id: Option<i64> = row.try_get("event_id").ok().flatten();
        let current_hash: String = row.try_get("current_hash").map_err(internal)?;
        let previous_hash: Option<String> = row.try_get("previous_hash").ok().flatten();
        let cascade_str: Option<String> = row.try_get("cascade_subjects").ok().flatten();
        let cascade_subjects: Vec<Subject> = cascade_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        let verified = audit_chain::verify_entry(
            sequence,
            &created_at_str,
            &actor_did,
            &action,
            subject_did.as_deref(),
            subject_uri.as_deref(),
            subject_cid.as_deref(),
            &rationale,
            snapshot_id,
            event_id,
            previous_hash.as_deref(),
            cascade_str.as_deref(),
            &current_hash,
        );
        let subject_ref = Subject::from_columns(
            subject_did.as_deref(),
            subject_uri.as_deref(),
            subject_cid.as_deref(),
        );
        last_at = Some(timestamp);
        last_id = Some(id);
        items.push(AuditEntry {
            id: id.to_string(),
            sequence,
            timestamp,
            actor_did,
            action,
            subject_ref,
            rationale,
            snapshot_id: snapshot_id.map(|i| i.to_string()),
            event_id: event_id.map(|i| i.to_string()),
            current_hash,
            previous_hash,
            verified,
            cascade_subjects,
        });
    }

    let next_cursor = if has_more {
        match (last_at, last_id) {
            (Some(t), Some(i)) => Some(
                CursorPosition {
                    after_created: t,
                    after_id: i,
                }
                .encode(),
            ),
            _ => None,
        }
    } else {
        None
    };
    Ok(Json(PaginatedResponse {
        items,
        cursor: next_cursor,
    }))
}

// ===========================================================================
// getRuntimeSetting / setRuntimeSetting — §8.16 (Phase 3.10)
// ===========================================================================
//
// Two endpoints for the runtime settings infrastructure. Read is
// public-at-any-role for the moderation-mode key (other operators
// need to know what mode they're operating in); write is SuperAdmin
// only. Writes are audit-chained per §8.16.
//
// Recovery path: AURORA_RECOVERY_MODE=true env var bypasses runtime
// settings on startup. The check is at AppContext construction —
// when recovery is set, the live moderation-mode read returns
// "full" regardless of the runtime_settings row, so an operator
// who deployed into a misconfigured "disabled" state can boot with
// the env var set, fix the runtime row, and unset the env var.

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetRuntimeSettingParams {
    pub key: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetRuntimeSettingOutput {
    pub key: String,
    pub value: serde_json::Value,
    pub source: &'static str,
    pub last_modified: Option<String>,
    pub last_modified_by: Option<String>,
}

const MODERATION_MODE_KEY: &str = "moderation-mode";
const MODERATION_MODE_REDIRECT_KEY: &str = "moderation-mode-redirect-url";
const RECOVERY_MODE_ENV: &str = "AURORA_RECOVERY_MODE";

fn default_for_key(key: &str) -> serde_json::Value {
    match key {
        MODERATION_MODE_KEY => serde_json::Value::String("full".to_string()),
        MODERATION_MODE_REDIRECT_KEY => serde_json::Value::String(String::new()),
        _ => serde_json::Value::Null,
    }
}

pub async fn get_runtime_setting(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    axum::extract::Query(params): axum::extract::Query<GetRuntimeSettingParams>,
) -> Result<Json<GetRuntimeSettingOutput>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    // Per §8.16: most settings require Admin+, but moderation-mode
    // is readable at any role since every operator needs to know
    // what mode they're in.
    if params.key != MODERATION_MODE_KEY && !auth.role.can_act_as(Role::Admin) {
        return Err(forbidden(&format!(
            "key '{}' requires Admin+ role; caller has {:?}",
            params.key, auth.role
        )));
    }
    // Recovery-mode override for moderation-mode reads.
    let recovery_active = std::env::var(RECOVERY_MODE_ENV)
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    if recovery_active && params.key == MODERATION_MODE_KEY {
        return Ok(Json(GetRuntimeSettingOutput {
            key: params.key,
            value: serde_json::Value::String("full".to_string()),
            source: "RecoveryMode",
            last_modified: None,
            last_modified_by: None,
        }));
    }
    let row = sqlx::query(
        "SELECT value, last_modified, last_modified_by FROM runtime_settings WHERE key = $1",
    )
    .bind(&params.key)
    .fetch_optional(&ctx.account_db)
    .await
    .map_err(internal)?;
    use sqlx::Row as _;
    if let Some(r) = row {
        let value_str: String = r.try_get("value").map_err(internal)?;
        let value = serde_json::from_str(&value_str)
            .unwrap_or(serde_json::Value::String(value_str));
        let last_modified: String = r.try_get("last_modified").map_err(internal)?;
        let last_modified_by: String = r.try_get("last_modified_by").map_err(internal)?;
        Ok(Json(GetRuntimeSettingOutput {
            key: params.key,
            value,
            source: "Runtime",
            last_modified: Some(last_modified),
            last_modified_by: Some(last_modified_by),
        }))
    } else {
        Ok(Json(GetRuntimeSettingOutput {
            key: params.key.clone(),
            value: default_for_key(&params.key),
            source: "Default",
            last_modified: None,
            last_modified_by: None,
        }))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetRuntimeSettingInput {
    pub key: String,
    pub value: serde_json::Value,
    pub rationale: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetRuntimeSettingOutput {
    pub key: String,
    pub previous_value: serde_json::Value,
    pub new_value: serde_json::Value,
    pub audit_entry_id: String,
}

pub async fn set_runtime_setting(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<SetRuntimeSettingInput>,
) -> Result<Json<SetRuntimeSettingOutput>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(forbidden(&format!(
            "setRuntimeSetting requires SuperAdmin role; caller has {:?}",
            auth.role
        )));
    }
    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }
    // Validate moderation-mode value if that's the key being set.
    if input.key == MODERATION_MODE_KEY {
        let s = input.value.as_str().unwrap_or("");
        if !["full", "reduced", "disabled"].contains(&s) {
            return Err(validation("moderation-mode must be one of: full, reduced, disabled"));
        }
    }
    // Read previous value for the diff returned in output.
    let prev_row = sqlx::query("SELECT value FROM runtime_settings WHERE key = $1")
        .bind(&input.key)
        .fetch_optional(&ctx.account_db)
        .await
        .map_err(internal)?;
    use sqlx::Row as _;
    let previous_value = if let Some(r) = prev_row {
        let s: String = r.try_get("value").map_err(internal)?;
        serde_json::from_str(&s).unwrap_or(serde_json::Value::String(s))
    } else {
        default_for_key(&input.key)
    };
    let now = chrono::Utc::now().to_rfc3339();
    let value_json =
        serde_json::to_string(&input.value).map_err(|e| internal(e))?;
    // Upsert (DELETE then INSERT for cross-backend portability — sqlx
    // ON CONFLICT syntax differs between SQLite and Postgres).
    let mut tx = ctx.account_db.begin().await.map_err(internal)?;
    sqlx::query("DELETE FROM runtime_settings WHERE key = $1")
        .bind(&input.key)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    sqlx::query(
        "INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(&input.key)
    .bind(&value_json)
    .bind(&now)
    .bind(&auth.did)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    // Audit chain entry per §8.16 ("All writes audit-chained").
    let audit_entry_id = audit_chain::append_entry(
        &ctx.account_db,
        AppendEntryParams {
            actor_did: &auth.did,
            action: "SetRuntimeSetting",
            subject: None,
            rationale: &format!("{} → {}: {}", input.key, value_json, input.rationale),
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
        },
    )
    .await
    .map_err(internal_pds)?;
    Ok(Json(SetRuntimeSettingOutput {
        key: input.key,
        previous_value,
        new_value: input.value,
        audit_entry_id: audit_entry_id.to_string(),
    }))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::roles::Role;
    use crate::account::ValidatedSession;

    fn moderator_auth() -> AdminAuthContext {
        AdminAuthContext {
            did: "did:plc:moderator".to_string(),
            session: ValidatedSession {
                did: "did:plc:moderator".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Moderator,
        }
    }

    fn admin_auth() -> AdminAuthContext {
        AdminAuthContext {
            did: "did:plc:admin".to_string(),
            session: ValidatedSession {
                did: "did:plc:admin".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        }
    }

    /// Reuse the test-context construction from aurora_moderator's tests.
    /// Mirrors the exact shape so we get a working AppContext with all
    /// managers wired and migrations applied.
    async fn create_test_context() -> AppContext {
        use crate::config::*;
        use std::path::PathBuf;
        use tempfile::tempdir;
        let dir = tempdir().unwrap().keep();
        let db_path = dir.join("test.db");
        let config = ServerConfig {
            service: ServiceConfig {
                hostname: "localhost".to_string(),
                port: 2583,
                service_did: "did:web:localhost".to_string(),
                version: "0.1.0-test".to_string(),
                blob_upload_limit: 5_242_880,
            },
            storage: StorageConfig {
                data_directory: dir.clone(),
                account_db: db_path.clone(),
                sequencer_db: dir.join("sequencer.db"),
                did_cache_db: dir.join("did_cache.db"),
                actor_store_directory: dir.join("actors"),
                blobstore: BlobstoreConfig::Disk {
                    location: dir.join("blobs"),
                    tmp_location: dir.join("temp"),
                },
            },
            database: Default::default(),
            authentication: AuthConfig {
                jwt_secret: "test-secret-key-aurora-admin-test-32xx".to_string(),
                repo_signing_key: "a".repeat(64),
                plc_rotation_key: "b".repeat(64),
                admin_dids: vec![],
                oauth: OAuthConfig {
                    client_id: "http://localhost:3000/client-metadata.json".to_string(),
                    redirect_uri: "http://localhost:3000/oauth/callback".to_string(),
                    pds_url: "https://bsky.social".to_string(),
                },
                jwt_sunset_date: "Sat, 31 Dec 2024 23:59:59 GMT".to_string(),
                oauth_migration_guide_url: "https://docs.atproto.com/guides/oauth-migration"
                    .to_string(),
                oauth_features: Default::default(),
            },
            identity: IdentityConfig {
                did_plc_url: "https://plc.directory".to_string(),
                service_handle_domains: vec![".localhost".to_string()],
                did_cache_stale_ttl: 3600,
                did_cache_max_ttl: 86400,
            },
            email: None,
            invites: InviteConfig {
                required: false,
                interval: 604800,
                epoch: "2024-01-01T00:00:00Z".to_string(),
            },
            rate_limit: RateLimitConfig {
                enabled: false,
                global_requests_per_minute: 3000,
                use_redis: false,
                redis_url: None,
            },
            logging: LoggingConfig {
                level: "info".to_string(),
            },
            federation: FederationConfig {
                enabled: false,
                relay_urls: vec![],
                appview_url: None,
                firehose_enabled: false,
                crawl_enabled: false,
                public_url: Some("http://localhost:2583".to_string()),
                auto_stream_events: false,
            },
            validation_mode: PathBuf::from("required").into_os_string().to_string_lossy().parse().unwrap_or(crate::validation::ValidationMode::Required),
        };
        AppContext::new(config).await.unwrap()
    }

    /// Insert a minimal actor row so account-targeted actions resolve.
    async fn seed_actor(ctx: &AppContext, did: &str, handle: &str) {
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(did)
            .bind(handle)
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&ctx.account_db)
            .await
            .ok();
    }

    fn repo_subject(did: &str) -> Subject {
        Subject::Repo {
            did: did.to_string(),
        }
    }

    #[tokio::test]
    async fn emit_event_takedown_account_writes_event_and_moderation_row() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:victim", "victim.test").await;
        let resp = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            Json(EmitEventInput {
                action: ModEventAction::TakedownAccount,
                subject: repo_subject("did:plc:victim"),
                rationale: "spam".to_string(),
                snapshot_capture: true,
                metadata: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(!resp.event_id.is_empty());
        // Phase 3.8 makes these meaningful — emitEvent now writes a
        // chain entry + snapshot for snapshottable subjects.
        assert!(resp.audit_entry_id.is_some(), "Phase 3.8 populates audit_entry_id");
        assert!(resp.snapshot_id.is_some(), "Phase 3.8 captures snapshot for Repo subjects");
        assert!(resp.cascading_actions.is_empty());
        // Verify the moderation_event row landed.
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM moderation_event WHERE actor_did = $1")
                .bind("did:plc:moderator")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        assert_eq!(count, 1);
        // Verify moderation row landed.
        let mod_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM account_moderation WHERE did = $1 AND action = $2",
        )
        .bind("did:plc:victim")
        .bind("takedown")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(mod_count, 1);
    }

    #[tokio::test]
    async fn emit_event_rejects_empty_rationale() {
        let ctx = create_test_context().await;
        let err = emit_event(
            State(ctx),
            moderator_auth(),
            Json(EmitEventInput {
                action: ModEventAction::TakedownAccount,
                subject: repo_subject("did:plc:victim"),
                rationale: "   ".to_string(),
                snapshot_capture: true,
                metadata: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn emit_event_rejects_record_subject_for_account_action() {
        let ctx = create_test_context().await;
        let err = emit_event(
            State(ctx),
            moderator_auth(),
            Json(EmitEventInput {
                action: ModEventAction::TakedownAccount,
                subject: Subject::Record {
                    uri: "at://did:plc:abc/app.bsky.feed.post/123".to_string(),
                    cid: "bafyrei...".to_string(),
                },
                rationale: "wrong subject type".to_string(),
                snapshot_capture: true,
                metadata: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn emit_event_delete_account_requires_admin_role() {
        let ctx = create_test_context().await;
        let err = emit_event(
            State(ctx),
            moderator_auth(),
            Json(EmitEventInput {
                action: ModEventAction::DeleteAccount,
                subject: repo_subject("did:plc:victim"),
                rationale: "test".to_string(),
                snapshot_capture: true,
                metadata: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn emit_event_apply_label_writes_label_row() {
        let ctx = create_test_context().await;
        let resp = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            Json(EmitEventInput {
                action: ModEventAction::ApplyLabel {
                    val: "spam".to_string(),
                    neg: false,
                },
                subject: Subject::Record {
                    uri: "at://did:plc:abc/app.bsky.feed.post/xyz".to_string(),
                    cid: "bafyreigh".to_string(),
                },
                rationale: "obvious spam".to_string(),
                snapshot_capture: true,
                metadata: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(!resp.event_id.is_empty());
        let label_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM label WHERE val = $1 AND neg = FALSE",
        )
        .bind("spam")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(label_count, 1);
    }

    #[tokio::test]
    async fn emit_event_resolve_appeal_approve_cascades_reversal() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:appellant", "appellant.test").await;
        // Apply a takedown so we have something to reverse.
        ctx.moderation_manager
            .apply_action(ApplyActionParams {
                did: "did:plc:appellant",
                action: ModerationAction::Takedown,
                reason: "initial".to_string().as_str(),
                moderated_by: "did:plc:m1",
                expires_in: None,
                report_id: None,
                notes: None,
            })
            .await
            .unwrap();
        let mod_id: i64 = sqlx::query_scalar(
            "SELECT id FROM account_moderation WHERE did = $1 ORDER BY id DESC LIMIT 1",
        )
        .bind("did:plc:appellant")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        // Submit an appeal against that moderation.
        let mgr = AppealManager::new(ctx.account_db.clone());
        let appeal = mgr
            .submit_appeal(
                Some(mod_id),
                None,
                None,
                "did:plc:appellant",
                "false positive",
                None,
            )
            .await
            .unwrap();
        // Approve the appeal — should reverse the original moderation
        // and surface the cascade in the response.
        let resp = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            Json(EmitEventInput {
                action: ModEventAction::ResolveAppeal {
                    appeal_id: appeal.id,
                    resolution: AppealResolutionDecision::Approve,
                },
                subject: repo_subject("did:plc:appellant"),
                rationale: "appeal valid".to_string(),
                snapshot_capture: true,
                metadata: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.cascading_actions.len(), 1);
        // Verify the moderation row is now reversed.
        let reversed = crate::db::read_bool(
            &sqlx::query("SELECT reversed FROM account_moderation WHERE id = $1")
                .bind(mod_id)
                .fetch_one(&ctx.account_db)
                .await
                .unwrap(),
            "reversed",
        )
        .unwrap();
        assert!(reversed, "appeal approval should reverse the original moderation");
    }

    #[tokio::test]
    async fn emit_event_resolve_appeal_deny_does_not_cascade() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:appellant2", "appellant2.test").await;
        ctx.moderation_manager
            .apply_action(ApplyActionParams {
                did: "did:plc:appellant2",
                action: ModerationAction::Takedown,
                reason: "initial",
                moderated_by: "did:plc:m1",
                expires_in: None,
                report_id: None,
                notes: None,
            })
            .await
            .unwrap();
        let mod_id: i64 = sqlx::query_scalar(
            "SELECT id FROM account_moderation WHERE did = $1 ORDER BY id DESC LIMIT 1",
        )
        .bind("did:plc:appellant2")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        let mgr = AppealManager::new(ctx.account_db.clone());
        let appeal = mgr
            .submit_appeal(Some(mod_id), None, None, "did:plc:appellant2", "frivolous", None)
            .await
            .unwrap();
        let resp = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            Json(EmitEventInput {
                action: ModEventAction::ResolveAppeal {
                    appeal_id: appeal.id,
                    resolution: AppealResolutionDecision::Deny,
                },
                subject: repo_subject("did:plc:appellant2"),
                rationale: "denied".to_string(),
                snapshot_capture: true,
                metadata: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(resp.cascading_actions.is_empty());
        // Reversal must NOT have happened.
        let reversed = crate::db::read_bool(
            &sqlx::query("SELECT reversed FROM account_moderation WHERE id = $1")
                .bind(mod_id)
                .fetch_one(&ctx.account_db)
                .await
                .unwrap(),
            "reversed",
        )
        .unwrap();
        assert!(!reversed, "denied appeals must not reverse the original moderation");
    }

    #[tokio::test]
    async fn emit_event_admin_role_can_delete_account() {
        let ctx = create_test_context().await;
        // Seed an actor row directly so delete has something to operate
        // on without going through the full PLC-registration path.
        seed_actor(&ctx, "did:plc:deleteme", "deleteme.test").await;
        let resp = emit_event(
            State(ctx),
            admin_auth(),
            Json(EmitEventInput {
                action: ModEventAction::DeleteAccount,
                subject: repo_subject("did:plc:deleteme"),
                rationale: "voluntary deletion".to_string(),
                snapshot_capture: true,
                metadata: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(!resp.event_id.is_empty());
    }

    #[test]
    fn emit_event_input_deserializes_unit_action() {
        let raw = serde_json::json!({
            "action": {"kind": "TakedownAccount"},
            "subject": {"$type": "com.atproto.admin.defs#repoRef", "did": "did:plc:abc"},
            "rationale": "spam"
        });
        let input: EmitEventInput = serde_json::from_value(raw).unwrap();
        assert!(matches!(input.action, ModEventAction::TakedownAccount));
        assert!(input.snapshot_capture, "snapshot_capture defaults to true");
    }

    #[test]
    fn emit_event_input_deserializes_action_with_inline_data() {
        let raw = serde_json::json!({
            "action": {"kind": "ApplyLabel", "val": "spam", "neg": false},
            "subject": {"$type": "com.atproto.repo.strongRef", "uri": "at://did:plc:abc/x/y", "cid": "bafy..."},
            "rationale": "obvious"
        });
        let input: EmitEventInput = serde_json::from_value(raw).unwrap();
        match input.action {
            ModEventAction::ApplyLabel { val, neg } => {
                assert_eq!(val, "spam");
                assert!(!neg);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn report_resolution_round_trip() {
        let r: ReportResolution =
            serde_json::from_str("\"resolved\"").unwrap();
        assert_eq!(r, ReportResolution::Resolved);
        assert_eq!(r.as_resolution_str(), "resolved");
        assert_eq!(r.as_db_status().as_str(), "resolved");
    }

    #[test]
    fn appeal_resolution_decision_round_trip() {
        let approve: AppealResolutionDecision =
            serde_json::from_str("\"approve\"").unwrap();
        assert_eq!(approve, AppealResolutionDecision::Approve);
        let deny: AppealResolutionDecision =
            serde_json::from_str("\"deny\"").unwrap();
        assert_eq!(deny, AppealResolutionDecision::Deny);
    }

    // ---------- Batch endpoints (§8.8–§8.13) ----------

    #[tokio::test]
    async fn batch_takedown_accepts_valid_batch_and_writes_one_event() {
        let ctx = create_test_context().await;
        for i in 0..3 {
            seed_actor(&ctx, &format!("did:plc:b{}", i), &format!("b{}.test", i)).await;
        }
        let resp = batch_takedown_accounts(
            State(ctx.clone()),
            moderator_auth(),
            Json(BatchAccountsInput {
                dids: vec![
                    "did:plc:b0".to_string(),
                    "did:plc:b1".to_string(),
                    "did:plc:b2".to_string(),
                ],
                rationale: "spam ring".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.affected_count, 3);
        assert_eq!(resp.snapshots.len(), 3);
        // ONE moderation_event row for the batch (per design doc).
        let event_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM moderation_event WHERE actor_did = $1")
                .bind("did:plc:moderator")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        assert_eq!(event_count, 1);
        // THREE account_moderation rows (one per subject).
        let mod_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM account_moderation WHERE moderated_by = $1 AND action = $2",
        )
        .bind("did:plc:moderator")
        .bind("takedown")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(mod_count, 3);
    }

    #[tokio::test]
    async fn batch_takedown_rejects_empty_batch() {
        let ctx = create_test_context().await;
        let err = batch_takedown_accounts(
            State(ctx),
            moderator_auth(),
            Json(BatchAccountsInput {
                dids: vec![],
                rationale: "test".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn batch_takedown_rejects_oversized_batch() {
        let ctx = create_test_context().await;
        let dids: Vec<String> = (0..51).map(|i| format!("did:plc:b{}", i)).collect();
        let err = batch_takedown_accounts(
            State(ctx),
            moderator_auth(),
            Json(BatchAccountsInput {
                dids,
                rationale: "too big".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn batch_apply_label_writes_per_subject_label_rows() {
        let ctx = create_test_context().await;
        let resp = batch_apply_label(
            State(ctx.clone()),
            moderator_auth(),
            Json(BatchLabelInput {
                subjects: vec![
                    Subject::Record {
                        uri: "at://did:plc:r1/x/y".to_string(),
                        cid: "bafy1".to_string(),
                    },
                    Subject::Record {
                        uri: "at://did:plc:r2/x/y".to_string(),
                        cid: "bafy2".to_string(),
                    },
                ],
                label_val: "spam".to_string(),
                label_neg: false,
                rationale: "obvious".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.affected_count, 2);
        let label_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM label WHERE val = $1 AND neg = FALSE")
                .bind("spam")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        assert_eq!(label_count, 2);
    }

    #[tokio::test]
    async fn batch_remove_label_skips_subjects_without_label() {
        let ctx = create_test_context().await;
        // Apply label to one of two subjects upfront.
        batch_apply_label(
            State(ctx.clone()),
            moderator_auth(),
            Json(BatchLabelInput {
                subjects: vec![Subject::Record {
                    uri: "at://did:plc:has/x/y".to_string(),
                    cid: "bafy".to_string(),
                }],
                label_val: "spam".to_string(),
                label_neg: false,
                rationale: "preseed".to_string(),
            }),
        )
        .await
        .unwrap();
        let resp = batch_remove_label(
            State(ctx),
            moderator_auth(),
            Json(BatchLabelInput {
                subjects: vec![
                    Subject::Record {
                        uri: "at://did:plc:has/x/y".to_string(),
                        cid: "bafy".to_string(),
                    },
                    Subject::Record {
                        uri: "at://did:plc:nope/x/y".to_string(),
                        cid: "bafy2".to_string(),
                    },
                ],
                label_val: "spam".to_string(),
                label_neg: false,
                rationale: "remove valid".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.affected_count, 1);
        assert_eq!(resp.skipped.len(), 1);
        // The skipped subject is the one that didn't have the label.
        match &resp.skipped[0] {
            Subject::Record { uri, .. } => assert_eq!(uri, "at://did:plc:nope/x/y"),
            _ => panic!("wrong skipped subject shape"),
        }
    }

    #[tokio::test]
    async fn batch_endpoints_accept_admin_role() {
        // Moderator is the floor role in the current Role enum
        // (Moderator < Admin < SuperAdmin), so a "below moderator"
        // negative test isn't expressible until a lower role lands.
        // This positive test verifies Admin (above Moderator) passes
        // the gate for the moderator+ batch endpoints.
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:admintest", "admintest.test").await;
        let resp = batch_takedown_accounts(
            State(ctx),
            admin_auth(),
            Json(BatchAccountsInput {
                dids: vec!["did:plc:admintest".to_string()],
                rationale: "admin batch".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.affected_count, 1);
    }

    // ---------- triggerPasswordReset (§8.6) ----------

    #[test]
    fn mask_email_formats_correctly() {
        assert_eq!(mask_email("evan@example.com"), "e****@example.com");
        assert_eq!(mask_email("a@b.co"), "a****@b.co");
        assert_eq!(mask_email("nodomain"), "****");
    }

    #[tokio::test]
    async fn trigger_password_reset_requires_admin_role() {
        let ctx = create_test_context().await;
        let err = trigger_password_reset(
            State(ctx),
            moderator_auth(),
            Json(TriggerPasswordResetInput {
                did: "did:plc:user".to_string(),
                rationale: "lost password".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn trigger_password_reset_rejects_account_without_email() {
        let ctx = create_test_context().await;
        // Seed actor + account row with NULL email.
        seed_actor(&ctx, "did:plc:noemail", "noemail.test").await;
        sqlx::query(
            "INSERT INTO account (did, email, password_hash, email_confirmed_at, invites_disabled) \
             VALUES ($1, NULL, $2, NULL, FALSE)",
        )
        .bind("did:plc:noemail")
        .bind("$argon2id$dummy")
        .execute(&ctx.account_db)
        .await
        .unwrap();
        let err = trigger_password_reset(
            State(ctx),
            admin_auth(),
            Json(TriggerPasswordResetInput {
                did: "did:plc:noemail".to_string(),
                rationale: "test".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    // ---------- Phase 3.7 — getQueueStats (§8.3) ----------

    #[tokio::test]
    async fn get_queue_stats_returns_zero_for_empty_db() {
        let ctx = create_test_context().await;
        let resp = get_queue_stats(State(ctx), moderator_auth())
            .await
            .unwrap()
            .0;
        assert_eq!(resp.open_reports, 0);
        assert_eq!(resp.pending_appeals, 0);
        assert_eq!(resp.queue_attention_total, 0);
    }

    #[tokio::test]
    async fn get_queue_stats_aggregates_open_reports() {
        let ctx = create_test_context().await;
        // Seed: one open report, one resolved (excluded from open_reports).
        sqlx::query(
            "INSERT INTO report (subject_did, reason_type, reported_by, reported_at, status) \
             VALUES ('did:plc:s1', 'spam', 'did:plc:r1', $1, 'open')",
        )
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&ctx.account_db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO report (subject_did, reason_type, reported_by, reported_at, status) \
             VALUES ('did:plc:s2', 'spam', 'did:plc:r1', $1, 'resolved')",
        )
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&ctx.account_db)
        .await
        .unwrap();
        let resp = get_queue_stats(State(ctx), moderator_auth())
            .await
            .unwrap()
            .0;
        assert_eq!(resp.open_reports, 1);
        assert_eq!(resp.queue_attention_total, 1);
        assert!(resp.average_age_open_reports_seconds >= 0);
    }

    // ---------- Phase 3.7 — getModerationMetrics (§8.2) ----------

    #[tokio::test]
    async fn get_moderation_metrics_rejects_invalid_range() {
        let ctx = create_test_context().await;
        let now = chrono::Utc::now().to_rfc3339();
        let err = get_moderation_metrics(
            State(ctx),
            moderator_auth(),
            Json(GetModerationMetricsInput {
                start: now.clone(),
                end: now,
                granularity: Granularity::Day,
                metrics: vec![],
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_moderation_metrics_returns_series_with_buckets() {
        let ctx = create_test_context().await;
        // Seed 3 reports today.
        for i in 0..3 {
            let when = (chrono::Utc::now() - chrono::Duration::hours(i)).to_rfc3339();
            sqlx::query(
                "INSERT INTO report (subject_did, reason_type, reported_by, reported_at, status) \
                 VALUES ('did:plc:s', 'spam', 'did:plc:r', $1, 'open')",
            )
            .bind(when)
            .execute(&ctx.account_db)
            .await
            .unwrap();
        }
        let start = (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339();
        let end = (chrono::Utc::now() + chrono::Duration::seconds(60)).to_rfc3339();
        let resp = get_moderation_metrics(
            State(ctx),
            moderator_auth(),
            Json(GetModerationMetricsInput {
                start,
                end,
                granularity: Granularity::Day,
                metrics: vec![MetricType::ReportsFiled],
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.series.len(), 1);
        assert_eq!(resp.series[0].metric, MetricType::ReportsFiled);
        assert_eq!(resp.series[0].aggregate, 3.0);
    }

    #[tokio::test]
    async fn get_moderation_metrics_delta_compares_previous_range() {
        let ctx = create_test_context().await;
        let now = chrono::Utc::now();
        // 2 reports in current 1-day window
        for _ in 0..2 {
            sqlx::query(
                "INSERT INTO report (subject_did, reason_type, reported_by, reported_at, status) \
                 VALUES ('did:plc:s', 'spam', 'did:plc:r', $1, 'open')",
            )
            .bind((now - chrono::Duration::hours(1)).to_rfc3339())
            .execute(&ctx.account_db)
            .await
            .unwrap();
        }
        // 5 reports in previous 1-day window
        for _ in 0..5 {
            sqlx::query(
                "INSERT INTO report (subject_did, reason_type, reported_by, reported_at, status) \
                 VALUES ('did:plc:s', 'spam', 'did:plc:r', $1, 'open')",
            )
            .bind((now - chrono::Duration::hours(30)).to_rfc3339())
            .execute(&ctx.account_db)
            .await
            .unwrap();
        }
        let start = (now - chrono::Duration::days(1)).to_rfc3339();
        let end = (now + chrono::Duration::seconds(60)).to_rfc3339();
        let resp = get_moderation_metrics(
            State(ctx),
            moderator_auth(),
            Json(GetModerationMetricsInput {
                start,
                end,
                granularity: Granularity::Day,
                metrics: vec![MetricType::ReportsFiled],
            }),
        )
        .await
        .unwrap()
        .0;
        let s = &resp.series[0];
        assert_eq!(s.aggregate, 2.0);
        let d = s.delta.as_ref().unwrap();
        assert_eq!(d.previous_aggregate, 5.0);
        assert_eq!(d.change_absolute, -3.0);
        assert!(d.change_percent < 0.0);
    }

    // ---------- Phase 3.8 — getAuditTrail (§8.4) ----------

    #[tokio::test]
    async fn emit_event_writes_chain_entry_and_snapshot() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:victim", "victim.test").await;
        let resp = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            Json(EmitEventInput {
                action: ModEventAction::TakedownAccount,
                subject: repo_subject("did:plc:victim"),
                rationale: "spam".to_string(),
                snapshot_capture: true,
                metadata: None,
            }),
        )
        .await
        .unwrap()
        .0;
        // Phase 3.8 fills these — Phase 3.5 returned None.
        assert!(resp.audit_entry_id.is_some(), "Phase 3.8 should populate audit_entry_id");
        assert!(resp.snapshot_id.is_some(), "Phase 3.8 should populate snapshot_id");
        // Verify the chain row landed.
        let chain_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_chain_entry WHERE actor_did = $1",
        )
        .bind("did:plc:moderator")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(chain_count, 1);
    }

    #[tokio::test]
    async fn get_audit_trail_returns_entries_with_verified_true() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:victim", "victim.test").await;
        emit_event(
            State(ctx.clone()),
            moderator_auth(),
            Json(EmitEventInput {
                action: ModEventAction::TakedownAccount,
                subject: repo_subject("did:plc:victim"),
                rationale: "spam".to_string(),
                snapshot_capture: true,
                metadata: None,
            }),
        )
        .await
        .unwrap();
        let resp = get_audit_trail(
            State(ctx),
            moderator_auth(),
            axum::extract::Query(GetAuditTrailParams {
                actor_did: None,
                action: None,
                subject_did: None,
                subject_uri: None,
                after_created: None,
                before_created: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.items.len(), 1);
        let entry = &resp.items[0];
        assert!(entry.verified, "fresh entry should be verified");
        assert_eq!(entry.actor_did, "did:plc:moderator");
        assert_eq!(entry.action, "TakedownAccount");
        assert!(entry.snapshot_id.is_some());
    }

    #[tokio::test]
    async fn get_audit_trail_filters_by_actor_did() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:s1", "s1.test").await;
        // Two events from different actors via direct chain inserts.
        crate::admin::audit_chain::append_entry(
            &ctx.account_db,
            crate::admin::audit_chain::AppendEntryParams {
                actor_did: "did:plc:m1",
                action: "TakedownAccount",
                subject: Some(&repo_subject("did:plc:s1")),
                rationale: "first",
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
            },
        ).await.unwrap();
        crate::admin::audit_chain::append_entry(
            &ctx.account_db,
            crate::admin::audit_chain::AppendEntryParams {
                actor_did: "did:plc:m2",
                action: "RestoreAccount",
                subject: Some(&repo_subject("did:plc:s1")),
                rationale: "second",
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
            },
        ).await.unwrap();
        let resp = get_audit_trail(
            State(ctx),
            moderator_auth(),
            axum::extract::Query(GetAuditTrailParams {
                actor_did: Some("did:plc:m1".to_string()),
                action: None, subject_did: None, subject_uri: None,
                after_created: None, before_created: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await.unwrap().0;
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].actor_did, "did:plc:m1");
    }

    // ---------- Phase 3.8 — exportAccountForensic (§8.7) ----------

    #[tokio::test]
    async fn export_forensic_requires_admin_role() {
        let ctx = create_test_context().await;
        let err = export_account_forensic(
            State(ctx),
            moderator_auth(),
            Json(ExportAccountForensicInput {
                did: "did:plc:victim".to_string(),
                rationale: "test".to_string(),
                include_repo: false,
                include_blobs: false,
                include_moderation_history: false,
                include_account_metadata: false,
                include_audit_chain: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn export_forensic_super_admin_gates_block_admin() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:victim", "victim.test").await;
        let err = export_account_forensic(
            State(ctx),
            admin_auth(),
            Json(ExportAccountForensicInput {
                did: "did:plc:victim".to_string(),
                rationale: "test".to_string(),
                include_repo: false,
                include_blobs: false,
                include_moderation_history: false,
                include_account_metadata: true, // SuperAdmin-only
                include_audit_chain: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn export_forensic_writes_audit_entry_and_returns_bundle_headers() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:exported", "exported.test").await;
        // get_account() LEFT-JOINs account onto actor; seed the
        // account row so the lookup resolves.
        sqlx::query(
            "INSERT INTO account (did, email, password_hash, email_confirmed_at, invites_disabled) \
             VALUES ($1, $2, $3, NULL, FALSE)",
        )
        .bind("did:plc:exported")
        .bind("exp@example.com")
        .bind("$argon2id$dummy")
        .execute(&ctx.account_db)
        .await
        .unwrap();
        let resp = export_account_forensic(
            State(ctx.clone()),
            admin_auth(),
            Json(ExportAccountForensicInput {
                did: "did:plc:exported".to_string(),
                rationale: "investigation".to_string(),
                include_repo: false,
                include_blobs: false,
                include_moderation_history: true,
                include_account_metadata: false,
                include_audit_chain: false,
            }),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let headers = resp.headers();
        assert_eq!(
            headers.get(axum::http::header::CONTENT_TYPE).unwrap(),
            "application/x-tar"
        );
        let cd = headers.get(axum::http::header::CONTENT_DISPOSITION).unwrap();
        let cd_str = cd.to_str().unwrap();
        assert!(cd_str.contains("forensic-export-did_plc_exported-"));
        assert!(headers.contains_key("X-Aurora-Audit-Entry-Id"));
        assert!(headers.contains_key("X-Aurora-Bundle-Hash"));
        // Verify the chain entry landed for the export action.
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_chain_entry WHERE action = $1 AND subject_did = $2",
        )
        .bind("ForensicExport")
        .bind("did:plc:exported")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    // ---------- Phase 3.10 — runtime settings (§8.16) ----------

    fn super_admin_auth() -> AdminAuthContext {
        use crate::admin::roles::Role;
        AdminAuthContext {
            did: "did:plc:superadmin".to_string(),
            session: ValidatedSession {
                did: "did:plc:superadmin".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::SuperAdmin,
        }
    }

    #[tokio::test]
    async fn get_runtime_setting_returns_default_for_unknown_row() {
        let ctx = create_test_context().await;
        let resp = get_runtime_setting(
            State(ctx),
            moderator_auth(),
            axum::extract::Query(GetRuntimeSettingParams {
                key: "moderation-mode".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.source, "Default");
        assert_eq!(resp.value, serde_json::Value::String("full".to_string()));
    }

    #[tokio::test]
    async fn get_runtime_setting_admin_required_for_non_mode_keys() {
        let ctx = create_test_context().await;
        let err = get_runtime_setting(
            State(ctx),
            moderator_auth(),
            axum::extract::Query(GetRuntimeSettingParams {
                key: "some-other-key".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn set_runtime_setting_requires_super_admin() {
        let ctx = create_test_context().await;
        let err = set_runtime_setting(
            State(ctx),
            admin_auth(),
            Json(SetRuntimeSettingInput {
                key: "moderation-mode".to_string(),
                value: serde_json::Value::String("reduced".to_string()),
                rationale: "test".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn set_runtime_setting_writes_value_and_audit_entry() {
        let ctx = create_test_context().await;
        let resp = set_runtime_setting(
            State(ctx.clone()),
            super_admin_auth(),
            Json(SetRuntimeSettingInput {
                key: "moderation-mode".to_string(),
                value: serde_json::Value::String("reduced".to_string()),
                rationale: "switching to reduced for moderator team change".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.previous_value, serde_json::Value::String("full".to_string()));
        assert_eq!(resp.new_value, serde_json::Value::String("reduced".to_string()));
        assert!(!resp.audit_entry_id.is_empty());
        // Verify the runtime row landed.
        let stored: String =
            sqlx::query_scalar("SELECT value FROM runtime_settings WHERE key = $1")
                .bind("moderation-mode")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        assert_eq!(stored, "\"reduced\"");
        // Verify the audit chain entry landed.
        let audit_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_chain_entry WHERE action = $1",
        )
        .bind("SetRuntimeSetting")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(audit_count, 1);
    }

    #[tokio::test]
    async fn set_runtime_setting_rejects_invalid_mode_value() {
        let ctx = create_test_context().await;
        let err = set_runtime_setting(
            State(ctx),
            super_admin_auth(),
            Json(SetRuntimeSettingInput {
                key: "moderation-mode".to_string(),
                value: serde_json::Value::String("invalid-mode".to_string()),
                rationale: "test".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn trigger_password_reset_returns_masked_email_and_audit_id() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:withemail", "withemail.test").await;
        sqlx::query(
            "INSERT INTO account (did, email, password_hash, email_confirmed_at, invites_disabled) \
             VALUES ($1, $2, $3, NULL, FALSE)",
        )
        .bind("did:plc:withemail")
        .bind("user@example.com")
        .bind("$argon2id$dummy")
        .execute(&ctx.account_db)
        .await
        .unwrap();
        let resp = trigger_password_reset(
            State(ctx.clone()),
            admin_auth(),
            Json(TriggerPasswordResetInput {
                did: "did:plc:withemail".to_string(),
                rationale: "user requested".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        // Mailer not configured in test ctx → reset_email_sent = false,
        // but token still generated and audit logged.
        assert!(!resp.reset_email_sent);
        assert_eq!(resp.masked_email, "u****@example.com");
        assert!(!resp.audit_entry_id.is_empty());
        // Audit entry exists.
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM admin_audit_log WHERE action = $1 AND subject_did = $2",
        )
        .bind("account.trigger_password_reset")
        .bind("did:plc:withemail")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }
}
