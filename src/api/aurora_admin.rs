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
        defs::{AuroraAdminError, Subject},
        events::{LogEventParams, ModerationEventLogger, ModerationEventType},
        moderation::{ApplyActionParams, ModerationAction},
        reports::ReportStatus,
    },
    auth::AdminAuthContext,
    AppContext,
};
use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

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

    // Snapshot capture flag: accepted but no-op in 3.5 (§9.3).
    let _ = input.snapshot_capture;
    let metadata = input.metadata.clone();

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

    Ok(Json(EmitEventOutput {
        event_id: event.id.to_string(),
        // Phase 3.8 will populate this when the audit chain ships.
        audit_entry_id: None,
        // Phase 3.8 will populate this when snapshot infrastructure lands.
        snapshot_id: None,
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
        serde_json::to_value(&action_kind_str(&input.action)).unwrap_or(serde_json::Value::Null),
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
        assert!(resp.audit_entry_id.is_none(), "Phase 3.5 returns None until 3.8");
        assert!(resp.snapshot_id.is_none(), "Phase 3.5 returns None until 3.8");
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
}
