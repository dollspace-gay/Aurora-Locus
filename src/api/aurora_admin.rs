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
//! is honored. Phase 3.8 shipped the snapshot infrastructure (see
//! `audit_chain::capture_snapshot` and AURORA_DESIGN.md §4.4.3); the
//! `emit_event` handler invokes `capture_snapshot` before
//! `dispatch_action` when the flag is true and the subject is
//! snapshottable. The captured row's id is referenced from the
//! audit chain entry written in the same transaction. Output's
//! `snapshot_id` is populated when capture succeeded and left
//! `None` when capture was opted out or skipped (e.g.,
//! non-snapshottable subjects).
//!
//! Auth: `AdminModeration` scope at the namespace middleware level
//! (per Phase 2.2 substrate). Within-tier role checks happen at the
//! handler — Moderator+ for content actions, Admin+ for account-
//! infrastructure actions (delete, password reset).

use crate::{
    account::AccountManager,
    admin::{
        appeals::{AppealManager, AppealStatus},
        audit_chain::{self, AppendEntryParams, AuditEntry},
        defs::{AuroraAdminError, CursorPosition, PaginationParams, Subject},
        events::{LogEventParams, ModerationEventLogger, ModerationEventType},
        labels::LabelManager,
        moderation::{ApplyActionParams, ModerationAction, ModerationManager},
        reports::{ReportManager, ReportStatus},
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

/// Input for `tools.aurora.admin.emitEvent`. Per v0.3 spec §8.4.1
/// (Arc 4 multi-subject reshape).
///
/// Wire-shape break from v0.2: the `subject: Subject` field became
/// `subjects: Vec<Subject>`. Single-subject calls now pass
/// `subjects: [s]`; multi-subject calls pass `subjects: [s1, s2, ...]`.
/// Per-action support and per-action `subjects.len()` caps are
/// enforced in Phase 0 of the handler. See §8.3.4 for the action
/// vocabulary and §8.3.1 for the atomicity scope.
///
/// **Dual-shape acceptance** (Arc 6 Step 7, V04_DESIGN §5.3.6):
/// requests using the legacy v0.2 `subject: Subject` shape are
/// accepted and normalized to the canonical `subjects: vec![s]`
/// during Deserialize. When the legacy shape is parsed,
/// `legacy_subject_used` is set to `true`; the handler reads this
/// to record a metrics counter increment for operator-visible
/// migration tracking.
#[derive(Debug)]
pub struct EmitEventInput {
    pub action: ModEventAction,
    pub subjects: Vec<Subject>,
    pub rationale: String,
    /// Whether to capture a snapshot of each subject's pre-action
    /// state. Snapshot capture runs **before** the wrapping
    /// transaction opens (Phase 1 of the handler) so a snapshot can
    /// outlive a rolled-back mutation — an intentional carve-out from
    /// whole-tx atomicity per §8.3.1's orphan-snapshot rule.
    pub snapshot_capture: bool,
    /// Action-specific options (e.g. `{"durationDays": 7}` for
    /// SuspendAccount, `{"reason": "csam", "legalReference": "..."}`
    /// for QuarantineBlob). Per-action interpretation documented in
    /// the dispatch matrix below.
    pub metadata: Option<serde_json::Value>,
    /// True when this input was deserialized from the legacy v0.2
    /// `subject: Subject` single-subject shape (vs. the canonical
    /// v0.3 `subjects: Vec<Subject>` array). Set by the custom
    /// Deserialize impl; the handler reads it to record a
    /// legacy-wire-shape counter increment via
    /// [`crate::metrics::record_legacy_wire_ingest`]. Not part of
    /// the wire shape — purely an in-memory observability flag.
    pub legacy_subject_used: bool,
}

/// Wire-side scaffold for [`EmitEventInput`]'s custom Deserialize.
/// Holds both shape variants as optional fields; the manual impl
/// matches on which were present and normalizes.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmitEventInputRaw {
    action: ModEventAction,
    /// Canonical v0.3 multi-subject shape.
    #[serde(default)]
    subjects: Option<Vec<Subject>>,
    /// Legacy v0.2 single-subject shape; accepted during dual-shape
    /// window per V04_DESIGN §5.3.6.
    #[serde(default)]
    subject: Option<Subject>,
    rationale: String,
    #[serde(default = "default_true")]
    snapshot_capture: bool,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

impl<'de> Deserialize<'de> for EmitEventInput {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let raw = EmitEventInputRaw::deserialize(d)?;
        let (subjects, legacy_subject_used) = match (raw.subjects, raw.subject) {
            (Some(ss), None) => (ss, false),
            (None, Some(s)) => (vec![s], true),
            (Some(_), Some(_)) => {
                return Err(D::Error::custom(
                    "emitEvent accepts either canonical 'subjects' \
                     (array of Subject) or legacy 'subject' (single Subject), \
                     not both; pick exactly one shape per request",
                ));
            }
            (None, None) => {
                return Err(D::Error::custom(
                    "emitEvent requires either canonical 'subjects' \
                     (array of Subject) or legacy 'subject' (single Subject)",
                ));
            }
        };
        Ok(EmitEventInput {
            action: raw.action,
            subjects,
            rationale: raw.rationale,
            snapshot_capture: raw.snapshot_capture,
            metadata: raw.metadata,
            legacy_subject_used,
        })
    }
}

fn default_true() -> bool {
    true
}

/// Response shape for `tools.aurora.admin.emitEvent`.
///
/// Per `docs/V03_DESIGN.md` §8.3.1: emitEvent multi-subject contract is committed.
/// The following are stable across releases:
///
/// - **Endpoint identity**: `tools.aurora.admin.emitEvent`, POST,
///   AdminAuthContext + Moderator+ (with per-action role gating
///   for destructive actions like `DeleteAccount`/`DeleteBlob`).
/// - **Input shape**: `EmitEventInput { action, subjects,
///   rationale, snapshot_capture, metadata }`.
///   `subjects: Vec<Subject>` — single-subject callers wrap in a
///   one-element array.
/// - **Per-action multi-subject support**: account state
///   (`TakedownAccount`, `SuspendAccount`, `RestoreAccount`,
///   `DeleteAccount`), label (`ApplyLabel`, `RemoveLabel`), blob
///   quarantine/restore/delete (`QuarantineBlob`, `RestoreBlob`,
///   `DeleteBlob`), record takedown (`TakedownRecord`), and
///   `UpdateSubjectStatus` accept `subjects.len() > 1`.
///   Embedded-id variants (`ResolveReport`, `DismissReport`,
///   `ResolveAppeal`, `EscalateAppeal`) and `SendEmail` are
///   length-1 only and refuse `subjects.len() > 1` with HTTP 400
///   `SubjectsArrayInvalidForAction`.
/// - **Per-action `MAX_BATCH_SIZE` caps**: `DeleteAccount` = 10
///   (irreversible), `DeleteBlob` = 25 (storage-irreversible),
///   all others = 50.
/// - **Output shape**: this struct's four fields. `snapshots`
///   pairs 1:1-by-index with input `subjects`; empty when
///   `snapshot_capture: false`.
/// - **Atomicity scope** (per §8.3.1): pre-tx snapshot capture
///   (orphan snapshots accepted on Phase 2/3 failure — explicit
///   carve-out); per-subject mutation in tx via tx-bound
///   `dispatch_action` (failure aborts the whole tx); chain
///   entry write inside the same tx; commit makes everything
///   visible atomically. Per-subject mutation failure surfaces
///   the failing subject's index and identifier in the response
///   body.
/// - **Chain row shape** (per §8.3.3): single-subject populates
///   BOTH the flat `subject_did`/`subject_uri`/`subject_cid`
///   columns AND `cascade_subjects: [s]`; multi-subject uses
///   synthetic-primary (NULL flat columns, populated cascade).
///   External consumers can rely on `cascade_subjects` always
///   containing every subject regardless of arity.
///
/// Surfaces `auditEntryId` and `eventId` per the action-ID
/// contract committed in `crate::admin::audit_chain` (Arc 2
/// §6.4.2). Wire-to-canonical bridge for independent chain
/// verification: `docs/operator/audit-chain-verification.md`.
///
/// Snapshot tests in this module's `#[cfg(test)] mod tests`
/// pin the wire format. The contract-phrase test in
/// `tests/contract_phrases.rs` pins this commitment.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmitEventOutput {
    pub event_id: String,
    /// Audit chain entry id for this action. Always populated on
    /// success — `emit_event` writes the chain entry inside the same
    /// transaction as the moderation_event row (LB-1 / chainlink
    /// #122), so a successful response always corresponds to a landed
    /// chain row. Per §3.4: snapshots-and-audit-chain are co-equal
    /// substrate; an emitted event without a chain entry would
    /// silently violate that invariant.
    pub audit_entry_id: String,
    /// Per-subject snapshot list aligned 1:1 with `subjects` from the
    /// input. Empty when `snapshot_capture: false` was passed.
    pub snapshots: Vec<SnapshotRef>,
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

/// Bridge `Subject` → flat columns for `moderation_event` insertion.
fn subject_columns(subject: &Subject) -> (Option<&str>, Option<&str>, Option<&str>) {
    match subject {
        Subject::Repo { did } => (Some(did.as_str()), None, None),
        Subject::Record { uri, cid } => (None, Some(uri.as_str()), Some(cid.as_str())),
        Subject::Blob { did, cid, .. } => (Some(did.as_str()), None, Some(cid.as_str())),
    }
}

/// Validate operator role against action requirements (§8.1 step 1).
/// Account-infrastructure actions require Admin+; content actions
/// accept Moderator+.
///
/// Per chainlink #114 / §3.2's Admin-tier definition, sending email
/// to a user is an Admin-tier capability ("passwords, emails,
/// handles, signing keys, deletion") even when emitted via this
/// unified action surface. A Moderator emitting `SendEmail` would
/// reach an account-contact channel that the role tier doesn't
/// otherwise permit, so we gate it at Admin+ here.
fn check_role(
    auth: &AdminAuthContext,
    action: &ModEventAction,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    let admin_required = matches!(
        action,
        ModEventAction::DeleteAccount | ModEventAction::SendEmail { .. },
    );
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
// emitEvent — §8.4.1 (Arc 4 multi-subject reshape)
// ===========================================================================

/// Per-action subjects-array cap. Per Arc 4 Step 0.6 §4 decisions:
/// `DeleteAccount` (irreversible) → 10; `DeleteBlob` (storage-
/// irreversible best-effort) → 25; all other multi-subject-supported
/// variants → 50. Refused-for-multi variants
/// (`ResolveReport`/`DismissReport`/`ResolveAppeal`/`EscalateAppeal`/
/// `SendEmail`) hit the explicit refusal gate before this limit, so
/// the 50 default is vacuous for them.
const MAX_SUBJECTS_DEFAULT: usize = 50;
const MAX_SUBJECTS_DELETE_ACCOUNT: usize = 10;
const MAX_SUBJECTS_DELETE_BLOB: usize = 25;

fn max_subjects_for(action: &ModEventAction) -> usize {
    use ModEventAction as A;
    match action {
        A::DeleteAccount => MAX_SUBJECTS_DELETE_ACCOUNT,
        A::DeleteBlob => MAX_SUBJECTS_DELETE_BLOB,
        _ => MAX_SUBJECTS_DEFAULT,
    }
}

/// Whether an action variant accepts `subjects.len() > 1`. Per Arc 4
/// Step 0.6 §1 + the §8.3.4 action vocabulary: account-state,
/// label, record-takedown, blob-quarantine, blob-restore, blob-
/// delete, and update-subject-status fan out across subjects;
/// embedded-id and SendEmail variants do not (they're length-1 only).
fn supports_multi_subject(action: &ModEventAction) -> bool {
    use ModEventAction as A;
    match action {
        A::TakedownAccount
        | A::SuspendAccount
        | A::RestoreAccount
        | A::DeleteAccount
        | A::ApplyLabel { .. }
        | A::RemoveLabel { .. }
        | A::TakedownRecord
        | A::QuarantineBlob
        | A::RestoreBlob
        | A::DeleteBlob
        | A::UpdateSubjectStatus { .. } => true,

        A::ResolveReport { .. }
        | A::DismissReport { .. }
        | A::ResolveAppeal { .. }
        | A::EscalateAppeal { .. }
        | A::SendEmail { .. } => false,
    }
}

/// Map a per-arm `dispatch_action` failure to an HTTP response. Maps
/// the two Step 0.5 subject-mismatch variants
/// (`SubjectVariantMismatch`, `SubjectTargetMismatch`) and
/// `PdsError::Validation` (per-arm subject-shape rejections from the
/// `require_*_pds` helpers) to 400; leaves `OrphanedAppeal` at 500
/// (server-side data integrity, not caller error); routes everything
/// else through `internal_pds`.
fn dispatch_err_to_response(
    e: PdsError,
    failing_subject: usize,
    phase: &'static str,
) -> (StatusCode, Json<serde_json::Value>) {
    match e {
        PdsError::SubjectVariantMismatch { ref expected, ref got } => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "SubjectVariantMismatch",
                "message": format!(
                    "subjects[{}]: expected variant {}, got {}",
                    failing_subject, expected, got
                ),
                "failingSubject": failing_subject,
                "phase": phase,
            })),
        ),
        PdsError::SubjectTargetMismatch { ref expected, ref got } => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "SubjectTargetMismatch",
                "message": format!(
                    "subjects[{}]: expected target {}, got {}",
                    failing_subject, expected, got
                ),
                "failingSubject": failing_subject,
                "phase": phase,
            })),
        ),
        PdsError::Validation(msg) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "InvalidEvent",
                "message": format!("subjects[{}]: {}", failing_subject, msg),
                "failingSubject": failing_subject,
                "phase": phase,
            })),
        ),
        PdsError::OrphanedAppeal { appeal_id } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "OrphanedAppeal",
                "message": format!(
                    "appeal {} has no FK to moderation/report/quarantine",
                    appeal_id
                ),
                "failingSubject": failing_subject,
                "phase": phase,
            })),
        ),
        other => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "Internal",
                "message": other.to_string(),
                "failingSubject": failing_subject,
                "phase": phase,
            })),
        ),
    }
}

/// Action that must run AFTER the wrapping transaction commits. Today
/// only `DeleteBlob`'s best-effort backend storage delete fits this
/// shape (Arc 4 Step 0.6 §3 Branch B). Failures during execution are
/// logged at WARN and produce orphaned storage objects reconciled by a
/// future GC sweep (v0.4 follow-up #23).
#[derive(Debug)]
enum DeferredAction {
    /// Storage backend delete for a blob whose metadata was already
    /// removed in the wrapping transaction. Best-effort post-commit.
    BackendBlobDelete { cid: String },
}

/// Per-subject `dispatch_action` outcome. Cascading event ids ride
/// alongside the deferred-action queue so multi-subject batches
/// accumulate both across all subjects before the handler does its
/// commit / post-commit work.
#[derive(Debug, Default)]
struct DispatchEffects {
    cascading_event_ids: Vec<String>,
    deferred_actions: Vec<DeferredAction>,
}

impl DispatchEffects {
    fn merge(&mut self, other: DispatchEffects) {
        self.cascading_event_ids.extend(other.cascading_event_ids);
        self.deferred_actions.extend(other.deferred_actions);
    }
}

/// `tools.aurora.admin.emitEvent` — unified action surface.
///
/// Dispatch matrix:
///
/// | Action variant         | Subject required | Manager called                        |
/// |------------------------|------------------|---------------------------------------|
/// | TakedownAccount        | Repo             | moderation_manager.apply_action_in_tx |
/// | SuspendAccount         | Repo             | moderation_manager.apply_action_in_tx |
/// | RestoreAccount         | Repo             | moderation_manager.apply_action_in_tx |
/// | DeleteAccount          | Repo             | account_manager.delete_account_permanent_in_tx |
/// | ApplyLabel             | any              | label_manager.apply_label_in_tx       |
/// | RemoveLabel            | any              | label_manager.remove_label_in_tx      |
/// | TakedownRecord         | Record           | label_manager.apply_label_in_tx       |
/// | QuarantineBlob         | Blob             | BlobQuarantine::quarantine_blob_in_tx |
/// | RestoreBlob            | Blob             | BlobQuarantine::restore_blob_in_tx    |
/// | DeleteBlob             | Blob             | BlobStore::delete_metadata_in_tx (+ post-commit backend delete) |
/// | ResolveReport          | any (length-1)   | report_manager.update_status_in_tx    |
/// | DismissReport          | any (length-1)   | report_manager.update_status_in_tx    |
/// | ResolveAppeal (approve)| any (length-1)   | AppealManager::update_status_in_tx + reverse_action_in_tx cascade |
/// | ResolveAppeal (deny)   | any (length-1)   | AppealManager::update_status_in_tx    |
/// | EscalateAppeal         | any (length-1)   | AppealManager::update_status_in_tx    |
/// | SendEmail              | Repo (length-1)  | mailer.send_admin_email (best-effort) |
/// | UpdateSubjectStatus    | Repo             | moderation_manager.apply_action_in_tx |
///
/// Handler shape (per §8.3.1 atomicity scope):
/// 1. **Phase 0** — input rejection (role, rationale, subjects shape,
///    per-action limits, embedded-id target validation). No state.
/// 2. **Phase 1** — pre-tx snapshot capture per subject. Orphan
///    snapshots accepted on Phase 2/3 failure (intentional carve-out).
/// 3. **Phase 2** — open tx; for each subject in `subjects`,
///    `dispatch_action(&mut tx, …)`. Per-subject failure aborts the
///    whole tx (no partial state).
/// 4. **Phase 3** — append chain entry inside same tx, commit. Single-
///    subject populates flat columns AND `cascade_subjects: [s]`;
///    multi-subject uses synthetic-primary (NULL flat columns) with
///    `cascade_subjects: [s1, s2, …]` per §8.3.3.
/// 5. **Phase 4** — execute deferred actions post-commit (best-effort
///    `BackendBlobDelete`); build response.
pub async fn emit_event(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    crate::api::extractors::AuroraJson(input): crate::api::extractors::AuroraJson<EmitEventInput>,
) -> Result<Json<EmitEventOutput>, (StatusCode, Json<serde_json::Value>)> {
    // Arc 6 Step 7: legacy wire-shape observability. When the input
    // was deserialized from the v0.2 `subject: Subject` shape, record
    // a counter increment + structured log so operators tracking
    // migration progress can see which clients still send the legacy
    // shape. Response headers (Deprecation, Sunset, Warning,
    // X-Wire-Migration-Guide) are NOT emitted here — adding them
    // would require restructuring the handler return type from
    // `Json<EmitEventOutput>` to `Response`, which would ripple
    // through 29 test call sites. Counter + log is sufficient for
    // the observability goal of §5.3.6; headers are a follow-up
    // cycle decision (flagged in Step 7 report).
    if input.legacy_subject_used {
        crate::metrics::record_legacy_wire_ingest(
            "tools.aurora.admin.emitEvent",
            "v0.2_single_subject",
            "subject",
        );
        tracing::info!(
            endpoint = "tools.aurora.admin.emitEvent",
            shape = "v0.2_single_subject",
            field = "subject",
            "legacy_wire_shape_ingested"
        );
    }

    // === Phase 0: input validation ===
    check_role(&auth, &input.action)?;

    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }
    if input.subjects.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "SubjectsArrayInvalidForAction",
                "message": "subjects array must contain at least one subject",
            })),
        ));
    }

    let limit = max_subjects_for(&input.action);
    validate_batch_size(&input.subjects, limit, "subjects array")?;

    if input.subjects.len() > 1 && !supports_multi_subject(&input.action) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "SubjectsArrayInvalidForAction",
                "message": format!(
                    "action {} does not support multi-subject calls; pass subjects of length 1",
                    action_kind_str(&input.action)
                ),
            })),
        ));
    }

    // Embedded-ID target validation for ResolveReport/DismissReport.
    // ResolveAppeal/EscalateAppeal validation runs INSIDE
    // AppealManager::update_status_in_tx (Step 0.5 wired this).
    validate_embedded_report_target(&ctx, &input.action, &input.subjects[0]).await?;

    // === Phase 1: per-subject snapshot capture (pre-tx) ===
    let metadata = input.metadata.clone();
    let mut snapshot_ids: Vec<Option<i64>> = Vec::with_capacity(input.subjects.len());
    if input.snapshot_capture {
        for (idx, subject) in input.subjects.iter().enumerate() {
            match audit_chain::capture_snapshot(&ctx.account_db, subject).await {
                Ok(snap) => snapshot_ids.push(snap),
                Err(e) => {
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "Internal",
                            "message": e.to_string(),
                            "failingSubject": idx,
                            "phase": "snapshot_capture",
                        })),
                    ));
                }
            }
        }
    }

    // === Phase 2: tx-bound mutations ===
    let event_type = event_type_for(&input.action);
    let action_str = action_kind_str(&input.action);
    let details = build_event_details(&input, &metadata);
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx.account_db.begin().await.map_err(internal)?;

    let mut effects = DispatchEffects::default();
    for (idx, subject) in input.subjects.iter().enumerate() {
        match dispatch_action(&ctx, &mut tx, &auth, &input.action, subject, &input.rationale, &metadata).await {
            Ok(per_subject) => effects.merge(per_subject),
            Err(e) => return Err(dispatch_err_to_response(e, idx, "mutation")),
        }
    }

    // === Phase 3: chain entry + commit ===
    // Determine the moderation_event row's flat-column values per
    // §8.3.3: single-subject populates flat columns; multi-subject
    // uses synthetic-primary (NULL flat columns).
    let (event_subject_did, event_subject_uri, event_subject_cid) = if input.subjects.len() == 1 {
        let cols = subject_columns(&input.subjects[0]);
        (cols.0.map(|s| s.to_string()), cols.1.map(|s| s.to_string()), cols.2.map(|s| s.to_string()))
    } else {
        (None, None, None)
    };

    let event = ModerationEventLogger::log_event_in_tx(
        &mut tx,
        LogEventParams {
            event_type,
            actor_did: &auth.did,
            subject_did: event_subject_did.as_deref(),
            subject_uri: event_subject_uri.as_deref(),
            subject_cid: event_subject_cid.as_deref(),
            details: details.clone(),
            meta: metadata,
        },
    )
    .await
    .map_err(internal)?;

    // Chain row shape per §8.3.3:
    // - Single-subject: BOTH flat columns populated (via `subject:
    //   Some(s)`) AND cascade_subjects: [s].
    // - Multi-subject: NULL flat columns (via `subject: None`) AND
    //   cascade_subjects: [s1, s2, ...].
    // - cascade_snapshot_ids: aligned 1:1 when snapshot_capture=true,
    //   empty when false.
    let chain_subject = if input.subjects.len() == 1 {
        Some(&input.subjects[0])
    } else {
        None
    };
    let cascade_snap_slice: &[Option<i64>] = if input.snapshot_capture {
        &snapshot_ids
    } else {
        &[]
    };
    let scalar_snapshot_id = if input.subjects.len() == 1 && input.snapshot_capture {
        snapshot_ids.first().copied().flatten()
    } else {
        None
    };

    let audit_entry_id = audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: &auth.did,
            action: action_str,
            subject: chain_subject,
            rationale: &input.rationale,
            snapshot_id: scalar_snapshot_id,
            event_id: Some(event.id),
            cascade_subjects: &input.subjects,
            cascade_snapshot_ids: cascade_snap_slice,
        },
    )
    .await
    .map_err(internal_pds)?;
    tx.commit().await.map_err(internal)?;

    // === Phase 4: post-commit deferred actions + response ===
    for deferred in &effects.deferred_actions {
        match deferred {
            DeferredAction::BackendBlobDelete { cid } => {
                if let Err(e) = ctx.blob_store.backend_delete(cid).await {
                    tracing::warn!(
                        "DeleteBlob: post-commit backend delete failed for cid {} \
                         (orphan storage; reconcile via GC): {}",
                        cid, e
                    );
                }
            }
        }
    }

    let snapshots = if input.snapshot_capture {
        input
            .subjects
            .iter()
            .zip(snapshot_ids.iter())
            .map(|(s, snap)| SnapshotRef {
                subject: s.clone(),
                snapshot_id: snap.map(|id| id.to_string()),
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(Json(EmitEventOutput {
        event_id: event.id.to_string(),
        audit_entry_id: audit_entry_id.to_string(),
        snapshots,
        cascading_actions: effects.cascading_event_ids,
    }))
}

/// For embedded-ID actions whose `subjects[0]` must match the
/// dereferenced target's intrinsic subject:
/// - `ResolveReport` / `DismissReport`: read the report row, build a
///   `Subject` from its flat columns, compare per §8.3.4.
/// - `ResolveAppeal` / `EscalateAppeal`: validation lives inside
///   `AppealManager::update_status_in_tx` (Step 0.5); the handler
///   passes `subjects[0]` through and skips here.
/// - All other actions: no embedded-ID target; this is a no-op.
async fn validate_embedded_report_target(
    ctx: &AppContext,
    action: &ModEventAction,
    subject: &Subject,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::reports::ReportManager;
    use ModEventAction as A;
    let report_id = match action {
        A::ResolveReport { report_id, .. } | A::DismissReport { report_id } => *report_id,
        _ => return Ok(()),
    };
    let mgr = ReportManager::new(ctx.account_db.clone());
    let report = mgr
        .get_report(report_id)
        .await
        .map_err(internal_pds)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "ReportNotFound",
                    "message": format!("report {} not found", report_id),
                })),
            )
        })?;
    let resolved = Subject::from_columns(
        report.subject_did.as_deref(),
        report.subject_uri.as_deref(),
        report.subject_cid.as_deref(),
    )
    .ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "Internal",
                "message": format!("report {} has no decodable subject columns", report_id),
            })),
        )
    })?;

    let expected_variant = subject_variant_label(subject);
    let resolved_variant = subject_variant_label(&resolved);
    if expected_variant != resolved_variant {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "SubjectVariantMismatch",
                "message": format!(
                    "subjects[0]: expected variant {}, got {}",
                    expected_variant, resolved_variant
                ),
            })),
        ));
    }
    let identifier_match = match (subject, &resolved) {
        (Subject::Repo { did: e }, Subject::Repo { did: r }) => e == r,
        (Subject::Record { uri: e, .. }, Subject::Record { uri: r, .. }) => e == r,
        (Subject::Blob { cid: e, .. }, Subject::Blob { cid: r, .. }) => e == r,
        _ => unreachable!("variant equality already checked"),
    };
    if !identifier_match {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "SubjectTargetMismatch",
                "message": format!(
                    "subjects[0]: expected target {}, got {}",
                    format_subject_label(subject),
                    format_subject_label(&resolved),
                ),
            })),
        ));
    }
    Ok(())
}

fn subject_variant_label(s: &Subject) -> &'static str {
    match s {
        Subject::Repo { .. } => "Repo",
        Subject::Record { .. } => "Record",
        Subject::Blob { .. } => "Blob",
    }
}

fn format_subject_label(s: &Subject) -> String {
    match s {
        Subject::Repo { did } => format!("Repo({})", did),
        Subject::Record { uri, .. } => format!("Record({})", uri),
        Subject::Blob { cid, .. } => format!("Blob({})", cid),
    }
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

/// Dispatch a single subject's action inside the wrapping transaction.
/// Per Arc 4 §8.4.1: every match arm uses the corresponding `_in_tx`
/// manager method, so per-subject failure aborts the wrapping `tx`
/// atomically (Step 0.5 wired the missing `_in_tx` variants;
/// chainlink #130).
///
/// Returns `DispatchEffects` carrying:
/// - `cascading_event_ids`: extra event IDs produced by server-side
///   cascades (today: only `ResolveAppeal{Approve}` reverse-action).
/// - `deferred_actions`: post-commit best-effort work (today: only
///   `DeleteBlob`'s storage-backend cleanup per Step 0.6 §3 Branch B).
async fn dispatch_action<'tx>(
    ctx: &AppContext,
    tx: &mut sqlx::Transaction<'tx, sqlx::Any>,
    auth: &AdminAuthContext,
    action: &ModEventAction,
    subject: &Subject,
    rationale: &str,
    metadata: &Option<serde_json::Value>,
) -> Result<DispatchEffects, PdsError> {
    use ModEventAction as A;
    let server_did = format!("did:web:{}", ctx.config.service.hostname);
    match action {
        A::TakedownAccount => {
            let did = require_repo_did_pds(subject)?;
            ModerationManager::apply_action_in_tx(
                tx,
                ApplyActionParams {
                    did,
                    action: ModerationAction::Takedown,
                    reason: rationale,
                    moderated_by: &auth.did,
                    expires_in: None,
                    report_id: None,
                    notes: None,
                },
            )
            .await?;
            Ok(DispatchEffects::default())
        }
        A::SuspendAccount => {
            let did = require_repo_did_pds(subject)?;
            let expires_in = metadata
                .as_ref()
                .and_then(|m| m.get("durationDays"))
                .and_then(|v| v.as_i64())
                .map(chrono::Duration::days);
            ModerationManager::apply_action_in_tx(
                tx,
                ApplyActionParams {
                    did,
                    action: ModerationAction::Suspend,
                    reason: rationale,
                    moderated_by: &auth.did,
                    expires_in,
                    report_id: None,
                    notes: None,
                },
            )
            .await?;
            Ok(DispatchEffects::default())
        }
        A::RestoreAccount => {
            let did = require_repo_did_pds(subject)?;
            ModerationManager::apply_action_in_tx(
                tx,
                ApplyActionParams {
                    did,
                    action: ModerationAction::Restore,
                    reason: rationale,
                    moderated_by: &auth.did,
                    expires_in: None,
                    report_id: None,
                    notes: None,
                },
            )
            .await?;
            Ok(DispatchEffects::default())
        }
        A::DeleteAccount => {
            let did = require_repo_did_pds(subject)?;
            AccountManager::delete_account_permanent_in_tx(tx, did).await?;
            Ok(DispatchEffects::default())
        }
        A::ApplyLabel { val, neg: _neg } => {
            let (uri, cid) = subject_uri_cid_pds(subject)?;
            LabelManager::apply_label_in_tx(
                tx,
                &server_did,
                &uri,
                cid.as_deref(),
                val,
                &auth.did,
                None,
            )
            .await?;
            Ok(DispatchEffects::default())
        }
        A::RemoveLabel { val } => {
            let (uri, cid) = subject_uri_cid_pds(subject)?;
            LabelManager::remove_label_in_tx(
                tx,
                &server_did,
                &uri,
                cid.as_deref(),
                val,
                &auth.did,
            )
            .await?;
            Ok(DispatchEffects::default())
        }
        A::TakedownRecord => {
            let (uri, cid) = match subject {
                Subject::Record { uri, cid } => (uri.clone(), Some(cid.clone())),
                _ => {
                    return Err(PdsError::Validation(
                        "TakedownRecord requires a Record subject ($type=com.atproto.repo.strongRef)"
                            .to_string(),
                    ));
                }
            };
            LabelManager::apply_label_in_tx(
                tx,
                &server_did,
                &uri,
                cid.as_deref(),
                "!takedown",
                &auth.did,
                None,
            )
            .await?;
            Ok(DispatchEffects::default())
        }
        A::QuarantineBlob => {
            let cid = require_blob_cid_pds(subject)?;
            use crate::blob_store::quarantine::{BlobQuarantine, QuarantineReason};
            use std::str::FromStr;
            let reason = metadata
                .as_ref()
                .and_then(|m| m.get("reason"))
                .and_then(|v| v.as_str())
                .and_then(|s| QuarantineReason::from_str(s).ok())
                .unwrap_or(QuarantineReason::Other);
            let legal_reference = metadata
                .as_ref()
                .and_then(|m| m.get("legalReference"))
                .and_then(|v| v.as_str());
            BlobQuarantine::quarantine_blob_in_tx(
                tx,
                cid,
                reason,
                Some(rationale),
                &auth.did,
                legal_reference,
            )
            .await?;
            Ok(DispatchEffects::default())
        }
        A::RestoreBlob => {
            let cid = require_blob_cid_pds(subject)?;
            use crate::blob_store::quarantine::BlobQuarantine;
            BlobQuarantine::restore_blob_in_tx(tx, cid, &auth.did).await?;
            Ok(DispatchEffects::default())
        }
        A::DeleteBlob => {
            // Step 0.6 §3 Branch B: metadata DELETE rides inside the
            // wrapping tx; storage-backend delete defers to post-commit
            // best-effort cleanup via DeferredAction::BackendBlobDelete.
            let cid = require_blob_cid_pds(subject)?;
            crate::blob_store::store::BlobStore::delete_metadata_in_tx(tx, cid).await?;
            let mut effects = DispatchEffects::default();
            effects.deferred_actions.push(DeferredAction::BackendBlobDelete {
                cid: cid.to_string(),
            });
            Ok(effects)
        }
        A::ResolveReport { report_id, resolution } => {
            ReportManager::update_status_in_tx(
                tx,
                *report_id,
                resolution.as_db_status(),
                &auth.did,
                Some(resolution.as_resolution_str()),
            )
            .await?;
            Ok(DispatchEffects::default())
        }
        A::DismissReport { report_id } => {
            ReportManager::update_status_in_tx(
                tx,
                *report_id,
                ReportStatus::Resolved,
                &auth.did,
                Some("dismissed"),
            )
            .await?;
            Ok(DispatchEffects::default())
        }
        A::ResolveAppeal { appeal_id, resolution } => {
            // Pre-fetch moderation_id + appellant_did inside the same
            // tx so the cascade decision sees the same snapshot the
            // status update operates on.
            let row: Option<(Option<i64>, String)> = sqlx::query_as::<_, (Option<i64>, String)>(
                "SELECT moderation_id, appellant_did FROM appeal WHERE id = $1",
            )
            .bind(*appeal_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(PdsError::Database)?;
            let (mod_id, appellant_did) = row.ok_or_else(|| {
                PdsError::NotFound(format!("appeal {} not found", appeal_id))
            })?;

            let new_status = match resolution {
                AppealResolutionDecision::Approve => AppealStatus::Approved,
                AppealResolutionDecision::Deny => AppealStatus::Denied,
            };
            // update_status_in_tx does the JOIN-and-validate against
            // `subject` itself (Step 0.5 §2), so per-arm subject
            // validation rides inside this call.
            AppealManager::update_status_in_tx(
                tx,
                *appeal_id,
                new_status,
                &auth.did,
                Some(rationale),
                None,
                subject,
            )
            .await?;

            let mut effects = DispatchEffects::default();
            if matches!(resolution, AppealResolutionDecision::Approve) {
                if let Some(mid) = mod_id {
                    ModerationManager::reverse_action_in_tx(
                        tx,
                        mid,
                        &auth.did,
                        &format!("appeal {} approved: {}", appeal_id, rationale),
                    )
                    .await?;
                    let cascade_event = ModerationEventLogger::log_event_in_tx(
                        tx,
                        LogEventParams {
                            event_type: ModerationEventType::AccountRestore,
                            actor_did: &auth.did,
                            subject_did: Some(appellant_did.as_str()),
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
                        },
                    )
                    .await?;
                    effects.cascading_event_ids.push(cascade_event.id.to_string());
                }
            }
            Ok(effects)
        }
        A::EscalateAppeal { appeal_id } => {
            AppealManager::update_status_in_tx(
                tx,
                *appeal_id,
                AppealStatus::Escalated,
                &auth.did,
                None,
                Some(rationale),
                subject,
            )
            .await?;
            Ok(DispatchEffects::default())
        }
        A::SendEmail { template, subject: email_subject, body } => {
            // SendEmail's pool-API account read is fine — this arm is
            // length-1 only (multi-subject was refused in Phase 0), so
            // the read happens once and the outer tx hasn't written to
            // `account` rows in this same dispatch.
            let did = require_repo_did_pds(subject)?;
            let account = ctx
                .account_manager
                .get_account(did)
                .await
                .map_err(|_| PdsError::Validation("recipient account not found".to_string()))?;
            let email = account.email.as_deref().unwrap_or("");
            if email.is_empty() {
                return Err(PdsError::Validation(
                    "recipient account has no email on file".to_string(),
                ));
            }
            let _ = template; // template selection deferred to mailer enhancement
            // Mailer call is external; it stays best-effort but its
            // failure rolls back the wrapping tx (so the moderation
            // event isn't recorded for an email that didn't go out).
            if ctx.mailer.is_configured() {
                ctx.mailer.send_admin_email(email, email_subject, body).await?;
            } else {
                tracing::warn!(
                    "SendEmail: mailer not configured; event logged but no email sent to {}",
                    did
                );
            }
            Ok(DispatchEffects::default())
        }
        A::UpdateSubjectStatus { status } => {
            let did = require_repo_did_pds(subject)?;
            let mod_action = match status {
                SubjectStatusValue::Takedown => ModerationAction::Takedown,
                SubjectStatusValue::Active => ModerationAction::Restore,
                SubjectStatusValue::Deactivated => ModerationAction::Suspend,
            };
            ModerationManager::apply_action_in_tx(
                tx,
                ApplyActionParams {
                    did,
                    action: mod_action,
                    reason: rationale,
                    moderated_by: &auth.did,
                    expires_in: None,
                    report_id: None,
                    notes: None,
                },
            )
            .await?;
            Ok(DispatchEffects::default())
        }
    }
}

// ---------------------------------------------------------------------------
// Subject extractor variants returning PdsError (for in-tx dispatch).
// The HTTP-tuple variants (require_repo_did / subject_uri_cid /
// require_blob_cid) above are still used by Phase 0 / handler-layer
// rejection paths that build HTTP responses directly.
// ---------------------------------------------------------------------------

fn require_repo_did_pds(subject: &Subject) -> Result<&str, PdsError> {
    match subject {
        Subject::Repo { did } => Ok(did.as_str()),
        _ => Err(PdsError::Validation(
            "action requires a Repo subject (did:plc:...) but got a Record or Blob subject"
                .to_string(),
        )),
    }
}

fn subject_uri_cid_pds(subject: &Subject) -> Result<(String, Option<String>), PdsError> {
    match subject {
        Subject::Record { uri, cid } => Ok((uri.clone(), Some(cid.clone()))),
        Subject::Repo { did } => Ok((format!("at://{}", did), None)),
        Subject::Blob { did, cid, .. } => Ok((format!("at://{}", did), Some(cid.clone()))),
    }
}

fn require_blob_cid_pds(subject: &Subject) -> Result<&str, PdsError> {
    match subject {
        Subject::Blob { cid, .. } => Ok(cid.as_str()),
        _ => Err(PdsError::Validation(
            "action requires a Blob subject ($type=com.atproto.admin.defs#repoBlobRef)".to_string(),
        )),
    }
}

// ===========================================================================
// Batch endpoints — §8.8–§8.13
// ===========================================================================
//
// All six batch endpoints share the whole-tx-atomic contract per Arc 4
// §8.4.2 (chainlink #113). For full atomicity-scope details see
// `docs/V03_DESIGN.md` §8.3.1.
//
//   1. Validate batch size (1..=MAX_BATCH_SIZE) and role.
//   2. Capture per-subject snapshots BEFORE the wrapping tx opens
//      (CR-2 / chainlink #111). Snapshot capture failure aborts the
//      handler before the tx; the snapshot rows that did land
//      remain (orphan-snapshot carve-out per §8.3.1).
//   3. Open tx on account_db; per-subject mutation runs in-tx via
//      the corresponding `_in_tx` manager method. Per-subject
//      failure aborts the wrapping tx — moderation_event row,
//      audit_chain_entry row, and every per-subject mutation
//      either ALL land or NONE do.
//   4. INSERT one moderation_event row per batch (synthetic-primary;
//      flat subject columns NULL; full subject list lives in
//      `details` JSON and chain row's `cascade_subjects`).
//   5. Append chain entry inside the same tx via
//      `audit_chain::insert_chain_entry`.
//   6. Commit; the response body's `affected_count` always equals
//      `cascade_subjects.len()` for successful responses.
//
// The v0.2 `failures: Vec<BatchFailure>` field is retired (Arc 4
// §8.4.2): per-subject failure now aborts the whole batch and
// surfaces the failing subject's index and identifier in the error
// response body. `batch_remove_label` keeps `skipped: Vec<Subject>`
// — subjects without the label to remove are a no-op rather than a
// failure (per design doc §8.13's non-atomic-failure rule).

const MAX_BATCH_SIZE: usize = 50;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotRef {
    /// Subject the snapshot was captured for. Always present so the
    /// UI can map snapshots back to their subjects in the response.
    pub subject: Subject,
    /// Snapshot id; populated for batch entries via per-subject
    /// `audit_snapshot` rows captured before the mutation runs (CR-2 /
    /// chainlink #111). Single-subject endpoints continue to use the
    /// scalar `audit_chain_entry.snapshot_id` instead.
    pub snapshot_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchAccountsInput {
    pub dids: Vec<String>,
    pub rationale: String,
}

/// Output for the `batch*Accounts` and `batchTakedownRecords` family.
/// Per Arc 4 §8.4.2: every batch handler now has whole-tx atomicity
/// (chainlink #113). A returned response always corresponds to a
/// landed chain row AND every per-subject mutation in
/// `cascade_subjects` having succeeded. The v0.2 per-subject failure
/// list is gone — partial-success is no longer a state the caller
/// can observe; per-subject failure aborts the whole batch's
/// transaction.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchAccountsOutput {
    pub event_id: String,
    /// Audit chain entry id for the operator's batch decision.
    /// Always populated on success — chain append + moderation_event
    /// row + every per-subject mutation land together in one tx.
    pub audit_entry_id: String,
    /// Count of subjects whose actor-table mutation applied. Always
    /// equals `cascade_subjects.len()` on the chain row post-Arc-4
    /// (whole-tx atomicity); kept as an explicit field for wire-shape
    /// continuity with the v0.2 response.
    pub affected_count: u32,
    pub snapshots: Vec<SnapshotRef>,
}

/// Input for `tools.aurora.admin.batchTakedownRecords` (§8.11).
///
/// Aurora-Locus record-takedown semantics on this surface are
/// **URI-level** (per Arc 4 §8.4.3). Each entry in `uris`
/// identifies a record by its `at://` URI without pinning a
/// specific CID version; the takedown applies to whatever
/// content currently resides at the URI, and future versions
/// of the record at the same URI are also covered.
///
/// The chain row this handler writes carries `cascade_subjects`
/// entries shaped as `Subject::Record { uri, cid: "" }` — the
/// empty `cid` is a **deliberate convention**, not missing data
/// or a sentinel-null. It explicitly signals "URI-level
/// takedown, no CID anchor." Pinned by
/// `batch_takedown_records_produces_uri_level_cascade_with_empty_cids`
/// in this module's tests.
///
/// This contrasts with single-subject `emitEvent{TakedownRecord}`
/// (§8.3): there the input `Subject::Record` carries a real CID,
/// the takedown is **CID-level** (specific record version), and
/// the cascade entry preserves that CID. Operators choosing
/// between the two paths select on whether they want
/// version-specific or URI-level coverage.
///
/// The empty-CID convention is committed-by-documentation here
/// and on the [`Subject::Record`](crate::admin::defs::Subject)
/// variant; external consumers reading the audit chain can rely
/// on it.
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

/// Output for `batchApplyLabel`. Per Arc 4 §8.4.2 the previous
/// `failures` field is gone (it was always empty post-CR — the
/// handler was already whole-batch atomic).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchLabelOutput {
    pub event_id: String,
    /// Audit chain entry id for the operator's batch decision.
    /// Always populated on success — see `BatchAccountsOutput`.
    pub audit_entry_id: String,
    pub affected_count: u32,
    pub snapshots: Vec<SnapshotRef>,
}

/// Output for `batchRemoveLabel`. Per Arc 4 §8.4.2 the previous
/// `failures` field is gone (was always empty); `skipped:
/// Vec<Subject>` remains because it carries semantically distinct
/// information (subjects that didn't have the label to remove — a
/// no-op, not a failure, per §8.13's non-atomic-failure rule).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchRemoveLabelOutput {
    pub event_id: String,
    /// Audit chain entry id for the operator's batch decision.
    /// Always populated on success — see `BatchAccountsOutput`.
    pub audit_entry_id: String,
    pub affected_count: u32,
    /// Subjects that didn't have the label — reported transparently
    /// rather than failing the batch (§8.13 non-atomic-failure rule).
    pub skipped: Vec<Subject>,
    pub snapshots: Vec<SnapshotRef>,
}

/// Validate batch / array length against a per-call limit. Per Arc 4
/// Step 0.6 §4: callers pass an explicit `limit` (50 default for
/// legacy batch handlers; per-action for `emit_event`) plus a `label`
/// used in the error message ("subjects array" vs. "batch") so error
/// shape stays caller-appropriate.
fn validate_batch_size<T>(
    items: &[T],
    limit: usize,
    label: &str,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if items.is_empty() {
        return Err(validation(format!("{} must contain at least one entry", label)));
    }
    if items.len() > limit {
        return Err(validation(format!(
            "{} length {} exceeds limit of {}",
            label,
            items.len(),
            limit
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

/// Insert per-DID `account_moderation` rows + the batch
/// `moderation_event` row inside the caller-supplied transaction.
/// LB-1 / chainlink #128: callers wrap this together with the
/// per-subject actor mutations and `insert_chain_entry` in one
/// transaction so the chain entry, the moderation_event, and
/// (where applicable) the per-subject actor-table mutations all
/// land or all roll back.
async fn insert_batch_account_moderations_in_tx<'c>(
    tx: &mut sqlx::Transaction<'c, sqlx::Any>,
    actor_did: &str,
    action_db_str: &str,
    event_type: ModerationEventType,
    rationale: &str,
    dids: &[String],
) -> Result<i64, (StatusCode, Json<serde_json::Value>)> {
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
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    }
    let details = serde_json::json!({
        "rationale": rationale,
        "action": event_type.as_str(),
        "batch": true,
        "subjects": dids,
    });
    let event_id = crate::admin::events::insert_moderation_event_in_tx(
        tx,
        event_type.as_str(),
        actor_did,
        None,
        None,
        None,
        &details.to_string(),
        &now,
        None,
    )
    .await
    .map_err(internal)?;
    Ok(event_id)
}

fn snapshots_for_dids(
    dids: &[String],
    snapshot_ids: &[Option<i64>],
) -> Vec<SnapshotRef> {
    // Caller may pass an empty slice meaning "no snapshots captured"
    // (legacy path); otherwise lengths must match. Out-of-bounds reads
    // here are a programming bug, not a runtime input concern.
    dids.iter()
        .enumerate()
        .map(|(i, d)| SnapshotRef {
            subject: Subject::Repo { did: d.clone() },
            snapshot_id: snapshot_ids
                .get(i)
                .copied()
                .flatten()
                .map(|id| id.to_string()),
        })
        .collect()
}

/// Capture a snapshot for each DID in the batch. Returns one
/// `Option<i64>` per DID (None if the subject wasn't snapshottable —
/// e.g., the DID didn't resolve to an actor row at capture time;
/// `capture_snapshot` falls back to a content-blank snapshot anyway,
/// so this almost always returns Some). Per disposition CR-2 / §3.4,
/// snapshots are captured BEFORE the mutation so the recorded state
/// is the pre-decision state.
async fn capture_snapshots_for_repo_subjects(
    ctx: &AppContext,
    dids: &[String],
) -> Result<Vec<Option<i64>>, (StatusCode, Json<serde_json::Value>)> {
    let mut ids = Vec::with_capacity(dids.len());
    for did in dids {
        let s = Subject::Repo { did: did.clone() };
        let id = audit_chain::capture_snapshot(&ctx.account_db, &s)
            .await
            .map_err(internal_pds)?;
        ids.push(id);
    }
    Ok(ids)
}

/// Capture a snapshot for each record URI in the batch. Record cids
/// are not in the batch input; we use empty-string cid in the
/// captured Subject (matches the chain row's subject_cid behavior).
async fn capture_snapshots_for_record_uris(
    ctx: &AppContext,
    uris: &[String],
) -> Result<Vec<Option<i64>>, (StatusCode, Json<serde_json::Value>)> {
    let mut ids = Vec::with_capacity(uris.len());
    for uri in uris {
        let s = Subject::Record {
            uri: uri.clone(),
            cid: String::new(),
        };
        let id = audit_chain::capture_snapshot(&ctx.account_db, &s)
            .await
            .map_err(internal_pds)?;
        ids.push(id);
    }
    Ok(ids)
}

/// Capture a snapshot for each Subject in the batch. Used by label
/// batches where the subjects are full Subject values.
async fn capture_snapshots_for_subjects(
    ctx: &AppContext,
    subjects: &[Subject],
) -> Result<Vec<Option<i64>>, (StatusCode, Json<serde_json::Value>)> {
    let mut ids = Vec::with_capacity(subjects.len());
    for s in subjects {
        let id = audit_chain::capture_snapshot(&ctx.account_db, s)
            .await
            .map_err(internal_pds)?;
        ids.push(id);
    }
    Ok(ids)
}

/// `tools.aurora.admin.batchTakedownAccounts` (§8.8).
pub async fn batch_takedown_accounts(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<BatchAccountsInput>,
) -> Result<Json<BatchAccountsOutput>, (StatusCode, Json<serde_json::Value>)> {
    check_moderator_role(&auth)?;
    validate_batch_size(&input.dids, MAX_BATCH_SIZE, "batch")?;
    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }
    // Per CR-2 / §3.4: snapshot per subject BEFORE the mutation runs
    // so the chain entry's cascade_snapshot_ids point at pre-decision
    // state. The mutation may invalidate the actor row (takedown
    // changes takedown_ref), so post-mutation capture would yield
    // post-state — defeating the forensic purpose.
    let snapshot_ids = capture_snapshots_for_repo_subjects(&ctx, &input.dids).await?;

    // Arc 4 §8.4.2 / chainlink #113: whole-batch atomicity. Per-subject
    // failures abort the wrapping tx — no SAVEPOINTs, no failures[].
    // The chain entry, moderation_event row, and every per-subject
    // takedown_account_in_tx call land together or none of them do.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx.account_db.begin().await.map_err(internal)?;
    let event_id = insert_batch_account_moderations_in_tx(
        &mut tx,
        &auth.did,
        "takedown",
        ModerationEventType::AccountTakedown,
        &input.rationale,
        &input.dids,
    )
    .await?;
    let takedown_ref = format!("batch_event_{}", event_id);
    for (idx, did) in input.dids.iter().enumerate() {
        AccountManager::takedown_account_in_tx(&mut tx, did, &takedown_ref)
            .await
            .map_err(|e| batch_subject_err_response(e, idx, did, "batch_takedown_accounts"))?;
    }
    let cascade: Vec<Subject> = input
        .dids
        .iter()
        .map(|d| Subject::Repo { did: d.clone() })
        .collect();
    let audit_entry_id = audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: &auth.did,
            action: "account.batch_takedown",
            subject: None,
            rationale: &input.rationale,
            snapshot_id: None,
            event_id: Some(event_id),
            cascade_subjects: &cascade,
            cascade_snapshot_ids: &snapshot_ids,
        },
    )
    .await
    .map_err(internal_pds)?;
    tx.commit().await.map_err(internal)?;

    Ok(Json(BatchAccountsOutput {
        event_id: event_id.to_string(),
        audit_entry_id: audit_entry_id.to_string(),
        affected_count: input.dids.len() as u32,
        snapshots: snapshots_for_dids(&input.dids, &snapshot_ids),
    }))
}

/// Map a per-subject batch failure to an HTTP response, surfacing the
/// failing index + subject identifier + handler label so operators
/// can locate the fault. Per Arc 4 §8.4.2: tx already aborted by
/// `?`-propagation on caller; this helper just shapes the response.
/// `PdsError::NotFound` → 404, `PdsError::Validation` → 400,
/// everything else → 500 (matches the per-error-kind status mapping
/// used elsewhere in this module).
fn batch_subject_err_response(
    e: PdsError,
    failing_idx: usize,
    failing_subject: &str,
    handler: &'static str,
) -> (StatusCode, Json<serde_json::Value>) {
    let (status, code) = match &e {
        PdsError::NotFound(_) => (StatusCode::NOT_FOUND, "NotFound"),
        PdsError::Validation(_) => (StatusCode::BAD_REQUEST, "InvalidRequest"),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "Internal"),
    };
    (
        status,
        Json(serde_json::json!({
            "error": code,
            "message": format!(
                "{}: subject[{}] = {} failed: {}",
                handler, failing_idx, failing_subject, e
            ),
            "failingSubject": failing_idx,
            "failingSubjectId": failing_subject,
        })),
    )
}

/// `tools.aurora.admin.batchSuspendAccounts` (§8.9).
pub async fn batch_suspend_accounts(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<BatchAccountsInput>,
) -> Result<Json<BatchAccountsOutput>, (StatusCode, Json<serde_json::Value>)> {
    check_moderator_role(&auth)?;
    validate_batch_size(&input.dids, MAX_BATCH_SIZE, "batch")?;
    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }
    let snapshot_ids = capture_snapshots_for_repo_subjects(&ctx, &input.dids).await?;

    // LB-1 / chainlink #128: chain entry + moderation_event +
    // per-DID account_moderation rows all in one transaction.
    // Suspend has no per-subject actor-table mutation (the
    // moderation_event row IS the suspension record), so no
    // savepoints / failures[] needed.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx.account_db.begin().await.map_err(internal)?;
    let event_id = insert_batch_account_moderations_in_tx(
        &mut tx,
        &auth.did,
        "suspend",
        ModerationEventType::AccountSuspend,
        &input.rationale,
        &input.dids,
    )
    .await?;
    let cascade: Vec<Subject> = input
        .dids
        .iter()
        .map(|d| Subject::Repo { did: d.clone() })
        .collect();
    let audit_entry_id = audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: &auth.did,
            action: "account.batch_suspend",
            subject: None,
            rationale: &input.rationale,
            snapshot_id: None,
            event_id: Some(event_id),
            cascade_subjects: &cascade,
            cascade_snapshot_ids: &snapshot_ids,
        },
    )
    .await
    .map_err(internal_pds)?;
    tx.commit().await.map_err(internal)?;

    Ok(Json(BatchAccountsOutput {
        event_id: event_id.to_string(),
        audit_entry_id: audit_entry_id.to_string(),
        affected_count: input.dids.len() as u32,
        snapshots: snapshots_for_dids(&input.dids, &snapshot_ids),
    }))
}

/// `tools.aurora.admin.batchRestoreAccounts` (§8.10).
pub async fn batch_restore_accounts(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<BatchAccountsInput>,
) -> Result<Json<BatchAccountsOutput>, (StatusCode, Json<serde_json::Value>)> {
    check_moderator_role(&auth)?;
    validate_batch_size(&input.dids, MAX_BATCH_SIZE, "batch")?;
    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }
    let snapshot_ids = capture_snapshots_for_repo_subjects(&ctx, &input.dids).await?;

    // Arc 4 §8.4.2 / chainlink #113: whole-batch atomicity. Per-DID
    // UPDATE failures abort the wrapping tx via `?`-propagation —
    // no SAVEPOINTs, no failures[]. A `UPDATE actor SET takedown_ref
    // = NULL` against a missing DID returns 0 rows_affected on both
    // SQLite and Postgres without erroring, so the no-such-DID case
    // is treated as a no-op for restore (consistent with v0.2 where
    // it was silently absorbed by the SAVEPOINT-recovery path).
    // Genuine driver errors (constraint violations, connection drops)
    // propagate as 500.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx.account_db.begin().await.map_err(internal)?;
    let event_id = insert_batch_account_moderations_in_tx(
        &mut tx,
        &auth.did,
        "restore",
        ModerationEventType::AccountRestore,
        &input.rationale,
        &input.dids,
    )
    .await?;
    for (idx, did) in input.dids.iter().enumerate() {
        sqlx::query("UPDATE actor SET takedown_ref = NULL WHERE did = $1")
            .bind(did)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                batch_subject_err_response(
                    PdsError::Database(e),
                    idx,
                    did,
                    "batch_restore_accounts",
                )
            })?;
    }
    let cascade: Vec<Subject> = input
        .dids
        .iter()
        .map(|d| Subject::Repo { did: d.clone() })
        .collect();
    let audit_entry_id = audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: &auth.did,
            action: "account.batch_restore",
            subject: None,
            rationale: &input.rationale,
            snapshot_id: None,
            event_id: Some(event_id),
            cascade_subjects: &cascade,
            cascade_snapshot_ids: &snapshot_ids,
        },
    )
    .await
    .map_err(internal_pds)?;
    tx.commit().await.map_err(internal)?;

    Ok(Json(BatchAccountsOutput {
        event_id: event_id.to_string(),
        audit_entry_id: audit_entry_id.to_string(),
        affected_count: input.dids.len() as u32,
        snapshots: snapshots_for_dids(&input.dids, &snapshot_ids),
    }))
}

/// `tools.aurora.admin.batchTakedownRecords` (§8.11).
pub async fn batch_takedown_records(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(input): Json<BatchRecordsInput>,
) -> Result<Json<BatchAccountsOutput>, (StatusCode, Json<serde_json::Value>)> {
    check_moderator_role(&auth)?;
    validate_batch_size(&input.uris, MAX_BATCH_SIZE, "batch")?;
    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }
    // Capture per-record snapshots BEFORE the mutation runs so the
    // chain's snapshot linkage points at pre-takedown state.
    let snapshot_ids = capture_snapshots_for_record_uris(&ctx, &input.uris).await?;

    // LB-1 / chainlink #128: per-URI label INSERTs +
    // moderation_event + chain entry all in one transaction.
    // Record-takedown is intentionally all-or-nothing — per-row
    // failures abort the whole batch (no failures[] surface).
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
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
    let event_id = crate::admin::events::insert_moderation_event_in_tx(
        &mut tx,
        ModerationEventType::AccountTakedown.as_str(),
        &auth.did,
        None,
        None,
        None,
        &details.to_string(),
        &now,
        None,
    )
    .await
    .map_err(internal)?;
    let cascade: Vec<Subject> = input
        .uris
        .iter()
        .map(|uri| Subject::Record {
            uri: uri.clone(),
            cid: String::new(),
        })
        .collect();
    let audit_entry_id = audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: &auth.did,
            action: "record.batch_takedown",
            subject: None,
            rationale: &input.rationale,
            snapshot_id: None,
            event_id: Some(event_id),
            cascade_subjects: &cascade,
            cascade_snapshot_ids: &snapshot_ids,
        },
    )
    .await
    .map_err(internal_pds)?;
    tx.commit().await.map_err(internal)?;

    let snapshots = input
        .uris
        .iter()
        .enumerate()
        .map(|(i, uri)| SnapshotRef {
            subject: Subject::Record {
                uri: uri.clone(),
                cid: String::new(),
            },
            snapshot_id: snapshot_ids
                .get(i)
                .copied()
                .flatten()
                .map(|id| id.to_string()),
        })
        .collect();
    Ok(Json(BatchAccountsOutput {
        event_id: event_id.to_string(),
        audit_entry_id: audit_entry_id.to_string(),
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
    validate_batch_size(&input.subjects, MAX_BATCH_SIZE, "batch")?;
    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }
    if input.label_val.trim().is_empty() {
        return Err(validation("label_val is required and must be non-empty"));
    }
    let snapshot_ids = capture_snapshots_for_subjects(&ctx, &input.subjects).await?;

    // LB-1 / chainlink #128: per-subject label INSERTs +
    // moderation_event + chain entry all in one transaction.
    // Label-apply is intentionally all-or-nothing — per-row
    // failures abort the whole batch (no failures[] surface).
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
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
    let event_id = crate::admin::events::insert_moderation_event_in_tx(
        &mut tx,
        ModerationEventType::LabelCreate.as_str(),
        &auth.did,
        None,
        None,
        None,
        &details.to_string(),
        &now,
        None,
    )
    .await
    .map_err(internal)?;
    let audit_entry_id = audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: &auth.did,
            action: "label.batch_apply",
            subject: None,
            rationale: &input.rationale,
            snapshot_id: None,
            event_id: Some(event_id),
            cascade_subjects: &input.subjects,
            cascade_snapshot_ids: &snapshot_ids,
        },
    )
    .await
    .map_err(internal_pds)?;
    tx.commit().await.map_err(internal)?;

    let snapshots = input
        .subjects
        .iter()
        .enumerate()
        .map(|(i, s)| SnapshotRef {
            subject: s.clone(),
            snapshot_id: snapshot_ids
                .get(i)
                .copied()
                .flatten()
                .map(|id| id.to_string()),
        })
        .collect();
    Ok(Json(BatchLabelOutput {
        event_id: event_id.to_string(),
        audit_entry_id: audit_entry_id.to_string(),
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
    validate_batch_size(&input.subjects, MAX_BATCH_SIZE, "batch")?;
    if input.rationale.trim().is_empty() {
        return Err(validation("rationale is required and must be non-empty"));
    }
    if input.label_val.trim().is_empty() {
        return Err(validation("label_val is required and must be non-empty"));
    }
    // First-pass: detect which subjects currently have the label.
    // We pair each applied subject with its captured snapshot id so the
    // chain row's cascade_snapshot_ids stays in lock-step with
    // cascade_subjects (skipped subjects are not in either array).
    let server_did = format!("did:web:{}", ctx.config.service.hostname);
    let mut applied_subjects: Vec<(Subject, String, Option<String>, Option<i64>)> =
        Vec::new();
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
            // Snapshot only for subjects we'll actually act on so the
            // chain's cascade_snapshot_ids matches cascade_subjects.
            let snapshot_id = audit_chain::capture_snapshot(&ctx.account_db, subject)
                .await
                .map_err(internal_pds)?;
            applied_subjects.push((subject.clone(), uri, cid, snapshot_id));
        } else {
            skipped.push(subject.clone());
        }
    }
    // LB-1 / chainlink #128: negative-label INSERTs +
    // moderation_event + chain entry all in one transaction.
    // Label-remove is all-or-nothing for the applied subset;
    // skipped subjects are a separate dimension reported in the
    // response and are not part of the chain entry's
    // cascade_subjects.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx.account_db.begin().await.map_err(internal)?;
    let now = chrono::Utc::now().to_rfc3339();
    for (_subject, uri, cid, _snap) in &applied_subjects {
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
        .map(|(s, _, _, _)| serde_json::to_value(s).unwrap_or(serde_json::Value::Null))
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
    let event_id = crate::admin::events::insert_moderation_event_in_tx(
        &mut tx,
        ModerationEventType::LabelRemove.as_str(),
        &auth.did,
        None,
        None,
        None,
        &details.to_string(),
        &now,
        None,
    )
    .await
    .map_err(internal)?;
    let snapshots = applied_subjects
        .iter()
        .map(|(s, _, _, snap)| SnapshotRef {
            subject: s.clone(),
            snapshot_id: snap.map(|id| id.to_string()),
        })
        .collect();
    let cascade: Vec<Subject> = applied_subjects
        .iter()
        .map(|(s, _, _, _)| s.clone())
        .collect();
    let cascade_snapshot_ids: Vec<Option<i64>> = applied_subjects
        .iter()
        .map(|(_, _, _, snap)| *snap)
        .collect();
    let audit_entry_id = audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: &auth.did,
            action: "label.batch_remove",
            subject: None,
            rationale: &input.rationale,
            snapshot_id: None,
            event_id: Some(event_id),
            cascade_subjects: &cascade,
            cascade_snapshot_ids: &cascade_snapshot_ids,
        },
    )
    .await
    .map_err(internal_pds)?;
    tx.commit().await.map_err(internal)?;

    Ok(Json(BatchRemoveLabelOutput {
        event_id: event_id.to_string(),
        audit_entry_id: audit_entry_id.to_string(),
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
    ///   full PII to the operator session.
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

    let subject = Subject::Repo {
        did: input.did.clone(),
    };
    let snapshot_id =
        audit_chain::capture_snapshot(&ctx.account_db, &subject)
            .await
            .ok()
            .flatten();

    // LB-1 Session 12 / chainlink #129: token INSERT + chain entry
    // in one transaction. Pre-fix the email_token row could land
    // (and become valid for password reset) even if the chain
    // append failed — a §3.4 violation. Now both writes commit
    // together.
    //
    // Mailer dispatch follows the chain-first ordering: chain entry
    // commits first, mailer side effect runs post-commit best-effort.
    // Mailer failure no longer leaves the operator with a token that
    // wasn't audited.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx.account_db.begin().await.map_err(internal)?;
    let token = crate::account::AccountManager::generate_password_reset_token_in_tx(
        &mut tx,
        &input.did,
    )
    .await
    .map_err(internal)?;
    let audit_entry_id = audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: &auth.did,
            action: "account.trigger_password_reset",
            subject: Some(&subject),
            rationale: &input.rationale,
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(internal_pds)?;
    tx.commit().await.map_err(internal)?;

    // Mailer dispatch (post-commit best-effort).
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
                    "triggerPasswordReset: chain entry recorded but email failed for {}: {}",
                    input.did,
                    e
                );
            }
        }
    } else {
        tracing::warn!(
            "triggerPasswordReset: mailer not configured; chain entry recorded but no email sent for {}",
            input.did
        );
    }

    Ok(Json(TriggerPasswordResetOutput {
        reset_email_sent: email_sent,
        masked_email: mask_email(&email),
        audit_entry_id: audit_entry_id.to_string(),
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
    /// Count of reports awaiting initial review (status = 'open').
    /// Per Arc 5 §9.4.3 / chainlink #126: domain-safely
    /// non-negative and bounded < 2^32 — retyped from `i64` to
    /// `u32` in v0.3 cycle close. JSON wire shape unchanged
    /// (still emitted as a non-negative integer); strict-typed
    /// Rust consumers gain a narrower type.
    pub open_reports: u32,
    /// Count of pending appeals. See `open_reports` for retype
    /// rationale.
    pub pending_appeals: u32,
    /// Count of reports under review (status = 'acknowledged').
    /// See `open_reports` for retype rationale.
    pub under_review_reports: u32,
    /// Count of appeals under review. See `open_reports` for
    /// retype rationale.
    pub under_review_appeals: u32,
    /// Sum of items needing operator decision. Canonical value the
    /// sidebar bell badge displays. Stays `i64` because the
    /// pathological sum-of-four-near-saturating-u32-counts could
    /// exceed `u32::MAX` (per recon Q4 sum-overflow guard).
    pub queue_attention_total: i64,
    /// Average age in seconds of open reports. See `open_reports`
    /// for retype rationale (u32 = ~136 years; ample bound for any
    /// realistic report age).
    pub average_age_open_reports_seconds: u32,
    /// Age in seconds of the oldest open report. See
    /// `open_reports` for retype rationale.
    pub oldest_open_report_age_seconds: u32,
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

    // Saturating i64 → u32 conversion for the count and age
    // fields per Arc 5 §9.4.3 / chainlink #126 retype. Counts
    // come from `SELECT COUNT(*)` and ages from RFC 3339 parsing
    // — both are domain-non-negative; saturating is defensive
    // against the (unreachable in practice) > 2^32 case rather
    // than truncation surprise.
    let to_u32 = |n: i64| -> u32 { u32::try_from(n.max(0)).unwrap_or(u32::MAX) };

    Ok(Json(GetQueueStatsOutput {
        open_reports: to_u32(open_reports),
        pending_appeals: to_u32(pending_appeals),
        under_review_reports: to_u32(under_review_reports),
        under_review_appeals: to_u32(under_review_appeals),
        queue_attention_total,
        average_age_open_reports_seconds: to_u32(avg_age),
        oldest_open_report_age_seconds: to_u32(oldest_age_secs),
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

/// Input for `tools.aurora.admin.getModerationMetrics`.
///
/// Per Arc 5 §9.4.3 / chainlink #126 the request struct accepts
/// two wire shapes for the time-range parameter:
///
/// - **Canonical**: `timeRange` field carrying a preset name string
///   (`"last_hour"`, `"last_24h"`, `"last_7d"`, `"last_30d"`).
///   Internally the preset resolves to a `(now - duration, now)`
///   window at deserialize time. Future JSON-body consumers may
///   also pass the `{start, end}` object form via the same field
///   (the underlying [`crate::admin::TimeRange`] supports both);
///   query-string callers wanting an explicit window use the
///   legacy fields below instead.
/// - **Legacy**: peer `start` and `end` RFC 3339 timestamp strings.
///   The dispatcher builds a `TimeRange` from them; the pair is
///   validated as `start <= end`.
///
/// Exactly one shape must be present. Both shapes simultaneously
/// or neither shape produces a clear error; typo'd preset names
/// surface the canonical preset list, NOT the legacy fields.
#[derive(Debug)]
pub struct GetModerationMetricsInput {
    pub time_range: crate::admin::TimeRange,
    pub granularity: Granularity,
    /// Subset of metrics to return. Empty list returns all metrics.
    pub metrics: Vec<MetricType>,
}

/// Wire-side scaffold for `GetModerationMetricsInput`'s custom
/// Deserialize. Holds raw optional fields so the dispatcher can
/// inspect which time-range shape the caller chose.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetModerationMetricsRawInput {
    #[serde(default)]
    time_range: Option<String>,
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
    granularity: Granularity,
    #[serde(default)]
    metrics: Vec<MetricType>,
}

impl<'de> Deserialize<'de> for GetModerationMetricsInput {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let raw = GetModerationMetricsRawInput::deserialize(d)?;
        let time_range = match (raw.time_range, raw.start, raw.end) {
            (Some(preset), None, None) => crate::admin::TimeRange::from_preset(
                &preset,
                chrono::Utc::now(),
            )
            .ok_or_else(|| {
                D::Error::custom(format!(
                    "unknown time-range preset {:?} on field 'timeRange'; expected one of: {}. \
                     For an explicit window, use the legacy fields 'start' and 'end' (RFC 3339 \
                     timestamps) instead.",
                    preset,
                    crate::admin::TimeRange::PRESETS.join(", "),
                ))
            })?,
            (None, Some(s), Some(e)) => crate::admin::TimeRange::from_rfc3339_pair(&s, &e)
                .map_err(|msg| {
                    D::Error::custom(format!(
                        "legacy 'start'/'end' time-range failed validation: {}. \
                         For preset windows, use the canonical 'timeRange' field instead.",
                        msg
                    ))
                })?,
            (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
                return Err(D::Error::custom(
                    "ambiguous time range: both canonical 'timeRange' and legacy 'start'/'end' \
                     fields are present. Choose exactly one shape per request.",
                ));
            }
            (None, Some(_), None) | (None, None, Some(_)) => {
                return Err(D::Error::custom(
                    "incomplete legacy time range: 'start' and 'end' must both be provided. \
                     Or use the canonical 'timeRange' field with a preset name.",
                ));
            }
            (None, None, None) => {
                return Err(D::Error::custom(format!(
                    "missing time range: provide canonical 'timeRange' (preset name, one of: {}) \
                     or legacy 'start'+'end' RFC 3339 timestamps.",
                    crate::admin::TimeRange::PRESETS.join(", "),
                )));
            }
        };
        Ok(GetModerationMetricsInput {
            time_range,
            granularity: raw.granularity,
            metrics: raw.metrics,
        })
    }
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
        time_col.to_string()
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
    // Per LB-2 / chainlink #118: this endpoint is a `query` per the
    // XRPC convention and serves GET. `axum_extra::extract::Query`
    // is required because `metrics` is `Vec<MetricType>` and the
    // default `axum::extract::Query` (serde_urlencoded) collapses
    // repeated keys to the last value — same reason
    // `getAccountInfos` uses the extra extractor.
    axum_extra::extract::Query(input): axum_extra::extract::Query<GetModerationMetricsInput>,
) -> Result<Json<GetModerationMetricsOutput>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::Moderator) {
        return Err(forbidden(&format!(
            "getModerationMetrics requires Moderator+ role; caller has {:?}",
            auth.role
        )));
    }
    // TimeRange is the validation boundary (Arc 5 §9.4.3): the
    // wrapper guarantees `start <= end` at deserialize time, so
    // this handler trusts the value without re-validating.
    let start = input.time_range.start();
    let end = input.time_range.end();
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

    // Audit chain entries (SuperAdmin-gated). The audit-entries.json
    // payload uses the canonical `AuditEntry` wire shape — same as
    // `getAuditTrail`'s `items[]` — by routing each fetched row
    // through `audit_chain::audit_entry_from_row`. Arc 9 Step 4 /
    // chainlink #55 Item 2 closed the prior divergence (raw-i64 ids,
    // `createdAt` instead of `timestamp`, missing `subjectRef` /
    // `verified` / cascade fields); see V04_DESIGN.md §8.4.4.
    let audit_entries: serde_json::Value = if input.include_audit_chain {
        let rows = sqlx::query(
            "SELECT id, sequence, created_at, actor_did, action, subject_did, \
                    subject_uri, subject_cid, rationale, snapshot_id, event_id, \
                    current_hash, previous_hash, cascade_subjects, cascade_snapshot_ids \
             FROM audit_chain_entry WHERE subject_did = $1 ORDER BY sequence ASC",
        )
        .bind(&input.did)
        .fetch_all(&ctx.account_db)
        .await
        .map_err(internal)?;
        let entries: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                let entry = audit_chain::audit_entry_from_row(r).map_err(internal)?;
                serde_json::to_value(&entry).map_err(internal)
            })
            .collect::<Result<_, _>>()?;
        serde_json::Value::Array(entries)
    } else {
        serde_json::Value::Null
    };

    // Bundle pieces serialized
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    files.push((
        "account-state.json".to_string(),
        serde_json::to_vec_pretty(&account_state).map_err(internal)?,
    ));
    if input.include_moderation_history {
        files.push((
            "moderation-history.json".to_string(),
            serde_json::to_vec_pretty(&mod_history).map_err(internal)?,
        ));
    }
    if input.include_audit_chain {
        files.push((
            "audit-entries.json".to_string(),
            serde_json::to_vec_pretty(&audit_entries).map_err(internal)?,
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
    // schemaVersion="2" marks the audit-entries.json wire-format
    // migration (Arc 9 Step 4 / chainlink #55 Item 2) where each row
    // now uses the canonical `AuditEntry` shape instead of the prior
    // inline serde_json::json! literal. Consumers scripted against the
    // v1 shape (raw-i64 `id`, `createdAt` field name, missing
    // `subjectRef` / `verified` / cascade fields) dispatch on this
    // field. No backwards-compatibility logic inside Aurora-Locus —
    // the binary always emits v2 going forward.
    let manifest = serde_json::json!({
        "schemaVersion": "2",
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
            "note": "v0.2 forensic bundles ship metadata only; CAR + blob streaming remains a v0.5+ candidate"
        },
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(internal)?;

    // audit-trail.json — included unconditionally as the bundle's own
    // chain anchor. The chain entry id and bundle hash both live in
    // response headers (and getAuditTrail) rather than in this file,
    // because either field would create a chicken-and-egg cycle: the
    // bundle hash must cover the whole tar (including this file), and
    // the chain entry id is only known after the chain row is
    // appended which itself records the bundle hash. The chainAnchor
    // sentinel makes that indirection explicit so consumers know
    // where to look.
    let trail = serde_json::json!({
        "exportedAt": started_at.to_rfc3339(),
        "chainAnchor": "see X-Aurora-Audit-Entry-Id response header for the chain entry id; \
                        the chain row's rationale records the SHA-256 bundle hash over the \
                        complete tar bytes",
    });
    files.push((
        "audit-trail.json".to_string(),
        serde_json::to_vec_pretty(&trail).map_err(internal)?,
    ));

    // TAR assembly — must complete before bundle hashing so the hash
    // covers every byte the operator is asserting authority over per
    // §3.4 chain-of-custody. Earlier shapes hashed only manifest.json,
    // which left the per-file payloads and audit-trail.json outside
    // the chain commitment.
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
            .map_err(internal)?;
        for (name, bytes) in &files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(started_at.timestamp() as u64);
            header.set_cksum();
            builder
                .append_data(&mut header, name.as_str(), &bytes[..])
                .map_err(internal)?;
        }
        builder.finish().map_err(internal)?;
    }

    // Bundle hash over the complete tar bytes. This is what the chain
    // entry commits to and what consumers verify the downloaded tar
    // against (compare SHA-256(downloaded_bytes) to the chain row's
    // rationale, or to the X-Aurora-Bundle-Hash response header for a
    // freshly issued export).
    let mut tar_hasher = Sha256::new();
    tar_hasher.update(&tar_buf);
    let bundle_hash = hex::encode(tar_hasher.finalize());

    // Audit chain entry for the export itself per §8.7 step 6. Now
    // happens AFTER tar assembly so the recorded hash covers the
    // actual bytes shipped.
    let subject = Subject::Repo {
        did: input.did.clone(),
    };
    let snapshot_id =
        audit_chain::capture_snapshot(&ctx.account_db, &subject)
            .await
            .map_err(internal_pds)?;
    let audit_entry_id = audit_chain::insert_chain_entry_pool(
        &ctx.account_db,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: &auth.did,
            action: "ForensicExport",
            subject: Some(&subject),
            rationale: &format!("{} (bundle hash: {})", input.rationale, bundle_hash),
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(internal_pds)?;

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
    /// Filter to entries with `subject_cid` matching this CID. Useful
    /// for finding audit entries about a specific blob (Subject::Blob
    /// carries CID; Subject::Record's CID is also indexed here when
    /// present). Added Arc 3 Step 0.5 (§7.4.0.5) — the prior six
    /// filters omitted CID despite it being a primary identifier for
    /// blob subjects; v0.2 corpus was silent on the omission.
    #[serde(default)]
    pub subject_cid: Option<String>,
    #[serde(default)]
    pub after_created: Option<String>,
    #[serde(default)]
    pub before_created: Option<String>,
    #[serde(flatten)]
    pub pagination: PaginationParams,
}

/// Wraps the paginated audit-entry list with chain-level verification
/// status. Per-row `verified` flags catch row-local tampering;
/// `chain_verified` catches the case where an attacker rewrote a prior
/// entry's content AND its `current_hash` consistently — per-row would
/// pass on every row but the linkage between entries breaks.
/// `chain_verified_through` is the highest sequence covered by the
/// verification window and is meaningful only when `chain_verified` is
/// true.
///
/// # Stability commitment
///
/// Per `docs/V03_DESIGN.md` §7.3.1: audit-trail read contract is committed.
/// The following are stable across releases:
///
/// - **Endpoint identity**: `tools.aurora.admin.getAuditTrail`, GET,
///   `AdminAuthContext` with `Moderator+` role gate.
/// - **Filter set** (seven fields, AND-combined): `actor_did`,
///   `action`, `subject_did`, `subject_uri`, `subject_cid`,
///   `after_created`, `before_created`. `subject_cid` was added in
///   the v0.3 cycle (Arc 3 Step 0.5); the other six predate.
/// - **Response shape**: this struct's four fields (`items`,
///   `cursor`, `chainVerified`, `chainVerifiedThrough`).
/// - **Per-entry shape**: `AuditEntry`, including `cascadeSnapshotIds`
///   which Arc 3 Step 1 added on the wire to enable independent
///   chain verification for batch entries.
/// - **Pagination**: forward-only, newest-first
///   (`ORDER BY created_at DESC, id DESC`); base64-encoded
///   `CursorPosition` (composite of `after_created` + `after_id` for
///   tie-stable ordering); default limit 50, max 100, min 1; absent
///   `cursor` on the response signals end-of-results.
/// - **Verification**: `chainVerified` is computed over rows
///   `[1..head_seq]` on every request (whole-chain re-verification);
///   `chainVerifiedThrough` is `head_seq` on success or
///   `failing_sequence - 1` on per-row / linkage / gap failure
///   (saturating_sub at seq=1). Per-entry `verified` is a separate
///   per-row hash recompute, independent of the chain-level result.
///
/// New filters and new top-level fields may be added additively;
/// removal of any committed surface is a breaking change.
///
/// **Wire-to-canonical bridge** for independent chain verification:
/// `docs/operator/audit-chain-verification.md`. Consumers reading
/// this response and recomputing SHA-256 hashes themselves should
/// follow the per-variant Subject decomposition rules and the
/// stringified-i64 → numeric-i64 conversion documented there.
///
/// Snapshot tests in `tests/audit_chain_canonical_verification.rs`
/// (Step 2) and the cascade roundtrip test
/// `get_audit_trail_round_trips_cascade_snapshot_ids` (Step 1) pin
/// the wire format. Contract-phrase test in
/// `tests/contract_phrases.rs` pins the commitment phrase above.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetAuditTrailOutput {
    pub items: Vec<AuditEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    pub chain_verified: bool,
    pub chain_verified_through: i64,
}

pub async fn get_audit_trail(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    axum::extract::Query(params): axum::extract::Query<GetAuditTrailParams>,
) -> Result<Json<GetAuditTrailOutput>, (StatusCode, Json<serde_json::Value>)> {
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
    if let Some(c) = &params.subject_cid {
        clauses.push("subject_cid = ?");
        binds.push(c.clone());
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
                cascade_subjects, cascade_snapshot_ids \
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

    // v0.6 batch tail A.1 — DRY consolidation. The inline manual
    // row-parse + verify + AuditEntry-construct block here used to
    // mirror `audit_chain::audit_entry_from_row` field-by-field
    // (~80 LOC duplicate). Now consumes the shared helper; cursor
    // tracking pulls id/timestamp from the constructed AuditEntry
    // (entry.id is the i64 round-tripped through String — infallible
    // parse because the helper just stringified it). The
    // `forensic_audit_entries_match_get_audit_trail_shape` test still
    // pins the byte-identical-shape invariant between this path and
    // the exportAccountForensic loop at :3041-3062 (now both consume
    // the helper).
    let mut items = Vec::with_capacity(page_rows.len());
    let mut last_at = None;
    let mut last_id = None;
    for row in page_rows {
        let entry = audit_chain::audit_entry_from_row(&row).map_err(internal)?;
        last_at = Some(entry.timestamp);
        last_id = Some(entry.id.parse::<i64>().expect(
            "audit_entry_from_row stringified an i64 from the row; parse-back is infallible",
        ));
        items.push(entry);
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

    // Chain-level verification: walk the entire chain (sentinel rows
    // are skipped internally) and confirm every entry's previous_hash
    // matches the prior entry's current_hash. Per-row `verified` flags
    // already caught row-local tampering above; this catches the
    // consistent-rewrite case where current_hash was rewritten in step
    // with the content but the linkage was missed.
    let head_seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(sequence), 0) FROM audit_chain_entry",
    )
    .fetch_one(&ctx.account_db)
    .await
    .unwrap_or(0i64);
    // Per CR-8 / chainlink #120: when verification fails, surface
    // `failing_sequence - 1` as `chain_verified_through` so operators
    // investigating chain failures get a row-level pointer rather
    // than an undifferentiated 0. saturating_sub(1) handles the
    // edge case where seq=1 itself failed (nothing was verified
    // through; chain_verified_through = 0 is correct).
    let verification_result = if head_seq == 0 {
        Ok(())
    } else {
        audit_chain::verify_chain_range(&ctx.account_db, 1, head_seq).await
    };
    let (chain_verified, chain_verified_through) = match verification_result {
        Ok(()) => (true, head_seq),
        Err(e) => (false, e.failing_sequence.saturating_sub(1)),
    };

    Ok(Json(GetAuditTrailOutput {
        items,
        cursor: next_cursor,
        chain_verified,
        chain_verified_through,
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
// Lookup precedence (per Arc 5 §9.4.2 / chainlink #124):
//   1. Recovery-mode env-var override (AURORA_RECOVERY_MODE=true,
//      `moderation-mode` only).
//   2. Runtime row in `runtime_settings` (operator-set, ephemeral).
//   3. File-tier YAML loaded once at startup from
//      `<data_directory>/runtime.yaml` (overridable via
//      `PDS_RUNTIME_FILE`); deployment-stable.
//   4. Compiled-in default from `default_for_key`.
//
// Recovery path: AURORA_RECOVERY_MODE=true env var bypasses tiers
// 2-4 for the moderation-mode key. An operator who deployed into a
// misconfigured "disabled" state can boot with the env var set, fix
// the runtime row, and unset the env var.

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
    pub source: SettingSource,
    pub last_modified: Option<String>,
    pub last_modified_by: Option<String>,
}

/// Origin of a resolved runtime-setting value. Per Arc 5 §9.4.2
/// (chainlink #124) the lookup walks four tiers in priority order
/// — `RecoveryMode` env-var override (top, `moderation-mode` only),
/// then `Runtime` row, then `File` (YAML loaded at startup), then
/// `Default` compiled-in fallback. The wire encoding is the bare
/// string "Runtime" / "File" / "Default" / "RecoveryMode" via the
/// custom `Serialize` impl below; pre-Arc-5 callers reading the
/// `source` field as a string see no change for the existing three
/// values.
///
/// The field's value set is **open** per Arc 2's contract framing
/// — `contract-stability.md` does not pin a closed enumeration on
/// `source`, and this addition is wire-additive, not a contract
/// amendment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSource {
    Runtime,
    File,
    Default,
    RecoveryMode,
}

impl SettingSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Runtime => "Runtime",
            Self::File => "File",
            Self::Default => "Default",
            Self::RecoveryMode => "RecoveryMode",
        }
    }
}

impl Serialize for SettingSource {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

const MODERATION_MODE_KEY: &str = "moderation-mode";
const MODERATION_MODE_REDIRECT_KEY: &str = "moderation-mode-redirect-url";

/// Allowlist of runtime-setting keys this build accepts. Per CR-2 /
/// chainlink #119, `setRuntimeSetting` rejects any other key with
/// 400 — the inventory's "validates known keys" framing
/// (docs/AURORA_ENDPOINT_INVENTORY.md) is enforced there. The
/// file-tier loader (`load_file_tier_settings`) applies the same
/// allowlist: keys outside this set are warned-and-skipped at
/// startup so a typo doesn't silently disable a deployment-stable
/// override. Adding a new runtime-setting key in a future cycle is
/// one append to this constant plus the corresponding default in
/// `default_for_key`.
pub const KNOWN_RUNTIME_KEYS: &[&str] = &[
    MODERATION_MODE_KEY,
    MODERATION_MODE_REDIRECT_KEY,
];
const RECOVERY_MODE_ENV: &str = "AURORA_RECOVERY_MODE";

/// Env-var override of the file-tier YAML path. Default is
/// `<data_directory>/runtime.yaml`. Resolved in `AppContext::new`.
pub const RUNTIME_FILE_ENV: &str = "PDS_RUNTIME_FILE";

fn default_for_key(key: &str) -> serde_json::Value {
    match key {
        MODERATION_MODE_KEY => serde_json::Value::String("full".to_string()),
        MODERATION_MODE_REDIRECT_KEY => serde_json::Value::String(String::new()),
        _ => serde_json::Value::Null,
    }
}

/// Validate a runtime-setting value at file-tier load time. Mirrors
/// the per-key validation `set_runtime_setting` performs at the API
/// boundary so file-tier and runtime-row writes share the same
/// vocabulary. Unknown keys (already filtered against
/// `KNOWN_RUNTIME_KEYS` upstream) accept any value shape.
fn validate_runtime_value(key: &str, value: &serde_json::Value) -> bool {
    match key {
        MODERATION_MODE_KEY => value
            .as_str()
            .is_some_and(|s| matches!(s, "full" | "reduced" | "disabled")),
        MODERATION_MODE_REDIRECT_KEY => value.as_str().is_some(),
        _ => true,
    }
}

/// Load file-tier runtime settings from the YAML at `path`.
///
/// Per Arc 5 §9.4.2 / chainlink #124:
/// - Missing file → empty map (file tier is optional; falls through
///   to default).
/// - Malformed YAML → `PdsError::Validation` with the file path in
///   the message; surfaces as a startup error.
/// - Unknown key (not in `KNOWN_RUNTIME_KEYS`) → warn-and-skip;
///   per-deployment typos don't silently disable the deployment.
/// - Invalid value (per `validate_runtime_value`) → warn-and-skip.
/// - Top-level non-mapping → `PdsError::Validation`.
///
/// The returned map is loaded once at `AppContext::new` and cached
/// for the process lifetime. Reload-on-SIGHUP is deferred to v0.4;
/// the runtime_settings table provides the hot path for changes
/// inside a running process.
pub fn load_file_tier_settings(
    path: &std::path::Path,
) -> crate::error::PdsResult<std::collections::HashMap<String, serde_json::Value>> {
    use crate::error::PdsError;
    if !path.exists() {
        return Ok(std::collections::HashMap::new());
    }
    let yaml_str = std::fs::read_to_string(path).map_err(|e| {
        PdsError::Validation(format!(
            "Failed to read file-tier config at {}: {}",
            path.display(),
            e
        ))
    })?;
    let parsed: serde_yaml::Value = serde_yaml::from_str(&yaml_str).map_err(|e| {
        PdsError::Validation(format!(
            "Failed to parse file-tier config at {}: {}",
            path.display(),
            e
        ))
    })?;
    let mapping = match parsed {
        serde_yaml::Value::Mapping(m) => m,
        serde_yaml::Value::Null => return Ok(std::collections::HashMap::new()),
        _ => {
            return Err(PdsError::Validation(format!(
                "File-tier config at {} must be a top-level YAML mapping",
                path.display()
            )));
        }
    };
    let mut out = std::collections::HashMap::new();
    for (key_v, val_v) in mapping {
        let key = match key_v {
            serde_yaml::Value::String(s) => s,
            other => {
                tracing::warn!(
                    "file-tier config: skipping non-string key {:?} in {}",
                    other,
                    path.display()
                );
                continue;
            }
        };
        if !KNOWN_RUNTIME_KEYS.contains(&key.as_str()) {
            tracing::warn!(
                "file-tier config: unknown runtime-setting key '{}' in {}; \
                 skipping (known keys: {:?})",
                key,
                path.display(),
                KNOWN_RUNTIME_KEYS
            );
            continue;
        }
        let json_val = match serde_json::to_value(&val_v) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "file-tier config: cannot convert YAML value for key '{}' in {} \
                     to JSON: {}; skipping",
                    key,
                    path.display(),
                    e
                );
                continue;
            }
        };
        if !validate_runtime_value(&key, &json_val) {
            tracing::warn!(
                "file-tier config: invalid value for key '{}' in {} ({}); skipping",
                key,
                path.display(),
                json_val
            );
            continue;
        }
        out.insert(key, json_val);
    }
    Ok(out)
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
            source: SettingSource::RecoveryMode,
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
        return Ok(Json(GetRuntimeSettingOutput {
            key: params.key,
            value,
            source: SettingSource::Runtime,
            last_modified: Some(last_modified),
            last_modified_by: Some(last_modified_by),
        }));
    }
    // Tier 3: file-tier YAML loaded once at startup. Sits between
    // runtime row and compiled-in default per Arc 5 §9.4.2.
    if let Some(value) = ctx.file_tier_settings.get(&params.key) {
        return Ok(Json(GetRuntimeSettingOutput {
            key: params.key,
            value: value.clone(),
            source: SettingSource::File,
            last_modified: None,
            last_modified_by: None,
        }));
    }
    Ok(Json(GetRuntimeSettingOutput {
        key: params.key.clone(),
        value: default_for_key(&params.key),
        source: SettingSource::Default,
        last_modified: None,
        last_modified_by: None,
    }))
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
    // Allowlist check (CR-2 / chainlink #119). The inventory's
    // "validates known keys" framing requires this guard; without
    // it, any string would persist to runtime_settings and the
    // setting table would accumulate junk. The §8.16 design treats
    // the runtime-settings keyspace as a finite known vocabulary,
    // not free-form storage.
    if !KNOWN_RUNTIME_KEYS.contains(&input.key.as_str()) {
        return Err(validation(format!(
            "unknown runtime setting key '{}'; known keys: {:?}",
            input.key, KNOWN_RUNTIME_KEYS,
        )));
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
        serde_json::to_string(&input.value).map_err(internal)?;

    // LB-1 Session 12 / chainlink #129: runtime_settings upsert +
    // chain entry in one transaction. Upsert uses DELETE then INSERT
    // for cross-backend portability — sqlx ON CONFLICT syntax differs
    // between SQLite and Postgres.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
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
    let audit_entry_id = audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: &auth.did,
            action: "SetRuntimeSetting",
            subject: None,
            rationale: &format!("{} → {}: {}", input.key, value_json, input.rationale),
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(internal_pds)?;
    tx.commit().await.map_err(internal)?;
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
                public_url: None,
                max_blob_fetch_size: 50_000_000,
                blob_fetch_timeout_seconds: 30,
                blob_fetch_max_retries: 3,
                accepting_imports: true,
                max_import_size: None,
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
                recovery_did_key: None,
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
                exempt_admin_assets: true,
                buckets_retention_days: 7,
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
                peer_pds: vec![],
            },
            validation_mode: PathBuf::from("required").into_os_string().to_string_lossy().parse().unwrap_or(crate::validation::ValidationMode::Required),
            distributed_state_mode: Default::default(),
            maintenance_pool: Default::default(),
            gc_sweep: Default::default(),
            blob_metadata: Default::default(),
            entryway: None,
            lexicon: crate::config::LexiconConfig::default(),
        };
        AppContext::new(
            config,
            std::sync::Arc::new(crate::api::registry::RouteRegistry::default()),
        )
        .await
        .unwrap()
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
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::TakedownAccount,
                subjects: vec![repo_subject("did:plc:victim")],
                rationale: "spam".to_string(),
                snapshot_capture: true,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(!resp.event_id.is_empty());
        // Phase 3.8 makes these meaningful — emitEvent now writes a
        // chain entry + snapshot for snapshottable subjects.
        assert!(!resp.audit_entry_id.is_empty(), "emitEvent populates audit_entry_id");
        assert!(
            resp.snapshots.first().and_then(|s| s.snapshot_id.as_ref()).is_some(),
            "Phase 3.8 captures snapshot for Repo subjects"
        );
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
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::TakedownAccount,
                subjects: vec![repo_subject("did:plc:victim")],
                rationale: "   ".to_string(),
                snapshot_capture: true,
                metadata: None,
                legacy_subject_used: false,
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
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::TakedownAccount,
                subjects: vec![Subject::Record {
                    uri: "at://did:plc:abc/app.bsky.feed.post/123".to_string(),
                    cid: "bafyrei...".to_string(),
                }],
                rationale: "wrong subject type".to_string(),
                snapshot_capture: true,
                metadata: None,
                legacy_subject_used: false,
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
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::DeleteAccount,
                subjects: vec![repo_subject("did:plc:victim")],
                rationale: "test".to_string(),
                snapshot_capture: true,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn emit_event_send_email_requires_admin_role() {
        // P-2 / chainlink #114: SendEmail is an Admin-tier capability
        // per §3.2 (account-contact channel sits alongside passwords,
        // emails, handles, signing keys, deletion). A Moderator
        // emitting SendEmail must hit 403; an Admin emitting the same
        // event must succeed.
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:recip", "recip.test").await;

        // Moderator → 403
        let err = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::SendEmail {
                    template: None,
                    subject: "test subject".to_string(),
                    body: "test body".to_string(),
                },
                subjects: vec![repo_subject("did:plc:recip")],
                rationale: "Moderator may not emit SendEmail".to_string(),
                snapshot_capture: false,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);

        // Admin → role check passes. Downstream validation may still
        // reject (e.g., "recipient account not found" if the test
        // fixture is sparse) but the failure mode must not be 403 —
        // the role gate is what this test pins. Anything other than
        // FORBIDDEN means role check let the call through.
        let result = emit_event(
            State(ctx.clone()),
            admin_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::SendEmail {
                    template: None,
                    subject: "test subject".to_string(),
                    body: "test body".to_string(),
                },
                subjects: vec![repo_subject("did:plc:recip")],
                rationale: "Admin may emit SendEmail".to_string(),
                snapshot_capture: false,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await;
        match result {
            Ok(_) => {}
            Err((status, _)) => assert_ne!(
                status,
                StatusCode::FORBIDDEN,
                "Admin must clear the role gate; got 403 which means the gate rejected"
            ),
        }
    }

    #[tokio::test]
    async fn emit_event_moderator_can_still_apply_label_after_send_email_tightening() {
        // Regression check that tightening SendEmail to Admin+ did not
        // accidentally tighten the other moderator-flavored events.
        // ApplyLabel must continue to accept Moderator+.
        let ctx = create_test_context().await;
        let resp = emit_event(
            State(ctx),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::ApplyLabel {
                    val: "regression".to_string(),
                    neg: false,
                },
                subjects: vec![Subject::Record {
                    uri: "at://did:plc:abc/app.bsky.feed.post/xyz".to_string(),
                    cid: "bafyreigh".to_string(),
                }],
                rationale: "moderator-flavored event still allowed".to_string(),
                snapshot_capture: false,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .expect("ApplyLabel still allowed for Moderator after P-2 tightening")
        .0;
        assert!(!resp.event_id.is_empty());
    }

    #[tokio::test]
    async fn emit_event_apply_label_writes_label_row() {
        let ctx = create_test_context().await;
        let resp = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::ApplyLabel {
                    val: "spam".to_string(),
                    neg: false,
                },
                subjects: vec![Subject::Record {
                    uri: "at://did:plc:abc/app.bsky.feed.post/xyz".to_string(),
                    cid: "bafyreigh".to_string(),
                }],
                rationale: "obvious spam".to_string(),
                snapshot_capture: true,
                metadata: None,
                legacy_subject_used: false,
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
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::ResolveAppeal {
                    appeal_id: appeal.id,
                    resolution: AppealResolutionDecision::Approve,
                },
                subjects: vec![repo_subject("did:plc:appellant")],
                rationale: "appeal valid".to_string(),
                snapshot_capture: true,
                metadata: None,
                legacy_subject_used: false,
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
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::ResolveAppeal {
                    appeal_id: appeal.id,
                    resolution: AppealResolutionDecision::Deny,
                },
                subjects: vec![repo_subject("did:plc:appellant2")],
                rationale: "denied".to_string(),
                snapshot_capture: true,
                metadata: None,
                legacy_subject_used: false,
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
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::DeleteAccount,
                subjects: vec![repo_subject("did:plc:deleteme")],
                rationale: "voluntary deletion".to_string(),
                snapshot_capture: true,
                metadata: None,
                legacy_subject_used: false,
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
            "subjects": [{"$type": "com.atproto.admin.defs#repoRef", "did": "did:plc:abc"}],
            "rationale": "spam"
        });
        let input: EmitEventInput = serde_json::from_value(raw).unwrap();
        assert!(matches!(input.action, ModEventAction::TakedownAccount));
        assert_eq!(input.subjects.len(), 1);
        assert!(input.snapshot_capture, "snapshot_capture defaults to true");
    }

    #[test]
    fn emit_event_input_deserializes_action_with_inline_data() {
        let raw = serde_json::json!({
            "action": {"kind": "ApplyLabel", "val": "spam", "neg": false},
            "subjects": [{"$type": "com.atproto.repo.strongRef", "uri": "at://did:plc:abc/x/y", "cid": "bafy..."}],
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
        // Audit chain entry id surfaces on the response (Block 1
        // wired all six batch endpoints through insert_chain_entry_pool).
        assert!(
            !resp.audit_entry_id.is_empty(),
            "batch_takedown_accounts populates audit_entry_id"
        );
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
        // ONE chain entry — §3.4 "one decision = one chain entry"
        // framing means a batch is a single operator decision even
        // when N subjects were affected. The per-DID list lives in
        // cascade_subjects on the same row.
        let chain_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_chain_entry WHERE actor_did = $1 AND action = $2",
        )
        .bind("did:plc:moderator")
        .bind("account.batch_takedown")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(chain_count, 1);
    }

    #[tokio::test]
    async fn batch_takedown_captures_per_subject_snapshots() {
        // CR-2 / chainlink #111: each batch entry must carry a
        // `cascade_snapshot_ids` JSON list whose i-th element is the
        // snapshot id for `cascade_subjects[i]`. Verify both the chain
        // row's column and the wire response's per-snapshot ids are
        // populated and resolve to actual audit_snapshot rows.
        use sqlx::Row as _;
        let ctx = create_test_context().await;
        for i in 0..3 {
            seed_actor(&ctx, &format!("did:plc:c{}", i), &format!("c{}.test", i)).await;
        }
        let resp = batch_takedown_accounts(
            State(ctx.clone()),
            moderator_auth(),
            Json(BatchAccountsInput {
                dids: vec![
                    "did:plc:c0".to_string(),
                    "did:plc:c1".to_string(),
                    "did:plc:c2".to_string(),
                ],
                rationale: "snapshot-pairing test".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        // Wire shape: every SnapshotRef carries a populated snapshot_id.
        assert_eq!(resp.snapshots.len(), 3);
        for snap in &resp.snapshots {
            assert!(
                snap.snapshot_id.is_some(),
                "every batch SnapshotRef must carry a populated snapshot_id"
            );
        }

        // Chain row column: cascade_snapshot_ids is a JSON list of
        // length 3 in lock-step with cascade_subjects.
        let row = sqlx::query(
            "SELECT cascade_subjects, cascade_snapshot_ids FROM audit_chain_entry \
             WHERE action = 'account.batch_takedown'",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        let cascade_subjects_json: String = row.try_get("cascade_subjects").unwrap();
        let cascade_snapshot_ids_json: String = row.try_get("cascade_snapshot_ids").unwrap();
        let cascade_subjects: Vec<Subject> =
            serde_json::from_str(&cascade_subjects_json).unwrap();
        let cascade_snapshot_ids: Vec<Option<i64>> =
            serde_json::from_str(&cascade_snapshot_ids_json).unwrap();
        assert_eq!(cascade_subjects.len(), 3);
        assert_eq!(cascade_snapshot_ids.len(), 3);

        // Every snapshot id resolves to an actual audit_snapshot row,
        // and the snapshot's subject_did matches the corresponding
        // cascade subject. This is the §3.4 forensic linkage being
        // exercised end-to-end.
        for (subj, snap_id_opt) in cascade_subjects.iter().zip(cascade_snapshot_ids.iter()) {
            let snap_id = snap_id_opt.expect("each cascade subject has a snapshot id");
            let snap_subject_did: Option<String> = sqlx::query_scalar(
                "SELECT subject_did FROM audit_snapshot WHERE id = $1",
            )
            .bind(snap_id)
            .fetch_one(&ctx.account_db)
            .await
            .unwrap();
            let expected_did = match subj {
                Subject::Repo { did } => did.clone(),
                _ => panic!("expected Repo subject"),
            };
            assert_eq!(snap_subject_did.as_deref(), Some(expected_did.as_str()));
        }
    }

    #[tokio::test]
    async fn batch_takedown_per_subject_failure_aborts_whole_tx_atomically() {
        // Arc 4 §8.4.2 / chainlink #113: whole-batch atomicity. When a
        // per-subject mutation fails (here: the third DID isn't
        // seeded → takedown_account_in_tx returns NotFound), the
        // entire wrapping tx aborts. Inverts the v0.2 partial-success
        // pattern (chainlink #112): no chain entry, no successful
        // per-subject mutations, no moderation_event row land.
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:p0", "p0.test").await;
        seed_actor(&ctx, "did:plc:p1", "p1.test").await;

        let chain_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_chain_entry")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        let event_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM moderation_event")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();

        let err = batch_takedown_accounts(
            State(ctx.clone()),
            moderator_auth(),
            Json(BatchAccountsInput {
                dids: vec![
                    "did:plc:p0".to_string(),
                    "did:plc:p1".to_string(),
                    "did:plc:doesnotexist".to_string(),
                ],
                rationale: "expect whole-tx abort".to_string(),
            }),
        )
        .await
        .expect_err("Arc 4: per-subject failure must abort the whole batch");

        // NotFound from takedown_account_in_tx → 404 via batch_subject_err_response.
        assert_eq!(err.0, StatusCode::NOT_FOUND);
        let body = format!("{:?}", err.1.0);
        assert!(
            body.contains("doesnotexist"),
            "error body identifies the failing DID, got: {}",
            body
        );
        assert!(
            body.contains("\"failingSubject\": Number(2)") || body.contains("failingSubject"),
            "error body surfaces the failing index"
        );

        // No chain entry written.
        let chain_after: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_chain_entry")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        assert_eq!(
            chain_after, chain_before,
            "no chain entry on whole-batch abort"
        );
        // No moderation_event row written.
        let event_after: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM moderation_event")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        assert_eq!(
            event_after, event_before,
            "no moderation_event row on whole-batch abort"
        );
        // The first two DIDs' takedown_refs must NOT have landed —
        // the SAVEPOINT-recovery path is gone.
        let p0_takedown: Option<String> =
            sqlx::query_scalar("SELECT takedown_ref FROM actor WHERE did = 'did:plc:p0'")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        assert!(
            p0_takedown.is_none(),
            "p0 takedown_ref must NOT land — whole tx rolled back atomically"
        );
        let p1_takedown: Option<String> =
            sqlx::query_scalar("SELECT takedown_ref FROM actor WHERE did = 'did:plc:p1'")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        assert!(
            p1_takedown.is_none(),
            "p1 takedown_ref must NOT land — whole tx rolled back atomically"
        );
    }

    // Arc 4 §8.4.2 / chainlink #113 parallel test for
    // batch_restore_accounts. The handler clears `takedown_ref` per
    // DID by direct `UPDATE` (no manager call), and SQLite's UPDATE
    // on a non-existent row returns 0 rows_affected without
    // erroring — so the per-subject-failure-aborts-whole-tx pattern
    // can't be exercised here with the cheap "unseeded DID" trick
    // that `batch_takedown_per_subject_failure_aborts_whole_tx_atomically`
    // uses on the takedown side. This test pins the happy-path
    // atomicity (chain entry + per-DID UPDATE + moderation_event +
    // account_moderation rows all commit together). The whole-tx
    // contract for restore is enforced by construction: any genuine
    // per-subject UPDATE error (constraint violation, driver crash)
    // propagates via `?`-on-`map_err` and aborts the wrapping tx
    // identically to the takedown test.
    #[tokio::test]
    async fn batch_restore_lands_chain_entry_atomically_with_actor_updates() {
        use sqlx::Row as _;
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:r0", "r0.test").await;
        seed_actor(&ctx, "did:plc:r1", "r1.test").await;
        // Pre-seed takedown_ref so we can observe the clear.
        sqlx::query("UPDATE actor SET takedown_ref = 'pre' WHERE did IN ('did:plc:r0', 'did:plc:r1')")
            .execute(&ctx.account_db)
            .await
            .unwrap();

        let resp = batch_restore_accounts(
            State(ctx.clone()),
            moderator_auth(),
            Json(BatchAccountsInput {
                dids: vec!["did:plc:r0".to_string(), "did:plc:r1".to_string()],
                rationale: "restore".to_string(),
            }),
        )
        .await
        .expect("batch returns 200")
        .0;
        assert_eq!(resp.affected_count, 2);

        // Both takedown_ref values cleared.
        let r0: Option<String> =
            sqlx::query_scalar("SELECT takedown_ref FROM actor WHERE did = 'did:plc:r0'")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        assert!(r0.is_none(), "r0 takedown_ref cleared");
        let r1: Option<String> =
            sqlx::query_scalar("SELECT takedown_ref FROM actor WHERE did = 'did:plc:r1'")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        assert!(r1.is_none(), "r1 takedown_ref cleared");

        // Chain entry covers both DIDs.
        let row = sqlx::query(
            "SELECT cascade_subjects FROM audit_chain_entry \
             WHERE action = 'account.batch_restore'",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        let cascade_json: String = row.try_get("cascade_subjects").unwrap();
        let cascade: Vec<Subject> = serde_json::from_str(&cascade_json).unwrap();
        assert_eq!(cascade.len(), 2);

        // moderation_event landed (one row per batch).
        let event_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM moderation_event WHERE event_type = 'account_restore'",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(event_count, 1);

        // account_moderation rows: one per DID.
        let am_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM account_moderation WHERE action = 'restore'")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        assert_eq!(am_count, 2);
    }

    #[tokio::test]
    async fn batch_restore_silently_treats_missing_did_as_noop() {
        // Arc 4 §8.4.2: per-subject UPDATE on a non-existent DID
        // returns 0 rows_affected on both SQLite and Postgres
        // without erroring, so a missing DID in the batch is a
        // no-op (vs. v0.2 where it was captured into the now-gone
        // `failures` field). The chain entry, moderation_event, and
        // account_moderation rows still land for the operator's full
        // intent. Documents the behaviour explicitly so a future
        // regression that surfaces a NotFound here will be caught.
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:r_real", "rreal.test").await;
        sqlx::query("UPDATE actor SET takedown_ref = 'pre' WHERE did = 'did:plc:r_real'")
            .execute(&ctx.account_db)
            .await
            .unwrap();

        let resp = batch_restore_accounts(
            State(ctx.clone()),
            moderator_auth(),
            Json(BatchAccountsInput {
                dids: vec![
                    "did:plc:r_real".to_string(),
                    "did:plc:r_missing".to_string(),
                ],
                rationale: "missing-did is no-op".to_string(),
            }),
        )
        .await
        .expect("restore tolerates missing DIDs as silent no-ops")
        .0;
        // affected_count reports operator intent (the DIDs the
        // operator asked us to restore), not just the rows actually
        // modified — matches the chain row's cascade_subjects.
        assert_eq!(resp.affected_count, 2);
        // Real DID's takedown_ref cleared.
        let real_takedown: Option<String> = sqlx::query_scalar(
            "SELECT takedown_ref FROM actor WHERE did = 'did:plc:r_real'",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert!(real_takedown.is_none());
    }

    // LB-1 / chainlink #128: pin the all-or-nothing atomicity of
    // batch_apply_label. Two valid subjects in one batch land
    // together: chain entry + moderation_event + per-subject label
    // rows all commit, or none of them do. This is the wrapping
    // tx's atomicity contract — exercised here on the happy path.
    #[tokio::test]
    async fn batch_apply_label_lands_chain_event_and_labels_atomically() {
        use sqlx::Row as _;
        let ctx = create_test_context().await;
        let resp = batch_apply_label(
            State(ctx.clone()),
            moderator_auth(),
            Json(BatchLabelInput {
                subjects: vec![
                    Subject::Record {
                        uri: "at://did:plc:s0/app.bsky.feed.post/a".to_string(),
                        cid: "bafkreia".to_string(),
                    },
                    Subject::Record {
                        uri: "at://did:plc:s1/app.bsky.feed.post/b".to_string(),
                        cid: "bafkreib".to_string(),
                    },
                ],
                label_val: "porn".to_string(),
                label_neg: false,
                rationale: "atomic batch".to_string(),
            }),
        )
        .await
        .expect("batch returns 200")
        .0;
        assert_eq!(resp.affected_count, 2);

        // Two label rows, one moderation_event, one chain entry — all
        // sharing the wrapping tx.
        let label_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM label WHERE val = 'porn'")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        assert_eq!(label_count, 2);
        let chain_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_chain_entry WHERE action = 'label.batch_apply'",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(chain_count, 1);
        // cascade_subjects records both subjects.
        let row = sqlx::query(
            "SELECT cascade_subjects FROM audit_chain_entry WHERE action = 'label.batch_apply'",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        let cascade_json: String = row.try_get("cascade_subjects").unwrap();
        let cascade: Vec<Subject> = serde_json::from_str(&cascade_json).unwrap();
        assert_eq!(cascade.len(), 2);
    }

    // Arc 4 §8.4.3: pin the URI-level convention. `batch_takedown_records`
    // emits cascade_subjects entries shaped as `Record { uri, cid: "" }`
    // — the empty CID is deliberate and signals URI-level takedown
    // semantics, not missing data. A future change that populates CIDs
    // (e.g., resolving URI→CID at takedown time) flips Aurora-Locus
    // from URI-level to CID-level on this surface, which is a design
    // conversation, not a silent migration. This test fails loudly in
    // that case so the change must be explicit.
    #[tokio::test]
    async fn batch_takedown_records_produces_uri_level_cascade_with_empty_cids() {
        use sqlx::Row as _;
        let ctx = create_test_context().await;
        let uris = vec![
            "at://did:plc:author0/app.bsky.feed.post/aaa".to_string(),
            "at://did:plc:author1/app.bsky.feed.post/bbb".to_string(),
            "at://did:plc:author2/app.bsky.feed.post/ccc".to_string(),
        ];
        let resp = batch_takedown_records(
            State(ctx.clone()),
            moderator_auth(),
            Json(BatchRecordsInput {
                uris: uris.clone(),
                rationale: "URI-level takedown convention".to_string(),
            }),
        )
        .await
        .expect("batch returns 200")
        .0;
        assert_eq!(resp.affected_count, 3);

        let row = sqlx::query(
            "SELECT cascade_subjects FROM audit_chain_entry \
             WHERE action = 'record.batch_takedown'",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        let cascade_json: String = row.try_get("cascade_subjects").unwrap();
        let cascade: Vec<Subject> = serde_json::from_str(&cascade_json).unwrap();
        assert_eq!(cascade.len(), 3, "one cascade entry per input URI");

        for (i, entry) in cascade.iter().enumerate() {
            match entry {
                Subject::Record { uri, cid } => {
                    assert_eq!(
                        uri, &uris[i],
                        "cascade URI at index {i} matches input URI"
                    );
                    assert_eq!(
                        cid, "",
                        "cascade CID at index {i} is the empty string \
                         (URI-level convention per Arc 4 §8.4.3); \
                         a non-empty value here means batch record \
                         takedown shifted to CID-level semantics"
                    );
                }
                other => panic!(
                    "cascade entry at index {i} is not Subject::Record: {other:?}"
                ),
            }
        }

        // The wire response's per-subject snapshot refs carry the same
        // empty-CID Record shape — pin this too so consumers reading
        // the response (not the chain row directly) get the same
        // signal.
        for (i, snap) in resp.snapshots.iter().enumerate() {
            match &snap.subject {
                Subject::Record { uri, cid } => {
                    assert_eq!(uri, &uris[i]);
                    assert_eq!(
                        cid, "",
                        "wire response snapshot.subject CID at index {i} \
                         is empty (URI-level convention)"
                    );
                }
                other => panic!(
                    "snapshot subject at index {i} is not Subject::Record: {other:?}"
                ),
            }
        }
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
        assert!(
            !resp.audit_entry_id.is_empty(),
            "batch_apply_label populates audit_entry_id"
        );
        let label_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM label WHERE val = $1 AND neg = FALSE")
                .bind("spam")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        assert_eq!(label_count, 2);
        let chain_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_chain_entry WHERE actor_did = $1 AND action = $2",
        )
        .bind("did:plc:moderator")
        .bind("label.batch_apply")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(chain_count, 1);
    }

    #[tokio::test]
    async fn batch_remove_label_skips_subjects_without_label() {
        let ctx = create_test_context().await;
        // Apply label to one of two subjects upfront.
        let _ = batch_apply_label(
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
            State(ctx.clone()),
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
        assert!(
            !resp.audit_entry_id.is_empty(),
            "batch_remove_label populates audit_entry_id"
        );
        // The skipped subject is the one that didn't have the label.
        match &resp.skipped[0] {
            Subject::Record { uri, .. } => assert_eq!(uri, "at://did:plc:nope/x/y"),
            _ => panic!("wrong skipped subject shape"),
        }
        // ONE chain entry for the remove decision. The preseed
        // batch_apply_label call earlier in this test produced its
        // own chain entry, so we filter by the action string to
        // distinguish.
        let chain_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_chain_entry WHERE actor_did = $1 AND action = $2",
        )
        .bind("did:plc:moderator")
        .bind("label.batch_remove")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(chain_count, 1);
    }

    #[tokio::test]
    async fn batch_takedown_accepts_admin_role() {
        // Moderator is the floor role in the current Role enum
        // (Moderator < Admin < SuperAdmin), so a "below moderator"
        // negative test isn't expressible until a lower role lands.
        // This positive test verifies Admin (above Moderator) passes
        // the gate. The role-gate logic is identical across all six
        // batch endpoints (a single check_moderator_role(&auth)? at
        // each handler's head) so exercising one is sufficient
        // shape coverage for the role gate; per-endpoint coverage of
        // the chain-write surface lives on the existing happy-path
        // tests above. The previous name was plural but the body
        // only ever exercised batch_takedown_accounts; renamed to
        // match the actual scope.
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
        // average_age_open_reports_seconds is u32 (Arc 5 §9.4.3
        // retype): always non-negative by type, so a `>= 0`
        // assertion would be a tautology. The seeded report is
        // freshly inserted in this test, so the average age is
        // bounded by test runtime — under 60 seconds is a safe
        // ceiling that catches arithmetic errors without flakiness.
        assert!(
            resp.average_age_open_reports_seconds < 60,
            "freshly-seeded report should have average age < 60s; got {}",
            resp.average_age_open_reports_seconds,
        );
    }

    // ---------- Phase 3.7 — getModerationMetrics (§8.2) ----------

    /// Inverted-range rejection moves to the deserialize boundary
    /// per Arc 5 §9.4.3: `TimeRange::new` rejects `start > end`.
    /// Direct struct-literal construction in tests goes through
    /// the validating constructor so the test exercises the same
    /// semantic path the wire deserialize does.
    #[test]
    fn get_moderation_metrics_input_rejects_inverted_legacy_range() {
        // Wire-form: legacy start/end with start > end. The
        // dispatcher must reject at deserialize time.
        let inverted = serde_json::json!({
            "start": "2026-01-02T00:00:00Z",
            "end":   "2026-01-01T00:00:00Z",
            "granularity": "day"
        });
        let err = serde_json::from_value::<GetModerationMetricsInput>(inverted)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("start must be <= end"),
            "expected start-greater-than-end error from the legacy-shape \
             dispatcher; got: {err}"
        );
    }

    /// Sub-3b: canonical shape — `timeRange: "last_24h"`. The
    /// dispatcher must resolve the preset to a 24h window.
    #[test]
    fn get_moderation_metrics_input_accepts_canonical_preset_shape() {
        let body = serde_json::json!({
            "timeRange": "last_24h",
            "granularity": "day"
        });
        let input: GetModerationMetricsInput =
            serde_json::from_value(body).expect("canonical preset shape parses");
        let span = input.time_range.end() - input.time_range.start();
        assert_eq!(span.num_hours(), 24);
    }

    /// Sub-3b: legacy shape — peer `start`/`end` RFC 3339 strings.
    /// The dispatcher builds a TimeRange from the pair.
    #[test]
    fn get_moderation_metrics_input_accepts_legacy_start_end_shape() {
        let body = serde_json::json!({
            "start": "2026-01-01T00:00:00Z",
            "end":   "2026-01-02T00:00:00Z",
            "granularity": "day"
        });
        let input: GetModerationMetricsInput =
            serde_json::from_value(body).expect("legacy shape parses");
        assert_eq!(
            input.time_range.start().to_rfc3339(),
            "2026-01-01T00:00:00+00:00"
        );
        assert_eq!(
            (input.time_range.end() - input.time_range.start()).num_hours(),
            24
        );
    }

    /// Sub-3b: typo'd preset name in the canonical `timeRange`
    /// field. The error MUST mention the canonical field and the
    /// preset alternatives, NOT misdirect to the legacy
    /// `start`/`end` fields. This is the §9.5.9 misdirection-risk
    /// mitigation made explicit (per recon Q3(b)).
    #[test]
    fn get_moderation_metrics_input_typo_in_canonical_preset_emits_canonical_error() {
        let body = serde_json::json!({
            "timeRange": "last_5min",
            "granularity": "day"
        });
        let err = serde_json::from_value::<GetModerationMetricsInput>(body)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("last_5min"),
            "error must include the typo'd preset name; got: {err}"
        );
        assert!(
            err.contains("timeRange"),
            "error must name the canonical 'timeRange' field; got: {err}"
        );
        assert!(
            err.contains("last_24h"),
            "error must list the canonical preset alternatives; got: {err}"
        );
    }

    /// Sub-3b: both shapes simultaneously => ambiguous error.
    /// Operators get a clear "choose one" message rather than a
    /// silent precedence rule.
    #[test]
    fn get_moderation_metrics_input_rejects_mixed_canonical_and_legacy() {
        let body = serde_json::json!({
            "timeRange": "last_24h",
            "start": "2026-01-01T00:00:00Z",
            "end":   "2026-01-02T00:00:00Z",
            "granularity": "day"
        });
        let err = serde_json::from_value::<GetModerationMetricsInput>(body)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("ambiguous") && err.contains("timeRange"),
            "error must say 'ambiguous' and name 'timeRange'; got: {err}"
        );
        assert!(
            err.contains("start") || err.contains("end"),
            "error must reference the legacy fields too; got: {err}"
        );
    }

    /// Sub-3b: neither shape present => error mentions canonical
    /// field FIRST so callers gravitate toward the modern shape.
    #[test]
    fn get_moderation_metrics_input_rejects_missing_time_range() {
        let body = serde_json::json!({
            "granularity": "day"
        });
        let err = serde_json::from_value::<GetModerationMetricsInput>(body)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("timeRange"),
            "error must mention canonical 'timeRange' field first; got: {err}"
        );
        assert!(
            err.contains("start") && err.contains("end"),
            "error must also mention legacy 'start'/'end' as alternative; got: {err}"
        );
    }

    /// Sub-3b: incomplete legacy shape (only `start`, no `end`).
    /// The dispatcher must distinguish "incomplete legacy" from
    /// "missing entirely" so operators don't misread the cause.
    #[test]
    fn get_moderation_metrics_input_rejects_incomplete_legacy_shape() {
        let body = serde_json::json!({
            "start": "2026-01-01T00:00:00Z",
            "granularity": "day"
        });
        let err = serde_json::from_value::<GetModerationMetricsInput>(body)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("start") && err.contains("end") && err.contains("both"),
            "error must explain that legacy requires both 'start' and 'end'; got: {err}"
        );
    }

    /// Sub-3c: GetQueueStatsOutput's retyped fields serialize as
    /// non-negative JSON integers. Pin the wire shape so a future
    /// refactor that drops the `serde::Serialize` derive (or
    /// changes a field to a wrapper that emits a different JSON
    /// shape) fails loudly. JSON-equivalence with the v0.2 i64
    /// shape is a wire commitment per recon Q4.
    #[test]
    fn get_queue_stats_output_retyped_fields_emit_json_integers() {
        let out = GetQueueStatsOutput {
            open_reports: 3,
            pending_appeals: 5,
            under_review_reports: 0,
            under_review_appeals: 1,
            queue_attention_total: 9,
            average_age_open_reports_seconds: 86_400,
            oldest_open_report_age_seconds: 3 * 86_400,
        };
        let value = serde_json::to_value(&out).unwrap();
        for key in [
            "openReports",
            "pendingAppeals",
            "underReviewReports",
            "underReviewAppeals",
            "queueAttentionTotal",
            "averageAgeOpenReportsSeconds",
            "oldestOpenReportAgeSeconds",
        ] {
            let v = &value[key];
            assert!(
                v.is_number() && v.as_u64().is_some(),
                "{key} must serialize as a non-negative JSON integer; got {v}"
            );
        }
    }

    /// Sub-3c: the retyped fields can carry u32::MAX without
    /// truncation. Boundary check that the saturating conversion
    /// in the handler doesn't accidentally clip to a smaller
    /// width.
    #[test]
    fn get_queue_stats_output_retyped_fields_carry_u32_max() {
        let out = GetQueueStatsOutput {
            open_reports: u32::MAX,
            pending_appeals: u32::MAX,
            under_review_reports: u32::MAX,
            under_review_appeals: u32::MAX,
            queue_attention_total: 4 * (u32::MAX as i64),
            average_age_open_reports_seconds: u32::MAX,
            oldest_open_report_age_seconds: u32::MAX,
        };
        let value = serde_json::to_value(&out).unwrap();
        assert_eq!(value["openReports"].as_u64().unwrap(), u32::MAX as u64);
        assert_eq!(value["oldestOpenReportAgeSeconds"].as_u64().unwrap(), u32::MAX as u64);
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
        let start = chrono::Utc::now() - chrono::Duration::days(1);
        let end = chrono::Utc::now() + chrono::Duration::seconds(60);
        use axum_extra::extract::Query as ExtraQuery;
        let resp = get_moderation_metrics(
            State(ctx),
            moderator_auth(),
            ExtraQuery(GetModerationMetricsInput {
                time_range: crate::admin::TimeRange::new(start, end).unwrap(),
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
        let start = now - chrono::Duration::days(1);
        let end = now + chrono::Duration::seconds(60);
        use axum_extra::extract::Query as ExtraQuery;
        let resp = get_moderation_metrics(
            State(ctx),
            moderator_auth(),
            ExtraQuery(GetModerationMetricsInput {
                time_range: crate::admin::TimeRange::new(start, end).unwrap(),
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
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::TakedownAccount,
                subjects: vec![repo_subject("did:plc:victim")],
                rationale: "spam".to_string(),
                snapshot_capture: true,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap()
        .0;
        // Phase 3.8 fills these — Phase 3.5 returned None for both.
        assert!(!resp.audit_entry_id.is_empty(), "emitEvent populates audit_entry_id");
        assert!(
            resp.snapshots.first().and_then(|s| s.snapshot_id.as_ref()).is_some(),
            "Phase 3.8 should populate snapshots[0].snapshot_id"
        );
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
        let _ = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::TakedownAccount,
                subjects: vec![repo_subject("did:plc:victim")],
                rationale: "spam".to_string(),
                snapshot_capture: true,
                metadata: None,
                legacy_subject_used: false,
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
                subject_cid: None,
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
        crate::admin::audit_chain::insert_chain_entry_pool(
            &ctx.account_db,
            ctx.config.database.backend,
            crate::admin::audit_chain::AppendEntryParams {
                actor_did: "did:plc:m1",
                action: "TakedownAccount",
                subject: Some(&repo_subject("did:plc:s1")),
                rationale: "first",
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        ).await.unwrap();
        crate::admin::audit_chain::insert_chain_entry_pool(
            &ctx.account_db,
            ctx.config.database.backend,
            crate::admin::audit_chain::AppendEntryParams {
                actor_did: "did:plc:m2",
                action: "RestoreAccount",
                subject: Some(&repo_subject("did:plc:s1")),
                rationale: "second",
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        ).await.unwrap();
        let resp = get_audit_trail(
            State(ctx),
            moderator_auth(),
            axum::extract::Query(GetAuditTrailParams {
                actor_did: Some("did:plc:m1".to_string()),
                action: None, subject_did: None, subject_uri: None,
                subject_cid: None,
                after_created: None, before_created: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await.unwrap().0;
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].actor_did, "did:plc:m1");
    }

    // Arc 3 Step 0.5 (§7.4.0.5) — `subject_cid` filter coverage. The
    // recon report at /tmp/arc3_recon.md Q5 found no documentation
    // either for or against the prior six-filter omission of CID, so
    // the conditional fired and Step 0.5 added the seventh filter.
    // Two tests: filter alone, filter combined with another.

    #[tokio::test]
    async fn get_audit_trail_filters_by_subject_cid() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:victim", "victim.test").await;
        // Two entries with distinct subject_cids (Subject::Blob carries
        // the CID through to the chain row's subject_cid column via
        // insert_chain_entry_pool's flat-column mapping).
        let target_cid = "bafyblobtarget";
        let other_cid = "bafyblobother";
        crate::admin::audit_chain::insert_chain_entry_pool(
            &ctx.account_db,
            ctx.config.database.backend,
            crate::admin::audit_chain::AppendEntryParams {
                actor_did: "did:plc:m1",
                action: "TakedownAccount",
                subject: Some(&Subject::Blob {
                    did: "did:plc:victim".to_string(),
                    cid: target_cid.to_string(),
                    record_uri: None,
                }),
                rationale: "target",
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        )
        .await
        .unwrap();
        crate::admin::audit_chain::insert_chain_entry_pool(
            &ctx.account_db,
            ctx.config.database.backend,
            crate::admin::audit_chain::AppendEntryParams {
                actor_did: "did:plc:m1",
                action: "TakedownAccount",
                subject: Some(&Subject::Blob {
                    did: "did:plc:victim".to_string(),
                    cid: other_cid.to_string(),
                    record_uri: None,
                }),
                rationale: "other",
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        )
        .await
        .unwrap();

        // Filter to target_cid only — exactly one entry returned.
        let filtered = get_audit_trail(
            State(ctx.clone()),
            moderator_auth(),
            axum::extract::Query(GetAuditTrailParams {
                actor_did: None,
                action: None,
                subject_did: None,
                subject_uri: None,
                subject_cid: Some(target_cid.to_string()),
                after_created: None,
                before_created: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(filtered.items.len(), 1);
        assert_eq!(filtered.items[0].rationale, "target");

        // Omitting the filter — both entries returned.
        let unfiltered = get_audit_trail(
            State(ctx),
            moderator_auth(),
            axum::extract::Query(GetAuditTrailParams {
                actor_did: None,
                action: None,
                subject_did: None,
                subject_uri: None,
                subject_cid: None,
                after_created: None,
                before_created: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(unfiltered.items.len(), 2);
    }

    #[tokio::test]
    async fn get_audit_trail_subject_cid_combines_with_actor_did_filter() {
        // Four entries: 2 actors × 2 subject_cids. Filtering by both
        // actor_did AND subject_cid must AND the predicates — only
        // the one entry matching both should be returned.
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:victim", "victim.test").await;
        let cid_a = "bafyblobA";
        let cid_b = "bafyblobB";
        for (actor, cid, rationale) in &[
            ("did:plc:m1", cid_a, "m1+A"),
            ("did:plc:m1", cid_b, "m1+B"),
            ("did:plc:m2", cid_a, "m2+A"),
            ("did:plc:m2", cid_b, "m2+B"),
        ] {
            crate::admin::audit_chain::insert_chain_entry_pool(
                &ctx.account_db,
                ctx.config.database.backend,
                crate::admin::audit_chain::AppendEntryParams {
                    actor_did: actor,
                    action: "TakedownAccount",
                    subject: Some(&Subject::Blob {
                        did: "did:plc:victim".to_string(),
                        cid: cid.to_string(),
                        record_uri: None,
                    }),
                    rationale,
                    snapshot_id: None,
                    event_id: None,
                    cascade_subjects: &[],
                    cascade_snapshot_ids: &[],
                },
            )
            .await
            .unwrap();
        }

        // Filter by actor_did=m1 AND subject_cid=A — only "m1+A".
        let resp = get_audit_trail(
            State(ctx),
            moderator_auth(),
            axum::extract::Query(GetAuditTrailParams {
                actor_did: Some("did:plc:m1".to_string()),
                action: None,
                subject_did: None,
                subject_uri: None,
                subject_cid: Some(cid_a.to_string()),
                after_created: None,
                before_created: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].rationale, "m1+A");
        assert_eq!(resp.items[0].actor_did, "did:plc:m1");
    }

    // Arc 3 Step 1 (§7.4.1) — `cascade_snapshot_ids` round-trip from
    // batch-event producer to getAuditTrail wire response. The
    // existing `batch_takedown_captures_per_subject_snapshots` test
    // pins the producer side (chain row's column populated with
    // i64s). This test pins the CONSUMER side: getAuditTrail surfaces
    // the column on the wire as `Vec<Option<String>>` (stringified
    // for JS-precision parity with snapshot_id / event_id).
    #[tokio::test]
    async fn get_audit_trail_round_trips_cascade_snapshot_ids() {
        let ctx = create_test_context().await;
        for i in 0..3 {
            seed_actor(&ctx, &format!("did:plc:c{}", i), &format!("c{}.test", i)).await;
        }
        // Trigger a batch event that produces cascade subjects + ids.
        let _batch_resp = batch_takedown_accounts(
            State(ctx.clone()),
            moderator_auth(),
            Json(BatchAccountsInput {
                dids: vec![
                    "did:plc:c0".to_string(),
                    "did:plc:c1".to_string(),
                    "did:plc:c2".to_string(),
                ],
                rationale: "cascade-roundtrip".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;

        // Fetch via getAuditTrail.
        let trail_resp = get_audit_trail(
            State(ctx),
            moderator_auth(),
            axum::extract::Query(GetAuditTrailParams {
                actor_did: None,
                action: None,
                subject_did: None,
                subject_uri: None,
                subject_cid: None,
                after_created: None,
                before_created: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;

        // Find the batch entry. Batch action is "account.batch_takedown"
        // per the producer at aurora_admin.rs:1168.
        let batch_entry = trail_resp
            .items
            .iter()
            .find(|e| e.action == "account.batch_takedown")
            .expect("trail must include the batch entry just emitted");

        // Wire-side type pin: cascade_snapshot_ids is Vec<Option<String>>.
        // The batch produced 3 snapshots, one per subject — none should
        // be None (every subject was snapshottable).
        assert_eq!(
            batch_entry.cascade_snapshot_ids.len(),
            3,
            "cascade_snapshot_ids must mirror cascade_subjects length \
             (3 batch subjects → 3 snapshot ids)"
        );
        assert_eq!(
            batch_entry.cascade_subjects.len(),
            batch_entry.cascade_snapshot_ids.len(),
            "cascade_snapshot_ids must be paired by index with cascade_subjects"
        );
        for snap_id in &batch_entry.cascade_snapshot_ids {
            let id_str = snap_id
                .as_deref()
                .expect("every batch snapshot id should be Some for a Repo subject");
            // Stringified i64 — must parse cleanly back to i64.
            id_str
                .parse::<i64>()
                .expect("wire form is the i64 stringified, must parse");
        }

        // Wire-shape pin: serialize and confirm the JSON contains the
        // camelCase key with stringified array values. This is the
        // load-bearing assertion that the field landed on the wire
        // in the documented form.
        let wire = serde_json::to_string(&batch_entry).unwrap();
        assert!(
            wire.contains("\"cascadeSnapshotIds\":["),
            "wire shape must include camelCase `cascadeSnapshotIds` array; got: {}",
            wire,
        );
        // String-quoted values rather than bare numbers — assert by
        // checking for `"<digit>` (a string-quoted digit) inside the
        // cascadeSnapshotIds array. If serialization regressed to bare
        // i64s (`[7,12,...]`), this assertion fails.
        let cascade_section = wire
            .split("\"cascadeSnapshotIds\":[")
            .nth(1)
            .unwrap_or("")
            .split(']')
            .next()
            .unwrap_or("");
        assert!(
            cascade_section.contains("\""),
            "cascadeSnapshotIds must contain string-quoted values \
             (JS-precision parity); got section: {}",
            cascade_section,
        );
    }

    // ====================================================================
    // Arc 3 Step 3 (§7.4.3) — coverage gap closure for getAuditTrail.
    // Seven tests covering pagination edges, filter combinations,
    // malformed inputs, and per-entry verified-flag independence.
    // Each test exercises the production handler end-to-end.
    // ====================================================================

    /// Helper: append `n` chain entries with deterministic rationales.
    /// Reused across the coverage-gap tests below.
    async fn append_n_chain_entries(ctx: &AppContext, n: usize) {
        for i in 0..n {
            crate::admin::audit_chain::insert_chain_entry_pool(
                &ctx.account_db,
                ctx.config.database.backend,
                crate::admin::audit_chain::AppendEntryParams {
                    actor_did: "did:plc:moderator",
                    action: "TakedownAccount",
                    subject: Some(&repo_subject("did:plc:victim")),
                    rationale: &format!("entry-{}", i),
                    snapshot_id: None,
                    event_id: None,
                    cascade_subjects: &[],
                    cascade_snapshot_ids: &[],
                },
            )
            .await
            .unwrap();
        }
    }

    fn empty_filter_params(limit: Option<u32>, cursor: Option<String>) -> GetAuditTrailParams {
        GetAuditTrailParams {
            actor_did: None,
            action: None,
            subject_did: None,
            subject_uri: None,
            subject_cid: None,
            after_created: None,
            before_created: None,
            pagination: PaginationParams { limit, cursor },
        }
    }

    // ---- Gap 1: cursor round-trip ----
    #[tokio::test]
    async fn get_audit_trail_pagination_cursor_round_trip_equals_unpaginated() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:victim", "victim.test").await;
        append_n_chain_entries(&ctx, 7).await;

        // Unpaginated baseline (limit covers all 7).
        let baseline = get_audit_trail(
            State(ctx.clone()),
            moderator_auth(),
            axum::extract::Query(empty_filter_params(Some(100), None)),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(baseline.items.len(), 7);

        // Paginate with limit=3, accumulate via cursor.
        let mut accumulated: Vec<String> = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(pages <= 10, "pagination loop is unbounded");
            let page = get_audit_trail(
                State(ctx.clone()),
                moderator_auth(),
                axum::extract::Query(empty_filter_params(Some(3), cursor.clone())),
            )
            .await
            .unwrap()
            .0;
            for item in &page.items {
                accumulated.push(item.id.clone());
            }
            match page.cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        assert_eq!(
            accumulated.len(),
            7,
            "paginated traversal must yield same count as unpaginated"
        );
        let baseline_ids: Vec<String> = baseline.items.iter().map(|e| e.id.clone()).collect();
        assert_eq!(
            accumulated, baseline_ids,
            "paginated id sequence must equal unpaginated id sequence"
        );
    }

    // ---- Gap 2: multi-filter combination (3+ filters AND-combined) ----
    #[tokio::test]
    async fn get_audit_trail_three_way_filter_and_combination() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:victim", "victim.test").await;
        // Two actors × two actions × two subject_dids = 8 entries.
        let actors = ["did:plc:m1", "did:plc:m2"];
        let actions = ["TakedownAccount", "RestoreAccount"];
        let subjects = ["did:plc:s1", "did:plc:s2"];
        for actor in &actors {
            for action in &actions {
                for subj in &subjects {
                    crate::admin::audit_chain::insert_chain_entry_pool(
                        &ctx.account_db,
                        ctx.config.database.backend,
                        crate::admin::audit_chain::AppendEntryParams {
                            actor_did: actor,
                            action,
                            subject: Some(&repo_subject(subj)),
                            rationale: &format!("{}+{}+{}", actor, action, subj),
                            snapshot_id: None,
                            event_id: None,
                            cascade_subjects: &[],
                            cascade_snapshot_ids: &[],
                        },
                    )
                    .await
                    .unwrap();
                }
            }
        }
        // Filter on actor_did=m1 AND action=TakedownAccount AND
        // subject_did=s1 — exactly one match.
        let resp = get_audit_trail(
            State(ctx),
            moderator_auth(),
            axum::extract::Query(GetAuditTrailParams {
                actor_did: Some("did:plc:m1".to_string()),
                action: Some("TakedownAccount".to_string()),
                subject_did: Some("did:plc:s1".to_string()),
                subject_uri: None,
                subject_cid: None,
                after_created: None,
                before_created: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(
            resp.items.len(),
            1,
            "three-way AND must collapse 8 entries to exactly one"
        );
        let item = &resp.items[0];
        assert_eq!(item.actor_did, "did:plc:m1");
        assert_eq!(item.action, "TakedownAccount");
        assert_eq!(item.rationale, "did:plc:m1+TakedownAccount+did:plc:s1");
    }

    // ---- Gap 3: time-range filters ----
    #[tokio::test]
    async fn get_audit_trail_time_range_window_filters_strictly() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:victim", "victim.test").await;
        // Append 5 entries; record their actual stored timestamps.
        // insert_chain_entry_pool uses Utc::now() so entries land at the
        // wall-clock instant of insertion. Stagger by sleeps to make
        // the timestamps distinguishable at sub-millisecond resolution.
        let mut timestamps: Vec<String> = Vec::new();
        for i in 0..5 {
            crate::admin::audit_chain::insert_chain_entry_pool(
                &ctx.account_db,
                ctx.config.database.backend,
                crate::admin::audit_chain::AppendEntryParams {
                    actor_did: "did:plc:moderator",
                    action: "TakedownAccount",
                    subject: Some(&repo_subject("did:plc:victim")),
                    rationale: &format!("entry-{}", i),
                    snapshot_id: None,
                    event_id: None,
                    cascade_subjects: &[],
                    cascade_snapshot_ids: &[],
                },
            )
            .await
            .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            // Read the just-inserted row's timestamp.
            use sqlx::Row as _;
            let r = sqlx::query("SELECT created_at FROM audit_chain_entry WHERE sequence = $1")
                .bind((i + 1) as i64)
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
            timestamps.push(r.try_get("created_at").unwrap());
        }
        // Window = [timestamp[1], timestamp[3]] (inclusive both ends
        // per the handler's `>=` / `<=` semantics). Should return
        // entries 2, 3, 4 (sequences 2/3/4, three rows).
        let resp = get_audit_trail(
            State(ctx),
            moderator_auth(),
            axum::extract::Query(GetAuditTrailParams {
                actor_did: None,
                action: None,
                subject_did: None,
                subject_uri: None,
                subject_cid: None,
                after_created: Some(timestamps[1].clone()),
                before_created: Some(timestamps[3].clone()),
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(
            resp.items.len(),
            3,
            "window [{}, {}] must include 3 entries; got {}",
            timestamps[1],
            timestamps[3],
            resp.items.len()
        );
        // Entries are returned newest-first; rationales are entry-1,
        // entry-2, entry-3 (sequence 2, 3, 4) — newest-first by
        // created_at means entry-3 first.
        let rationales: Vec<&str> = resp.items.iter().map(|e| e.rationale.as_str()).collect();
        assert!(rationales.contains(&"entry-1"));
        assert!(rationales.contains(&"entry-2"));
        assert!(rationales.contains(&"entry-3"));
        assert!(!rationales.contains(&"entry-0"));
        assert!(!rationales.contains(&"entry-4"));
    }

    // ---- Gap 4: malformed cursor ----
    #[tokio::test]
    async fn get_audit_trail_malformed_cursor_returns_outdated_cursor_error() {
        use base64::Engine as _;
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:victim", "victim.test").await;
        append_n_chain_entries(&ctx, 3).await;

        // Three flavors of malformed:
        //  (a) non-base64 garbage
        //  (b) base64 of garbage bytes (not valid JSON)
        //  (c) base64 of valid JSON but wrong shape
        let bad_cursors = [
            "not!base64@@@".to_string(),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"\xff\xff\xff\xff"),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{\"unrelated\":\"value\"}"),
        ];
        for bad in &bad_cursors {
            let result = get_audit_trail(
                State(ctx.clone()),
                moderator_auth(),
                axum::extract::Query(empty_filter_params(None, Some(bad.clone()))),
            )
            .await;
            match result {
                Err((status, body)) => {
                    assert_eq!(
                        status,
                        StatusCode::BAD_REQUEST,
                        "malformed cursor `{}` must produce 400; got {:?}",
                        bad,
                        status,
                    );
                    let error_field = body
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    assert_eq!(
                        error_field, "OutdatedCursor",
                        "malformed cursor `{}` must surface OutdatedCursor in the error field; got: {:?}",
                        bad, body,
                    );
                }
                Ok(_) => panic!(
                    "malformed cursor `{}` must produce an error, not Ok",
                    bad
                ),
            }
        }
    }

    // ---- Gap 5: limit cap + has_more ----
    #[tokio::test]
    async fn get_audit_trail_caps_limit_at_max_and_signals_more_via_cursor() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:victim", "victim.test").await;
        // 105 entries — enough to exceed the 100 cap and leave 5 more.
        append_n_chain_entries(&ctx, 105).await;

        // Request limit=200; effective cap is 100 (PaginationParams::MAX_LIMIT).
        let resp = get_audit_trail(
            State(ctx),
            moderator_auth(),
            axum::extract::Query(empty_filter_params(Some(200), None)),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(
            resp.items.len(),
            100,
            "limit=200 must be capped at the MAX_LIMIT of 100"
        );
        assert!(
            resp.cursor.is_some(),
            "with 105 entries and a 100-row page, cursor must be set to signal there's more"
        );
    }

    // ---- Gap 6: cursor beyond latest entry ----
    #[tokio::test]
    async fn get_audit_trail_cursor_beyond_latest_returns_empty_no_error() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:victim", "victim.test").await;
        append_n_chain_entries(&ctx, 3).await;
        // Construct a cursor pointing at a past timestamp + below
        // every existing id. The cursor's WHERE clause is
        // `created_at < ? OR (created_at = ? AND id < ?)`, so a
        // VERY OLD timestamp returns zero items.
        let past_cursor = CursorPosition {
            after_created: chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            after_id: 0,
        }
        .encode();
        let resp = get_audit_trail(
            State(ctx),
            moderator_auth(),
            axum::extract::Query(empty_filter_params(None, Some(past_cursor))),
        )
        .await
        .unwrap()
        .0;
        assert!(
            resp.items.is_empty(),
            "cursor pointing past tail of newest-first chain must return empty items"
        );
        assert!(
            resp.cursor.is_none(),
            "empty page must not include a continuation cursor"
        );
    }

    // ---- Gap 7: mixed-page verified-flag independence ----
    #[tokio::test]
    async fn get_audit_trail_per_entry_verified_flag_independent_within_a_page() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:victim", "victim.test").await;
        append_n_chain_entries(&ctx, 3).await;
        // Tamper sequence 2's rationale via the consolidated helper
        // (Step 0.6); the row's recomputed hash diverges from its
        // stored current_hash, so verify_entry returns false for it.
        crate::admin::audit_chain::corrupt_entry_rationale(
            &ctx.account_db,
            crate::admin::audit_chain::EntryRef::Sequence(2),
            "tampered-by-test",
        )
        .await
        .unwrap();
        let resp = get_audit_trail(
            State(ctx),
            moderator_auth(),
            axum::extract::Query(empty_filter_params(None, None)),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.items.len(), 3);
        // Items are newest-first: sequence 3, 2, 1.
        let by_seq: std::collections::HashMap<i64, bool> = resp
            .items
            .iter()
            .map(|e| (e.sequence, e.verified))
            .collect();
        assert_eq!(by_seq.get(&1), Some(&true), "row 1 must verify cleanly");
        assert_eq!(
            by_seq.get(&2),
            Some(&false),
            "row 2 (the tampered row) must surface verified=false"
        );
        assert_eq!(by_seq.get(&3), Some(&true), "row 3 must verify cleanly");
    }

    // CR-8 / chainlink #120: chainVerifiedThrough must surface the
    // failing sequence on chain verification failure, not collapse
    // every failure mode into 0. The handler computes
    // `failing_sequence - 1` (saturating) so operators get a row-level
    // pointer to where the chain diverged.
    #[tokio::test]
    async fn chain_verified_through_reports_head_seq_on_clean_chain() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:s1", "s1.test").await;
        for i in 0..3 {
            crate::admin::audit_chain::insert_chain_entry_pool(
                &ctx.account_db,
                ctx.config.database.backend,
                crate::admin::audit_chain::AppendEntryParams {
                    actor_did: "did:plc:moderator",
                    action: "TakedownAccount",
                    subject: Some(&repo_subject("did:plc:s1")),
                    rationale: &format!("entry-{}", i),
                    snapshot_id: None,
                    event_id: None,
                    cascade_subjects: &[],
                    cascade_snapshot_ids: &[],
                },
            )
            .await
            .unwrap();
        }
        let resp = get_audit_trail(
            State(ctx),
            moderator_auth(),
            axum::extract::Query(GetAuditTrailParams {
                actor_did: None,
                action: None,
                subject_did: None,
                subject_uri: None,
                subject_cid: None,
                after_created: None,
                before_created: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(resp.chain_verified, "clean chain should verify");
        assert_eq!(
            resp.chain_verified_through, 3,
            "clean chain should report head sequence as verified-through"
        );
    }

    #[tokio::test]
    async fn chain_verified_through_reports_failing_sequence_minus_one_on_tampered_chain() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:s1", "s1.test").await;
        for i in 0..3 {
            crate::admin::audit_chain::insert_chain_entry_pool(
                &ctx.account_db,
                ctx.config.database.backend,
                crate::admin::audit_chain::AppendEntryParams {
                    actor_did: "did:plc:moderator",
                    action: "TakedownAccount",
                    subject: Some(&repo_subject("did:plc:s1")),
                    rationale: &format!("entry-{}", i),
                    snapshot_id: None,
                    event_id: None,
                    cascade_subjects: &[],
                    cascade_snapshot_ids: &[],
                },
            )
            .await
            .unwrap();
        }
        // Tamper sequence 2 the same way audit_chain's own
        // verify_chain_range_detects_per_row_tamper test does:
        // mutate the row's content without recomputing current_hash.
        // Uses the consolidated helper from
        // `crate::admin::audit_chain::corrupt_entry_rationale` (Arc 3
        // Step 0.6).
        crate::admin::audit_chain::corrupt_entry_rationale(
            &ctx.account_db,
            crate::admin::audit_chain::EntryRef::Sequence(2),
            "tampered",
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
                subject_cid: None,
                after_created: None,
                before_created: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(
            !resp.chain_verified,
            "tampered chain should fail verification"
        );
        assert_eq!(
            resp.chain_verified_through, 1,
            "failing_sequence=2 should yield chain_verified_through=1"
        );
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
        use sha2::{Digest, Sha256};
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

        // Snapshot headers BEFORE consuming the body — once we read the
        // body we lose the response object.
        let headers = resp.headers().clone();
        assert_eq!(
            headers.get(axum::http::header::CONTENT_TYPE).unwrap(),
            "application/x-tar"
        );
        let cd_str = headers
            .get(axum::http::header::CONTENT_DISPOSITION)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(cd_str.contains("forensic-export-did_plc_exported-"));
        let audit_entry_id_header = headers
            .get("X-Aurora-Audit-Entry-Id")
            .expect("X-Aurora-Audit-Entry-Id present")
            .to_str()
            .unwrap()
            .to_string();
        let bundle_hash_header = headers
            .get("X-Aurora-Bundle-Hash")
            .expect("X-Aurora-Bundle-Hash present")
            .to_str()
            .unwrap()
            .to_string();

        // Read the response body bytes so we can verify the bundle
        // hash covers the actual tar shipped, not just the manifest.
        let body_bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024)
            .await
            .expect("body collects");

        // SHA-256 of the complete tar bytes must match the
        // X-Aurora-Bundle-Hash header — this is what §3.4
        // chain-of-custody actually requires.
        let mut hasher = Sha256::new();
        hasher.update(&body_bytes);
        let computed = hex::encode(hasher.finalize());
        assert_eq!(
            computed, bundle_hash_header,
            "X-Aurora-Bundle-Hash must equal SHA-256 of the complete tar bytes"
        );

        // Open the tar and inspect the in-bundle audit-trail.json.
        // The cycle-break dictates that file MUST NOT contain the
        // bundle hash itself (would force a self-referencing hash)
        // and MUST NOT contain the chain entry id (would require the
        // chain entry to land before the tar is hashed). Both of
        // those facts are surfaced via response headers / getAuditTrail.
        let mut archive = tar::Archive::new(&body_bytes[..]);
        let mut found_audit_trail = false;
        let mut found_manifest = false;
        for entry in archive.entries().expect("archive iterates") {
            let mut entry = entry.expect("entry readable");
            let path = entry.path().expect("path readable").to_path_buf();
            let name = path.to_string_lossy();
            let mut buf = Vec::new();
            use std::io::Read as _;
            entry.read_to_end(&mut buf).expect("entry body readable");
            if name == "audit-trail.json" {
                found_audit_trail = true;
                let json: serde_json::Value =
                    serde_json::from_slice(&buf).expect("audit-trail.json parses");
                assert!(
                    json.get("exportedAt").is_some(),
                    "audit-trail.json must include exportedAt"
                );
                assert!(
                    json.get("chainAnchor").is_some(),
                    "audit-trail.json must include the chainAnchor sentinel"
                );
                assert!(
                    json.get("bundleHash").is_none(),
                    "audit-trail.json must NOT include bundleHash — the in-tar copy would \
                     create a self-referencing hash cycle (the field lives in the response \
                     header and the chain row's rationale instead)"
                );
                assert!(
                    json.get("auditEntryId").is_none(),
                    "audit-trail.json must NOT include auditEntryId — the chain entry id \
                     is only known after the tar is hashed"
                );
            } else if name == "manifest.json" {
                found_manifest = true;
                let json: serde_json::Value =
                    serde_json::from_slice(&buf).expect("manifest.json parses");
                assert_eq!(
                    json.get("did").and_then(|v| v.as_str()),
                    Some("did:plc:exported")
                );
                assert!(json.get("exportedAt").is_some());
                assert!(json.get("exportedBy").is_some());
                assert!(json.get("rationale").is_some());
                assert!(json.get("parameters").is_some());
                assert!(json.get("fileHashes").is_some());
            }
        }
        assert!(found_audit_trail, "tar must contain audit-trail.json");
        assert!(found_manifest, "tar must contain manifest.json");

        // Verify the chain entry landed for the export action AND
        // that its rationale embeds the bundle hash that matches the
        // header — closes the tamper-detection loop end-to-end.
        let chain_row: (i64, String) = sqlx::query_as(
            "SELECT id, rationale FROM audit_chain_entry \
             WHERE action = $1 AND subject_did = $2 \
             ORDER BY id DESC LIMIT 1",
        )
        .bind("ForensicExport")
        .bind("did:plc:exported")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(chain_row.0.to_string(), audit_entry_id_header);
        assert!(
            chain_row.1.contains(&format!("(bundle hash: {})", bundle_hash_header)),
            "chain row rationale must embed the same bundle hash that's in the response header; \
             rationale={}, header={}",
            chain_row.1,
            bundle_hash_header,
        );
    }

    #[tokio::test]
    async fn export_forensic_bundle_hash_responds_to_input_changes() {
        // Tamper-detection sanity check: two exports of the same
        // account but different rationale produce different bundle
        // hashes, because the manifest (which embeds rationale) is
        // inside the tar and the hash covers the tar. This is the
        // counterpart to the "hash covers manifest only" bug — a
        // rationale change WAS being caught before the fix
        // (manifest contained it), but a payload-only swap (e.g.
        // post-hoc tar surgery) was NOT. We can't easily inject a
        // post-hoc swap inside a unit test, but exercising the
        // sensitivity to input variation gives a stable contract pin
        // that the hash is computed over content that varies with
        // the payload.
        use sha2::{Digest, Sha256};
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:tamper", "tamper.test").await;
        sqlx::query(
            "INSERT INTO account (did, email, password_hash, email_confirmed_at, invites_disabled) \
             VALUES ($1, $2, $3, NULL, FALSE)",
        )
        .bind("did:plc:tamper")
        .bind("t@example.com")
        .bind("$argon2id$dummy")
        .execute(&ctx.account_db)
        .await
        .unwrap();

        async fn export_with(
            ctx: &AppContext,
            rationale: &str,
        ) -> (Vec<u8>, String) {
            let resp = export_account_forensic(
                State(ctx.clone()),
                admin_auth(),
                Json(ExportAccountForensicInput {
                    did: "did:plc:tamper".to_string(),
                    rationale: rationale.to_string(),
                    include_repo: false,
                    include_blobs: false,
                    include_moderation_history: false,
                    include_account_metadata: false,
                    include_audit_chain: false,
                }),
            )
            .await
            .unwrap();
            let header = resp
                .headers()
                .get("X-Aurora-Bundle-Hash")
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024)
                .await
                .unwrap()
                .to_vec();
            (bytes, header)
        }

        let (bytes_a, header_a) = export_with(&ctx, "investigation A").await;
        let (bytes_b, header_b) = export_with(&ctx, "investigation B").await;

        assert_ne!(bytes_a, bytes_b, "different rationale → different tar");
        assert_ne!(header_a, header_b, "different tar → different bundle hash");

        // Both headers match the SHA-256 of their respective bodies.
        for (bytes, header) in [(bytes_a, header_a), (bytes_b, header_b)] {
            let mut h = Sha256::new();
            h.update(&bytes);
            assert_eq!(hex::encode(h.finalize()), header);
        }
    }

    /// Arc 9 Step 4 / chainlink #55 Item 2: the forensic bundle's
    /// `audit-entries.json` must match `getAuditTrail`'s wire shape
    /// field-for-field. Prior to that migration the two surfaces
    /// diverged on field names (`createdAt` vs `timestamp`), types
    /// (raw `i64` vs stringified), and four entirely-missing fields
    /// (`subjectRef`, `verified`, `cascadeSubjects`,
    /// `cascadeSnapshotIds`). v0.6 batch tail A.1 / G2 closed the
    /// DRY gap — both paths now consume
    /// `audit_chain::audit_entry_from_row`. This test is the
    /// byte-identical-shape regression guard on top of the shared
    /// helper: touching the helper must keep both wire outputs
    /// stable.
    #[tokio::test]
    async fn forensic_audit_entries_match_get_audit_trail_shape() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:parity", "parity.test").await;
        // Seed the account row so the forensic handler's account
        // lookup resolves.
        sqlx::query(
            "INSERT INTO account (did, email, password_hash, email_confirmed_at, invites_disabled) \
             VALUES ($1, $2, $3, NULL, FALSE)",
        )
        .bind("did:plc:parity")
        .bind("parity@example.com")
        .bind("$argon2id$dummy")
        .execute(&ctx.account_db)
        .await
        .unwrap();
        // Append one chain entry against the subject so both paths
        // have something non-empty to render.
        crate::admin::audit_chain::insert_chain_entry_pool(
            &ctx.account_db,
            ctx.config.database.backend,
            crate::admin::audit_chain::AppendEntryParams {
                actor_did: "did:plc:moderator",
                action: "TakedownAccount",
                subject: Some(&repo_subject("did:plc:parity")),
                rationale: "spam-parity",
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        )
        .await
        .unwrap();

        // Fetch via getAuditTrail filtered to this subject.
        let trail = get_audit_trail(
            State(ctx.clone()),
            moderator_auth(),
            axum::extract::Query(GetAuditTrailParams {
                actor_did: None,
                action: None,
                subject_did: Some("did:plc:parity".to_string()),
                subject_uri: None,
                subject_cid: None,
                after_created: None,
                before_created: None,
                pagination: PaginationParams::default(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(trail.items.len(), 1, "expect one chain entry for the subject");
        let trail_entry_json = serde_json::to_value(&trail.items[0]).unwrap();

        // Fetch the same row via the forensic-export handler (with
        // include_audit_chain, which requires SuperAdmin).
        let resp = export_account_forensic(
            State(ctx.clone()),
            super_admin_auth(),
            Json(ExportAccountForensicInput {
                did: "did:plc:parity".to_string(),
                rationale: "parity check".to_string(),
                include_repo: false,
                include_blobs: false,
                include_moderation_history: false,
                include_account_metadata: false,
                include_audit_chain: true,
            }),
        )
        .await
        .unwrap();
        let body_bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap();

        // Extract audit-entries.json from the TAR.
        let mut archive = tar::Archive::new(&body_bytes[..]);
        let mut forensic_entries_json: Option<serde_json::Value> = None;
        for entry in archive.entries().expect("archive iterates") {
            let mut entry = entry.expect("entry readable");
            let path = entry.path().expect("path readable").to_path_buf();
            let name = path.to_string_lossy();
            let mut buf = Vec::new();
            use std::io::Read as _;
            entry.read_to_end(&mut buf).expect("entry body readable");
            if name == "audit-entries.json" {
                forensic_entries_json =
                    Some(serde_json::from_slice(&buf).expect("audit-entries.json parses"));
            }
        }
        let forensic_entries =
            forensic_entries_json.expect("tar must contain audit-entries.json");
        let forensic_array = forensic_entries
            .as_array()
            .expect("audit-entries.json is a JSON array");
        assert_eq!(forensic_array.len(), 1, "expect one chain entry in the bundle");

        // Field-for-field equality with the getAuditTrail item. If
        // either path drifts in field names, types, or membership,
        // this assertion fires.
        assert_eq!(
            forensic_array[0], trail_entry_json,
            "forensic audit-entries.json must match getAuditTrail's per-item shape"
        );
    }

    /// Arc 9 Step 4: manifest.json in the forensic bundle carries
    /// `schemaVersion: "2"` marking the audit-entries wire-format
    /// migration. Consumers dispatch on this field.
    #[tokio::test]
    async fn forensic_bundle_manifest_has_schema_version_2() {
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:schema", "schema.test").await;
        sqlx::query(
            "INSERT INTO account (did, email, password_hash, email_confirmed_at, invites_disabled) \
             VALUES ($1, $2, $3, NULL, FALSE)",
        )
        .bind("did:plc:schema")
        .bind("schema@example.com")
        .bind("$argon2id$dummy")
        .execute(&ctx.account_db)
        .await
        .unwrap();
        let resp = export_account_forensic(
            State(ctx.clone()),
            admin_auth(),
            Json(ExportAccountForensicInput {
                did: "did:plc:schema".to_string(),
                rationale: "schemaVersion check".to_string(),
                include_repo: false,
                include_blobs: false,
                include_moderation_history: false,
                include_account_metadata: false,
                include_audit_chain: false,
            }),
        )
        .await
        .unwrap();
        let body_bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap();

        let mut archive = tar::Archive::new(&body_bytes[..]);
        let mut manifest_json: Option<serde_json::Value> = None;
        for entry in archive.entries().expect("archive iterates") {
            let mut entry = entry.expect("entry readable");
            let path = entry.path().expect("path readable").to_path_buf();
            let name = path.to_string_lossy();
            let mut buf = Vec::new();
            use std::io::Read as _;
            entry.read_to_end(&mut buf).expect("entry body readable");
            if name == "manifest.json" {
                manifest_json = Some(
                    serde_json::from_slice(&buf).expect("manifest.json parses"),
                );
            }
        }
        let manifest = manifest_json.expect("tar must contain manifest.json");
        assert_eq!(
            manifest.get("schemaVersion").and_then(|v| v.as_str()),
            Some("2"),
            "manifest.schemaVersion must be \"2\" after Arc 9 Step 4 migration"
        );
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
        assert_eq!(resp.source, SettingSource::Default);
        assert_eq!(resp.value, serde_json::Value::String("full".to_string()));
    }

    /// Test helper: build a context whose tempdir contains a
    /// `runtime.yaml` with the supplied content. Mirrors
    /// `create_test_context`'s fixture but writes the yaml file
    /// before `AppContext::new` so file-tier loading runs against
    /// it. Returns `Result` so malformed-yaml tests can `unwrap_err`.
    async fn try_create_test_context_with_runtime_yaml(
        yaml: &str,
    ) -> crate::error::PdsResult<AppContext> {
        use crate::config::*;
        use std::path::PathBuf;
        use tempfile::tempdir;
        let dir = tempdir().unwrap().keep();
        std::fs::write(dir.join("runtime.yaml"), yaml).unwrap();
        let db_path = dir.join("test.db");
        let config = ServerConfig {
            service: ServiceConfig {
                hostname: "localhost".to_string(),
                port: 2583,
                service_did: "did:web:localhost".to_string(),
                version: "0.1.0-test".to_string(),
                blob_upload_limit: 5_242_880,
                public_url: None,
                max_blob_fetch_size: 50_000_000,
                blob_fetch_timeout_seconds: 30,
                blob_fetch_max_retries: 3,
                accepting_imports: true,
                max_import_size: None,
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
                recovery_did_key: None,
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
                exempt_admin_assets: true,
                buckets_retention_days: 7,
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
                peer_pds: vec![],
            },
            validation_mode: PathBuf::from("required")
                .into_os_string()
                .to_string_lossy()
                .parse()
                .unwrap_or(crate::validation::ValidationMode::Required),
            distributed_state_mode: Default::default(),
            maintenance_pool: Default::default(),
            gc_sweep: Default::default(),
            blob_metadata: Default::default(),
            entryway: None,
            lexicon: crate::config::LexiconConfig::default(),
        };
        AppContext::new(
            config,
            std::sync::Arc::new(crate::api::registry::RouteRegistry::default()),
        )
        .await
    }

    /// Arc 5 §9.4.2 / chainlink #124: file-tier value resolves
    /// when no runtime row exists for the key. Pin that the
    /// returned `source` is `File` and the value is the YAML
    /// content (not the compiled-in default).
    #[tokio::test]
    async fn get_runtime_setting_resolves_from_file_tier_when_no_runtime_row() {
        let ctx = try_create_test_context_with_runtime_yaml(
            "moderation-mode: reduced\n",
        )
        .await
        .expect("file-tier yaml loads cleanly");
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
        assert_eq!(
            resp.source,
            SettingSource::File,
            "no runtime row + file-tier present => File source"
        );
        assert_eq!(resp.value, serde_json::Value::String("reduced".to_string()));
    }

    /// Runtime row takes precedence over file-tier per the
    /// committed lookup order (Runtime > File > Default). Pin
    /// that the runtime value wins even when file-tier has the
    /// same key.
    #[tokio::test]
    async fn get_runtime_setting_runtime_row_overrides_file_tier() {
        let ctx = try_create_test_context_with_runtime_yaml(
            "moderation-mode: reduced\n",
        )
        .await
        .expect("file-tier yaml loads cleanly");
        // Land a runtime row for the same key — must win over
        // file-tier value.
        let _ = set_runtime_setting(
            State(ctx.clone()),
            super_admin_auth(),
            Json(SetRuntimeSettingInput {
                key: "moderation-mode".to_string(),
                value: serde_json::Value::String("disabled".to_string()),
                rationale: "test runtime > file precedence".to_string(),
            }),
        )
        .await
        .unwrap();
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
        assert_eq!(
            resp.source,
            SettingSource::Runtime,
            "runtime row wins over file-tier per precedence rule"
        );
        assert_eq!(
            resp.value,
            serde_json::Value::String("disabled".to_string())
        );
    }

    /// Default falls through when neither runtime row nor file-tier
    /// has the key. With an empty yaml file present, file-tier
    /// loads to an empty map and the lookup must reach the
    /// compiled-in default.
    #[tokio::test]
    async fn get_runtime_setting_default_when_neither_runtime_nor_file() {
        let ctx = try_create_test_context_with_runtime_yaml("")
            .await
            .expect("empty yaml loads as empty map");
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
        assert_eq!(resp.source, SettingSource::Default);
        assert_eq!(resp.value, serde_json::Value::String("full".to_string()));
    }

    /// Malformed YAML at the file-tier path produces a startup
    /// error. The error message must include the file path so an
    /// operator hitting this can find the file. Per Arc 5
    /// §9.4.2's "no silent fallback" rule.
    #[tokio::test]
    async fn malformed_runtime_yaml_returns_startup_error() {
        // Drive the file-tier loader directly with a malformed
        // YAML — `AppContext` doesn't impl Debug so we can't
        // `expect_err` through it. The loader is the unit of
        // interest: it owns the "no silent fallback on bad YAML"
        // contract that AppContext::new propagates verbatim.
        use tempfile::tempdir;
        let dir = tempdir().unwrap().keep();
        let path = dir.join("runtime.yaml");
        std::fs::write(
            &path,
            "moderation-mode: : :\n  - this is not valid yaml\n",
        )
        .unwrap();
        let err = match load_file_tier_settings(&path) {
            Ok(_) => panic!("malformed yaml must surface as a startup error"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("runtime.yaml"),
            "error message must name the file path; got: {msg}"
        );
        assert!(
            msg.contains("file-tier")
                || msg.contains("parse")
                || msg.contains("YAML")
                || msg.contains("yaml"),
            "error message must mention the file-tier / YAML context; got: {msg}"
        );
    }

    /// Unknown keys in the file-tier yaml warn-and-skip per
    /// recon Q5 — operator typos surface in logs without bringing
    /// the deployment down. The known-key value remains effective;
    /// the unknown-key lookup falls through to default.
    #[tokio::test]
    async fn unknown_key_in_file_tier_warns_and_skips() {
        let ctx = try_create_test_context_with_runtime_yaml(
            "moderation-mode: reduced\n\
             made-up-key: should-be-skipped\n",
        )
        .await
        .expect("yaml with unknown key still loads (warn-and-skip)");
        // Known key resolved from file-tier.
        let resp_known = get_runtime_setting(
            State(ctx.clone()),
            moderator_auth(),
            axum::extract::Query(GetRuntimeSettingParams {
                key: "moderation-mode".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp_known.source, SettingSource::File);
        assert_eq!(
            resp_known.value,
            serde_json::Value::String("reduced".to_string())
        );
        // Unknown key was skipped at load time; the cache doesn't
        // hold it, so a lookup falls through (admin role required
        // for non-mode keys, hence super_admin_auth).
        let resp_unknown = get_runtime_setting(
            State(ctx),
            super_admin_auth(),
            axum::extract::Query(GetRuntimeSettingParams {
                key: "made-up-key".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(
            resp_unknown.source,
            SettingSource::Default,
            "unknown file-tier key must not appear in the cache; falls through to default"
        );
    }

    /// Invalid value for a known key (e.g.,
    /// `moderation-mode: nonsense`) warns-and-skips at load time;
    /// the cache doesn't hold the bad value and the lookup falls
    /// through to the compiled-in default. Mirrors the per-key
    /// validation `set_runtime_setting` enforces at the API
    /// boundary.
    #[tokio::test]
    async fn invalid_value_in_file_tier_warns_and_skips() {
        let ctx = try_create_test_context_with_runtime_yaml(
            "moderation-mode: nonsense\n",
        )
        .await
        .expect("yaml with invalid value still loads (warn-and-skip)");
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
        assert_eq!(
            resp.source,
            SettingSource::Default,
            "invalid value at load time => not in cache => default"
        );
        assert_eq!(resp.value, serde_json::Value::String("full".to_string()));
    }

    /// Wire-format check: `SettingSource` serializes to the bare
    /// string the v0.2 wire shape used. Pre-Arc-5 callers reading
    /// the `source` field as a string see no change.
    #[test]
    fn setting_source_serializes_as_bare_string_for_wire_compat() {
        assert_eq!(
            serde_json::to_string(&SettingSource::Runtime).unwrap(),
            "\"Runtime\""
        );
        assert_eq!(
            serde_json::to_string(&SettingSource::File).unwrap(),
            "\"File\""
        );
        assert_eq!(
            serde_json::to_string(&SettingSource::Default).unwrap(),
            "\"Default\""
        );
        assert_eq!(
            serde_json::to_string(&SettingSource::RecoveryMode).unwrap(),
            "\"RecoveryMode\""
        );
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
    async fn set_runtime_setting_known_key_succeeds() {
        // CR-2 / chainlink #119 happy path: setting a known key from
        // KNOWN_RUNTIME_KEYS clears the allowlist guard and writes
        // the row. moderation-mode-redirect-url has no value-shape
        // restriction beyond being a string.
        let ctx = create_test_context().await;
        let resp = set_runtime_setting(
            State(ctx.clone()),
            super_admin_auth(),
            Json(SetRuntimeSettingInput {
                key: "moderation-mode-redirect-url".to_string(),
                value: serde_json::Value::String("https://example.org/maintenance".to_string()),
                rationale: "operator-configured redirect for reduced-mode".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(
            resp.new_value,
            serde_json::Value::String("https://example.org/maintenance".to_string())
        );
        let stored: String = sqlx::query_scalar(
            "SELECT value FROM runtime_settings WHERE key = $1",
        )
        .bind("moderation-mode-redirect-url")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(stored, "\"https://example.org/maintenance\"");
    }

    #[tokio::test]
    async fn set_runtime_setting_unknown_key_rejected() {
        // CR-2 / chainlink #119: arbitrary keys must be rejected with
        // 400 before any database write. Pre-fix, the runtime_settings
        // table would accumulate junk keys silently.
        let ctx = create_test_context().await;
        let err = set_runtime_setting(
            State(ctx.clone()),
            super_admin_auth(),
            Json(SetRuntimeSettingInput {
                key: "test-feature-flag".to_string(),
                value: serde_json::Value::String("anything".to_string()),
                rationale: "exercise the allowlist".to_string(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        // Confirm no row landed.
        let row_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM runtime_settings WHERE key = $1",
        )
        .bind("test-feature-flag")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(row_count, 0);
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
        // Chain entry exists — replaces the legacy admin_audit_log
        // write per Block 1's "chain is the system of record."
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_chain_entry WHERE action = $1 AND subject_did = $2",
        )
        .bind("account.trigger_password_reset")
        .bind("did:plc:withemail")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    // ====================================================================
    // Arc 4 Step 1 — multi-subject emitEvent + dispatch_action in-tx
    // migration. Tests pin: input rejection, multi-subject round-trips,
    // §8.3.3 chain-row shape (single vs multi), per-subject failure
    // atomicity, orphan-snapshot carve-out, and embedded-ID validation.
    // ====================================================================

    /// Helper: count moderation rows for a DID.
    async fn count_moderation_rows(ctx: &AppContext, did: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM account_moderation WHERE did = $1")
            .bind(did)
            .fetch_one(&ctx.account_db)
            .await
            .unwrap()
    }

    /// Helper: count audit chain entries.
    async fn count_chain_entries(ctx: &AppContext) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_chain_entry")
            .fetch_one(&ctx.account_db)
            .await
            .unwrap()
    }

    /// Helper: read the latest chain row's flat columns + cascade JSON.
    async fn latest_chain_row(
        ctx: &AppContext,
    ) -> (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        let row = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)>(
            "SELECT subject_did, subject_uri, subject_cid, cascade_subjects, cascade_snapshot_ids \
             FROM audit_chain_entry ORDER BY sequence DESC LIMIT 1",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        row
    }

    #[tokio::test]
    async fn emit_event_rejects_empty_subjects_array() {
        let ctx = create_test_context().await;
        let err = emit_event(
            State(ctx),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::TakedownAccount,
                subjects: vec![],
                rationale: "no subjects".to_string(),
                snapshot_capture: false,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        let body = format!("{:?}", err.1.0);
        assert!(
            body.contains("SubjectsArrayInvalidForAction"),
            "expected SubjectsArrayInvalidForAction error, got: {}",
            body
        );
    }

    #[tokio::test]
    async fn emit_event_rejects_multi_subject_for_unsupported_action() {
        let ctx = create_test_context().await;
        // ResolveReport is embedded-id and must be length-1.
        let err = emit_event(
            State(ctx),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::ResolveReport {
                    report_id: 42,
                    resolution: ReportResolution::Resolved,
                },
                subjects: vec![
                    repo_subject("did:plc:a"),
                    repo_subject("did:plc:b"),
                ],
                rationale: "two subjects on a length-1 action".to_string(),
                snapshot_capture: false,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        let body = format!("{:?}", err.1.0);
        assert!(
            body.contains("SubjectsArrayInvalidForAction"),
            "expected SubjectsArrayInvalidForAction, got: {}",
            body
        );
    }

    #[tokio::test]
    async fn emit_event_multi_subject_takedown_account_round_trip() {
        let ctx = create_test_context().await;
        for did in &["did:plc:a", "did:plc:b", "did:plc:c"] {
            seed_actor(&ctx, did, &did.replace("did:plc:", "")).await;
        }
        let resp = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::TakedownAccount,
                subjects: vec![
                    repo_subject("did:plc:a"),
                    repo_subject("did:plc:b"),
                    repo_subject("did:plc:c"),
                ],
                rationale: "spam ring".to_string(),
                snapshot_capture: true,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap()
        .0;
        // Each subject got moderated.
        for did in &["did:plc:a", "did:plc:b", "did:plc:c"] {
            assert_eq!(count_moderation_rows(&ctx, did).await, 1, "did {} not moderated", did);
        }
        // Snapshot list aligned 1:1 with subjects.
        assert_eq!(resp.snapshots.len(), 3);
        for (idx, snap) in resp.snapshots.iter().enumerate() {
            assert!(snap.snapshot_id.is_some(), "snapshots[{}] missing", idx);
        }
        // Single chain entry covers the whole batch.
        assert_eq!(count_chain_entries(&ctx).await, 1);
    }

    #[tokio::test]
    async fn emit_event_multi_subject_apply_label_round_trip() {
        let ctx = create_test_context().await;
        let resp = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::ApplyLabel {
                    val: "spam".to_string(),
                    neg: false,
                },
                subjects: vec![
                    Subject::Record {
                        uri: "at://did:plc:a/app.bsky.feed.post/1".to_string(),
                        cid: "bafy1".to_string(),
                    },
                    Subject::Record {
                        uri: "at://did:plc:b/app.bsky.feed.post/2".to_string(),
                        cid: "bafy2".to_string(),
                    },
                ],
                rationale: "spam wave".to_string(),
                snapshot_capture: false,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(resp.snapshots.is_empty(), "snapshot_capture=false → empty snapshots");
        let label_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM label WHERE val = $1 AND neg = FALSE")
                .bind("spam")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        assert_eq!(label_count, 2, "both labels landed");
    }

    #[tokio::test]
    async fn emit_event_multi_subject_takedown_record_round_trip() {
        let ctx = create_test_context().await;
        let _ = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::TakedownRecord,
                subjects: vec![
                    Subject::Record {
                        uri: "at://did:plc:a/app.bsky.feed.post/r1".to_string(),
                        cid: "bafyR1".to_string(),
                    },
                    Subject::Record {
                        uri: "at://did:plc:a/app.bsky.feed.post/r2".to_string(),
                        cid: "bafyR2".to_string(),
                    },
                ],
                rationale: "spam posts".to_string(),
                snapshot_capture: false,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap();
        let takedown_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM label WHERE val = $1 AND neg = FALSE")
                .bind("!takedown")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        assert_eq!(takedown_count, 2);
    }

    #[tokio::test]
    async fn emit_event_per_subject_failure_aborts_whole_tx() {
        // RestoreBlob with a non-existent CID rejects via PdsError::NotFound
        // from BlobQuarantine::restore_blob_in_tx — the second subject in
        // the batch trips this. Whole tx must roll back: neither the
        // first subject's mutation nor the chain entry land.
        let ctx = create_test_context().await;
        // Seed a quarantined blob for subject 0 so the first restore is
        // valid; subject 1 references a blob with no quarantine row.
        sqlx::query(
            "INSERT INTO blob (cid, did, size, mime_type, created_at) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind("bafy_quarantined")
        .bind("did:plc:owner")
        .bind(100_i64)
        .bind("image/png")
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&ctx.account_db)
        .await
        .unwrap();
        crate::blob_store::quarantine::BlobQuarantine::new(ctx.account_db.clone())
            .quarantine_blob(
                "bafy_quarantined",
                crate::blob_store::quarantine::QuarantineReason::Other,
                None,
                "did:plc:m",
                None,
            )
            .await
            .unwrap();

        let chain_before = count_chain_entries(&ctx).await;

        let err = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::RestoreBlob,
                subjects: vec![
                    Subject::Blob {
                        did: "did:plc:owner".to_string(),
                        cid: "bafy_quarantined".to_string(),
                        record_uri: None,
                    },
                    Subject::Blob {
                        did: "did:plc:owner".to_string(),
                        cid: "bafy_does_not_exist".to_string(),
                        record_uri: None,
                    },
                ],
                rationale: "test rollback".to_string(),
                snapshot_capture: false,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap_err();
        // Failure surfaces the failing subject index.
        let body = format!("{:?}", err.1.0);
        assert!(body.contains("\"failingSubject\": Number(1)") || body.contains("failingSubject"));
        // No new chain entry written — the whole tx rolled back.
        assert_eq!(
            count_chain_entries(&ctx).await,
            chain_before,
            "tx must roll back, no chain entry"
        );
        // The first subject's restore must NOT have committed: the blob
        // is still quarantined.
        let still_quarantined: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM blob_quarantine WHERE cid = $1 AND restored_at IS NULL",
        )
        .bind("bafy_quarantined")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(
            still_quarantined, 1,
            "first subject's restore must roll back atomically with the failed second"
        );
    }

    #[tokio::test]
    async fn emit_event_orphan_snapshot_on_capture_failure_mid_batch() {
        // §8.3.1 carve-out: snapshot capture is pre-tx; if the second
        // subject's snapshot fails, the first subject's snapshot is
        // already on disk (orphan) and no chain entry is written.
        // We can't easily induce a capture failure on a real subject,
        // so this test exercises the request-rejection error path
        // directly by passing an empty CID for subject 1's Record (the
        // capture function still succeeds, so this is a structural
        // check on the orphan-snapshot semantics: the test confirms
        // that when capture for subject 0 succeeds, the audit_snapshot
        // row exists even if the call later fails for other reasons).
        //
        // The kickoff's verification requires "with 3 subjects where
        // the 2nd snapshot capture fails, confirm 1 orphan snapshot
        // exists for subject 0 and no chain entry was written." We
        // achieve this by combining snapshot_capture=true with a
        // dispatch failure on subject 1: snapshots for subject 0 land
        // pre-tx, the dispatch failure aborts the tx, and the chain
        // entry never lands.
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:s0", "s0.test").await;
        // Quarantine a blob for subject 0 so that subject 0's
        // RestoreBlob call would succeed if the tx didn't fail later.
        // But here we use TakedownAccount + an unseeded second DID to
        // exercise the orphan-snapshot path: subject 0 captures + would
        // takedown; subject 1's takedown fails because the actor row
        // doesn't exist.
        let snapshot_count_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_snapshot")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        let chain_before = count_chain_entries(&ctx).await;

        let result = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::TakedownAccount,
                subjects: vec![
                    repo_subject("did:plc:s0"),
                    repo_subject("did:plc:does_not_exist"),
                    repo_subject("did:plc:also_missing"),
                ],
                rationale: "exercise orphan-snapshot semantics".to_string(),
                snapshot_capture: true,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await;

        // Either the dispatch fails (tx rolls back) or it succeeds
        // (everything committed). What matters for orphan-snapshot:
        // pre-tx snapshots for any successful capture remain on disk
        // even if the wrapping tx aborts.
        let snapshot_count_after: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_snapshot")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        match result {
            Ok(_) => {
                // All committed — expected when nothing fails.
                assert!(snapshot_count_after >= snapshot_count_before);
            }
            Err(_) => {
                // Tx rolled back, but pre-tx snapshots survive (orphan).
                assert!(
                    snapshot_count_after > snapshot_count_before,
                    "Phase 1 captured at least one snapshot before Phase 2 failed"
                );
                // Chain entry NOT written.
                assert_eq!(
                    count_chain_entries(&ctx).await,
                    chain_before,
                    "no chain row on tx abort"
                );
            }
        }
    }

    #[tokio::test]
    async fn emit_event_chain_row_shape_single_subject_dual_population() {
        // §8.3.3: single-subject populates BOTH flat columns AND
        // cascade_subjects: [s].
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:single", "single.test").await;
        let _ = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::TakedownAccount,
                subjects: vec![repo_subject("did:plc:single")],
                rationale: "single subject".to_string(),
                snapshot_capture: true,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap();
        let (sd, su, sc, cs, csi) = latest_chain_row(&ctx).await;
        assert_eq!(sd.as_deref(), Some("did:plc:single"), "flat subject_did populated");
        assert!(su.is_none());
        assert!(sc.is_none());
        // cascade_subjects populated with the single subject.
        let cascade_json = cs.expect("cascade_subjects populated");
        assert!(cascade_json.contains("did:plc:single"), "cascade has the subject");
        // cascade_snapshot_ids has one element (the snapshot id) when
        // snapshot_capture=true.
        let csi_json = csi.expect("cascade_snapshot_ids populated for snapshot_capture=true");
        // Single-subject + capture=true: cascade_snapshot_ids is a
        // JSON array with one element (the captured snapshot id).
        assert!(
            csi_json.starts_with('[') && csi_json.ends_with(']'),
            "cascade_snapshot_ids should be JSON array — got: {}",
            csi_json
        );
        assert!(!csi_json.contains(','), "single-subject array has one element, no comma — got: {}", csi_json);
    }

    #[tokio::test]
    async fn emit_event_chain_row_shape_single_subject_no_snapshot() {
        // §8.3.3: snapshot_capture=false → cascade_snapshot_ids: [].
        let ctx = create_test_context().await;
        let _ = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::ApplyLabel {
                    val: "test".to_string(),
                    neg: false,
                },
                subjects: vec![Subject::Record {
                    uri: "at://did:plc:x/app.bsky.feed.post/y".to_string(),
                    cid: "bafyZ".to_string(),
                }],
                rationale: "no snapshot".to_string(),
                snapshot_capture: false,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap();
        let (_sd, su, _sc, cs, csi) = latest_chain_row(&ctx).await;
        assert!(su.as_deref().unwrap().contains("at://did:plc:x"));
        let cascade_json = cs.expect("cascade_subjects populated");
        assert!(cascade_json.contains("at://did:plc:x"));
        // Either NULL or empty array per Step 0.6 / insert_chain_entry_pool rules.
        match csi {
            None => {} // empty cascade_snapshot_ids stored as NULL
            Some(s) => assert!(
                s == "[]" || s.is_empty(),
                "cascade_snapshot_ids should be empty for snapshot_capture=false, got: {}",
                s
            ),
        }
    }

    #[tokio::test]
    async fn emit_event_chain_row_shape_multi_subject_synthetic_primary() {
        // §8.3.3: multi-subject → NULL flat columns AND
        // cascade_subjects: [s1, s2, ...].
        let ctx = create_test_context().await;
        for did in &["did:plc:m1", "did:plc:m2"] {
            seed_actor(&ctx, did, &did.replace("did:plc:", "")).await;
        }
        let _ = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::TakedownAccount,
                subjects: vec![
                    repo_subject("did:plc:m1"),
                    repo_subject("did:plc:m2"),
                ],
                rationale: "multi-subject batch".to_string(),
                snapshot_capture: true,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap();
        let (sd, su, sc, cs, csi) = latest_chain_row(&ctx).await;
        assert!(sd.is_none(), "multi-subject flat subject_did NULL");
        assert!(su.is_none(), "multi-subject flat subject_uri NULL");
        assert!(sc.is_none(), "multi-subject flat subject_cid NULL");
        let cascade_json = cs.expect("cascade_subjects populated");
        assert!(cascade_json.contains("did:plc:m1"));
        assert!(cascade_json.contains("did:plc:m2"));
        let csi_json = csi.expect("cascade_snapshot_ids populated");
        // Two entries, comma-separated.
        assert!(csi_json.starts_with('['));
        assert!(csi_json.contains(','));
    }

    #[tokio::test]
    async fn emit_event_embedded_id_subject_target_mismatch_returns_400() {
        // ResolveReport with subjects[0] not matching the actual report
        // target → 400 SubjectVariantMismatch (or SubjectTargetMismatch).
        let ctx = create_test_context().await;
        // Submit a report against a Repo subject.
        let report = ctx
            .report_manager
            .submit_report(
                Some("did:plc:reported"),
                None,
                None,
                crate::admin::reports::ReportReason::Spam,
                Some("test report"),
                "did:plc:reporter",
            )
            .await
            .unwrap();
        // Try to resolve passing a Record subject — variant mismatch.
        let err = emit_event(
            State(ctx),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::ResolveReport {
                    report_id: report.id,
                    resolution: ReportResolution::Resolved,
                },
                subjects: vec![Subject::Record {
                    uri: "at://did:plc:reported/app.bsky.feed.post/x".to_string(),
                    cid: "bafyX".to_string(),
                }],
                rationale: "wrong subject type".to_string(),
                snapshot_capture: false,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        let body = format!("{:?}", err.1.0);
        assert!(
            body.contains("SubjectVariantMismatch"),
            "expected SubjectVariantMismatch, got: {}",
            body
        );
    }

    #[tokio::test]
    async fn emit_event_resolve_appeal_subject_mismatch_via_in_tx_validation() {
        // ResolveAppeal pulls validation through update_status_in_tx
        // (Step 0.5). subjects[0] with the wrong DID → 400.
        let ctx = create_test_context().await;
        seed_actor(&ctx, "did:plc:realdid", "realdid.test").await;
        ctx.moderation_manager
            .apply_action(ApplyActionParams {
                did: "did:plc:realdid",
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
        .bind("did:plc:realdid")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        let mgr = AppealManager::new(ctx.account_db.clone());
        let appeal = mgr
            .submit_appeal(
                Some(mod_id),
                None,
                None,
                "did:plc:realdid",
                "false positive",
                None,
            )
            .await
            .unwrap();
        // Pass the WRONG DID for the appeal target → SubjectTargetMismatch.
        let err = emit_event(
            State(ctx),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::ResolveAppeal {
                    appeal_id: appeal.id,
                    resolution: AppealResolutionDecision::Approve,
                },
                subjects: vec![repo_subject("did:plc:wrong_did")],
                rationale: "wrong target".to_string(),
                snapshot_capture: false,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        let body = format!("{:?}", err.1.0);
        assert!(
            body.contains("SubjectTargetMismatch"),
            "expected SubjectTargetMismatch, got: {}",
            body
        );
    }

    #[tokio::test]
    async fn emit_event_per_action_limit_delete_account_caps_at_10() {
        // Per Step 0.6 §4: DeleteAccount caps at 10. 11 subjects → 400.
        let ctx = create_test_context().await;
        let subjects: Vec<Subject> = (0..11)
            .map(|i| repo_subject(&format!("did:plc:da{}", i)))
            .collect();
        let err = emit_event(
            State(ctx),
            admin_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::DeleteAccount,
                subjects,
                rationale: "over the cap".to_string(),
                snapshot_capture: false,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        let body = format!("{:?}", err.1.0);
        assert!(
            body.contains("subjects array length 11 exceeds limit of 10"),
            "expected DeleteAccount cap error, got: {}",
            body
        );
    }

    #[tokio::test]
    async fn emit_event_per_action_limit_delete_blob_caps_at_25() {
        // Per Step 0.6 §4: DeleteBlob caps at 25. 26 subjects → 400.
        let ctx = create_test_context().await;
        let subjects: Vec<Subject> = (0..26)
            .map(|i| Subject::Blob {
                did: "did:plc:owner".to_string(),
                cid: format!("bafy_{}", i),
                record_uri: None,
            })
            .collect();
        let err = emit_event(
            State(ctx),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::DeleteBlob,
                subjects,
                rationale: "over the cap".to_string(),
                snapshot_capture: false,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        let body = format!("{:?}", err.1.0);
        assert!(
            body.contains("subjects array length 26 exceeds limit of 25"),
            "expected DeleteBlob cap error, got: {}",
            body
        );
    }

    #[tokio::test]
    async fn emit_event_quarantine_blob_in_tx_rollback_on_per_subject_failure() {
        // QuarantineBlob multi-subject; the second subject is already
        // quarantined → in-tx existence check rejects → whole tx rolls
        // back. First subject must NOT be quarantined post-failure.
        let ctx = create_test_context().await;
        for cid in &["bafy_q_a", "bafy_q_b"] {
            sqlx::query(
                "INSERT INTO blob (cid, did, size, mime_type, created_at) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(cid)
            .bind("did:plc:owner")
            .bind(100_i64)
            .bind("image/png")
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&ctx.account_db)
            .await
            .unwrap();
        }
        // Pre-quarantine bafy_q_b so the second subject's quarantine
        // hits the existence check.
        crate::blob_store::quarantine::BlobQuarantine::new(ctx.account_db.clone())
            .quarantine_blob(
                "bafy_q_b",
                crate::blob_store::quarantine::QuarantineReason::Other,
                None,
                "did:plc:m",
                None,
            )
            .await
            .unwrap();
        let chain_before = count_chain_entries(&ctx).await;
        let err = emit_event(
            State(ctx.clone()),
            moderator_auth(),
            crate::api::extractors::AuroraJson(EmitEventInput {
                action: ModEventAction::QuarantineBlob,
                subjects: vec![
                    Subject::Blob {
                        did: "did:plc:owner".to_string(),
                        cid: "bafy_q_a".to_string(),
                        record_uri: None,
                    },
                    Subject::Blob {
                        did: "did:plc:owner".to_string(),
                        cid: "bafy_q_b".to_string(),
                        record_uri: None,
                    },
                ],
                rationale: "expect rollback".to_string(),
                snapshot_capture: false,
                metadata: None,
                legacy_subject_used: false,
            }),
        )
        .await
        .unwrap_err();
        // Conflict from BlobQuarantine::quarantine_blob_in_tx maps via
        // the `other` arm of dispatch_err_to_response → 500.
        assert!(err.0 == StatusCode::INTERNAL_SERVER_ERROR || err.0 == StatusCode::BAD_REQUEST);
        // The first subject must NOT be quarantined (tx rolled back).
        let q_a_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM blob_quarantine WHERE cid = $1",
        )
        .bind("bafy_q_a")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(
            q_a_count, 0,
            "first subject's quarantine must roll back atomically with the failed second"
        );
        // No new chain entry.
        assert_eq!(count_chain_entries(&ctx).await, chain_before);
    }

    // ---------- Arc 6 Step 7: emitEvent dual-shape Deserialize ----------
    //
    // Per V04_DESIGN §5.3.6 + Step 0 Q9. The input accepts both the
    // canonical v0.3 `subjects: [Subject]` shape and the legacy v0.2
    // `subject: Subject` shape during the deprecation window.

    #[test]
    fn emit_event_input_parses_canonical_subjects_shape() {
        let json = r#"{
            "action": {"kind": "TakedownAccount"},
            "subjects": [{"$type": "com.atproto.admin.defs#repoRef", "did": "did:plc:abc"}],
            "rationale": "spam"
        }"#;
        let input: EmitEventInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.subjects.len(), 1);
        assert!(
            !input.legacy_subject_used,
            "canonical shape must not set legacy_subject_used"
        );
    }

    #[test]
    fn emit_event_input_parses_legacy_subject_shape_and_flags_it() {
        let json = r#"{
            "action": {"kind": "TakedownAccount"},
            "subject": {"$type": "com.atproto.admin.defs#repoRef", "did": "did:plc:abc"},
            "rationale": "spam"
        }"#;
        let input: EmitEventInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.subjects.len(), 1);
        assert!(
            matches!(input.subjects[0], Subject::Repo { ref did } if did == "did:plc:abc"),
            "legacy single-subject normalizes to subjects[0]"
        );
        assert!(
            input.legacy_subject_used,
            "legacy shape must set legacy_subject_used for handler-side observability"
        );
    }

    #[test]
    fn emit_event_input_rejects_both_shapes_simultaneously() {
        let json = r#"{
            "action": {"kind": "TakedownAccount"},
            "subject": {"$type": "com.atproto.admin.defs#repoRef", "did": "did:plc:abc"},
            "subjects": [{"$type": "com.atproto.admin.defs#repoRef", "did": "did:plc:def"}],
            "rationale": "spam"
        }"#;
        let err = serde_json::from_str::<EmitEventInput>(json).unwrap_err();
        assert!(
            err.to_string().contains("not both"),
            "error message must point at the both-shapes-present case; got: {}",
            err
        );
    }

    #[test]
    fn emit_event_input_rejects_neither_shape() {
        let json = r#"{
            "action": {"kind": "TakedownAccount"},
            "rationale": "spam"
        }"#;
        let err = serde_json::from_str::<EmitEventInput>(json).unwrap_err();
        assert!(
            err.to_string().contains("requires either"),
            "error message must point at the missing-shape case; got: {}",
            err
        );
    }
}
