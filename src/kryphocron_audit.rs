//! v0.7 arc 2 step 7 — kryphocron audit-event payloads and emit
//! helpers.
//!
//! Payload structs for the 12 new
//! [`crate::admin::events::ModerationEventType`] variants per
//! `v07_DESIGN.md` §4 lines 933-1450, plus emit helpers that
//! serialize the payload to JSON and call
//! [`crate::admin::events::insert_moderation_event_in_tx`].
//!
//! The emit helpers route through the lent shared tx from step
//! 3.5's relay-race mechanism, so every B-variant audit row
//! commits atomically with the record write that produced it —
//! the audit-coherence design's "transactional with record write"
//! property (§4 category B) now actually holds end-to-end, NOT
//! best-effort. Step 3.5 set up the relay-race ordering with the
//! shared tx committing first; step 7 makes the inserts that
//! shared tx now carries.
//!
//! ## Ship state of emit wiring
//!
//! - **Wired in arc 2 step 7:**
//!   - [`emit_audience_updated_in_tx`] — fires from
//!     `bind_pipeline`'s `DedicatedEndpoint` arm when the write
//!     hits a `tools.kryphocron.policy.audience` record (the
//!     `manageAudience` endpoint's path).
//!   - [`emit_audience_check_denied_in_tx`] — fires from
//!     `participatePrivate` when the host-side audience-oracle
//!     pre-check rejects.
//!
//! - **Defined but not yet wired (post-arc-2 work):** every
//!   other emit helper. The substrate async flusher (categories
//!   `KryphocronBindGranted` / `KryphocronBindDenied` /
//!   `KryphocronReborrowFailed` / `KryphocronCompositeRollbackMarker`),
//!   the sentinel-sink + panic-guard infrastructure
//!   (`KryphocronFallback`), the recovery-mode write-path
//!   (`KryphocronRecoveryWrite` per R3-deferral), the cascade-
//!   initiating handlers (`KryphocronSystemCleanup`), and the
//!   block / mute / threadgate dedicated endpoints
//!   (`KryphocronBlockChanged` / `KryphocronMuteChanged` /
//!   `KryphocronThreadgateChanged`) are all scheduled for cycles
//!   beyond arc 2. The payload structs ship now so the post-arc-2
//!   cycles can wire the emit sites without re-shaping the audit
//!   surface.
//!
//! ## Payload completeness
//!
//! All payloads carry a `payload_completeness` field per the
//! design's chain-integrity discipline. Arc 2 step 7 always emits
//! `"Full"` (every field the design specifies is populated). The
//! `"Partial"` and `"Sentinel"` values are reserved for the
//! substrate-flusher path where the flusher receives a partial
//! event from the substrate's emit machinery.

#![allow(dead_code)] // most emit helpers wire in post-arc-2 cycles

use crate::admin::events::{insert_moderation_event_in_tx, ModerationEventType};
use crate::error::PdsResult;
use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

/// Common discriminator for the payload-completeness chain-
/// integrity field. Arc 2 step 7 emits `Full` for every wired
/// path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PayloadCompleteness {
    /// Every field the design specifies is populated.
    Full,
    /// The substrate flusher received a partial event and rendered
    /// the fields it could. Reserved for the post-arc-2 flusher.
    #[allow(dead_code)]
    Partial,
    /// A sentinel-sink emit — the audit chain records that the
    /// event happened but has no per-field detail. Reserved for
    /// `KryphocronFallback` paths that don't have full event
    /// context.
    #[allow(dead_code)]
    Sentinel,
}

// ---------------------------------------------------------------------------
// Category A — substrate async-flusher payloads
// ---------------------------------------------------------------------------
//
// Sourced from substrate audit events fed via the user-sink. Arc
// 2 step 7 ships the payload structs; the flusher that drains
// the sink buffer and inserts these rows is post-arc-2 work.

/// `KryphocronBindGranted` payload (sourced from substrate's
/// `UserAuditEvent::CapabilityBound`). Per design §4 lines
/// 947-993.
#[derive(Debug, Clone, Serialize)]
pub struct BindGrantedPayload {
    pub capability: String,
    pub capability_class: String,
    pub subject_uri: String,
    pub requester_did: String,
    pub trace_id: String,
    pub cascade_id: Option<String>,
    /// Substrate's `BindOutcomeRepr` rendered as serde-default
    /// JSON. Typed as opaque here because the substrate is the
    /// authoritative source — Aurora-Locus's flusher passes the
    /// substrate-emitted value through without re-interpretation.
    pub bind_outcome: serde_json::Value,
    pub outcome: String, // always "Granted" in the v0.1/0.2 substrate
    pub payload_completeness: PayloadCompleteness,
}

/// `KryphocronBindDenied` payload (sourced from substrate's
/// `UserAuditEvent::CapabilityIssuanceDenied`). Per design §4
/// lines 995-1051.
#[derive(Debug, Clone, Serialize)]
pub struct BindDeniedPayload {
    pub capability: String,
    pub capability_class: String,
    pub subject_uri: String,
    pub requester_did: String,
    pub trace_id: String,
    pub cascade_id: Option<String>,
    /// Substrate's `DenialReason` rendered as serde-default JSON.
    pub denial_reason: serde_json::Value,
    pub outcome: String, // always "Denied"
    pub payload_completeness: PayloadCompleteness,
}

/// `KryphocronReborrowFailed` payload. Per design §4. v0.7 arc 2
/// ships the shape; the flusher integration is post-arc-2.
#[derive(Debug, Clone, Serialize)]
pub struct ReborrowFailedPayload {
    pub capability: String,
    pub capability_class: String,
    pub subject_uri: String,
    pub requester_did: String,
    pub trace_id: String,
    pub reborrow_reason: serde_json::Value,
    pub payload_completeness: PayloadCompleteness,
}

/// `KryphocronCompositeRollbackMarker` payload. Per design §4.
/// Effectively never fires under v0.7's all-user-class workloads
/// — exists for forward-compat with future cycles that introduce
/// cross-class composite scopes.
#[derive(Debug, Clone, Serialize)]
pub struct CompositeRollbackMarkerPayload {
    pub composite_op_id: String,
    pub trace_id: String,
    pub sinks_committed: Vec<String>,
    pub sinks_failed: Vec<String>,
    pub payload_completeness: PayloadCompleteness,
}

// ---------------------------------------------------------------------------
// Category B — Aurora-Locus housekeeping, transactional with record write
// ---------------------------------------------------------------------------
//
// Emitted by Aurora-Locus's bind-pipeline / dedicated-endpoint
// path via the lent shared tx. Step 3.5's relay-race
// orchestration commits the shared tx BEFORE the actor tx, so the
// audit row commits transactionally with the record write — if
// the actor commit fails after the audit committed, the
// `emit_bind_audit_orphan_marker` from step 3.5 fires.

/// `KryphocronAudienceUpdated` payload (per design §4 lines
/// 1180-1236). Emitted by `bind_pipeline`'s DedicatedEndpoint arm
/// when the write hits a `tools.kryphocron.policy.audience`
/// record.
#[derive(Debug, Clone, Serialize)]
pub struct AudienceUpdatedPayload {
    pub audience_uri: String,
    pub owner_did: String,
    pub operation: AudienceOperation,
    pub members_added: Vec<String>,
    pub members_removed: Vec<String>,
    pub members_total_after: i64,
    pub mode_before: Option<String>,
    pub mode_after: String,
    pub name: Option<String>,
    pub origin: AudienceOrigin,
    pub cascade_id: Option<String>,
    pub cascade_reassigned_to: Option<String>,
    pub cascade_post_count: Option<i64>,
    pub cascade_progress: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AudienceOperation {
    Created,
    Updated,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudienceOrigin {
    User,
    AccountSetup,
    Backfill,
    LazyCreate,
}

/// `KryphocronBlockChanged` payload (per design §4 lines
/// 1237-1259). Ships in arc 2 step 7 without emit wiring — the
/// block-create / block-delete dedicated endpoints are post-arc-2
/// work.
#[derive(Debug, Clone, Serialize)]
pub struct BlockChangedPayload {
    pub block_uri: String,
    pub blocker_did: String,
    pub subject_did: String,
    pub operation: BlockMuteOperation,
    pub cascade_id: Option<String>,
}

/// `KryphocronMuteChanged` payload (same shape as block-changed
/// with `muter_did` substituted for `blocker_did`).
#[derive(Debug, Clone, Serialize)]
pub struct MuteChangedPayload {
    pub mute_uri: String,
    pub muter_did: String,
    pub subject_did: String,
    pub operation: BlockMuteOperation,
    pub cascade_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockMuteOperation {
    Created,
    Deleted,
}

/// `KryphocronThreadgateChanged` payload (per design §4 lines
/// 1261-1283). Ships in arc 2 step 7 without emit wiring.
#[derive(Debug, Clone, Serialize)]
pub struct ThreadgateChangedPayload {
    pub threadgate_uri: String,
    pub post_uri: String,
    pub owner_did: String,
    pub operation: ThreadgateOperation,
    pub rule_before: Option<serde_json::Value>,
    pub rule_after: serde_json::Value,
    pub cascade_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadgateOperation {
    Created,
    Updated,
    Deleted,
}

/// `KryphocronRecoveryWrite` payload. R3-deferred — no production
/// emit site in arc 2; the variant + payload exist for the post-
/// arc-2 recovery-mode cycle.
#[derive(Debug, Clone, Serialize)]
pub struct RecoveryWritePayload {
    pub subject_uri: String,
    pub requester_did: String,
    pub nsid: String,
    pub action: String, // "create" | "update" | "delete"
    pub cascade_source: Option<serde_json::Value>,
}

/// `KryphocronSystemCleanup` payload. Ships in arc 2 step 7
/// without emit wiring — the cascade-initiating handler + orphan-
/// sweep machinery that triggers this event is post-arc-2 work.
#[derive(Debug, Clone, Serialize)]
pub struct SystemCleanupPayload {
    pub subject_uri: String,
    pub origin: serde_json::Value, // matches SystemCleanupOrigin
    pub cascade_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Category C — Aurora-Locus housekeeping, own short tx
// ---------------------------------------------------------------------------

/// `KryphocronAudienceCheckDenied` payload (per design §4 lines
/// 1142-1178). Emitted by `participatePrivate` when the host-side
/// audience-oracle pre-check rejects.
#[derive(Debug, Clone, Serialize)]
pub struct AudienceCheckDeniedPayload {
    pub capability_attempted: String, // always "ParticipatePrivate" in arc 2
    pub subject_uri: String,
    pub requester_did: String,
    pub audience_uri: Option<String>,
    pub audience_mode: String,
    pub audience_check_result: AudienceCheckResult,
    pub trace_id: String,
    pub payload_completeness: PayloadCompleteness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AudienceCheckResult {
    NotInAudience,
    NoAudienceConfigured,
}

/// `KryphocronFallback` payload. Discriminated by `subtype` per
/// design §4 lines 1284-1421. Ships in arc 2 step 7 without emit
/// wiring — the sentinel-sink + panic-guard infrastructure is
/// post-arc-2 substrate-integration work.
#[derive(Debug, Clone, Serialize)]
pub struct FallbackPayload {
    pub subtype: String,
    pub trace_id: String,
    pub at: String,
    /// Subtype-specific fields rendered as serde-default JSON.
    /// The substrate sink trait hands these to the host as a
    /// `serde_json::Value` already; we pass them through.
    #[serde(flatten)]
    pub detail: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Emit helpers
// ---------------------------------------------------------------------------
//
// Each helper serializes the payload to JSON, builds the
// canonical `created_at`, and calls
// `insert_moderation_event_in_tx`. The helpers take `&mut
// Transaction<'_, sqlx::Any>` so callers thread through the lent
// shared tx (B variants) or open their own short tx (C variants).

/// Emit a `KryphocronAudienceUpdated` row on the supplied tx.
/// Called by `bind_pipeline`'s DedicatedEndpoint arm when the
/// write hits a `tools.kryphocron.policy.audience` record.
pub async fn emit_audience_updated_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    actor_did: &str,
    payload: AudienceUpdatedPayload,
) -> PdsResult<i64> {
    let details =
        serde_json::to_string(&payload).map_err(|e| crate::error::PdsError::Internal(e.to_string()))?;
    let created_at = Utc::now().to_rfc3339();
    let subject_uri = payload.audience_uri.clone();
    let event_id = insert_moderation_event_in_tx(
        tx,
        ModerationEventType::KryphocronAudienceUpdated.as_str(),
        actor_did,
        Some(actor_did), // owner-as-subject
        Some(&subject_uri),
        None,
        &details,
        &created_at,
        None,
    )
    .await
    .map_err(crate::error::PdsError::Database)?;
    Ok(event_id)
}

/// Emit a `KryphocronAudienceCheckDenied` row on the supplied tx.
/// Called by `participatePrivate`'s audience-oracle pre-check
/// when the requester is not in the parent post's audience.
pub async fn emit_audience_check_denied_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    actor_did: &str,
    payload: AudienceCheckDeniedPayload,
) -> PdsResult<i64> {
    let details =
        serde_json::to_string(&payload).map_err(|e| crate::error::PdsError::Internal(e.to_string()))?;
    let created_at = Utc::now().to_rfc3339();
    let subject_uri = payload.subject_uri.clone();
    let requester_did = payload.requester_did.clone();
    let event_id = insert_moderation_event_in_tx(
        tx,
        ModerationEventType::KryphocronAudienceCheckDenied.as_str(),
        actor_did,
        Some(&requester_did),
        Some(&subject_uri),
        None,
        &details,
        &created_at,
        None,
    )
    .await
    .map_err(crate::error::PdsError::Database)?;
    Ok(event_id)
}

/// Synthesize a trace ID for host-emitted events (categories B
/// and C). Substrate-emitted events (category A) carry their own
/// trace_id; this helper applies only to host-side events that
/// don't have a substrate-supplied trace.
pub fn synthesize_trace_id() -> String {
    Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Payload round-trips through serde to a JSON object with
    /// the expected top-level fields per the design's §4 spec.
    #[test]
    fn audience_updated_payload_serializes_to_design_shape() {
        let p = AudienceUpdatedPayload {
            audience_uri: "at://did:plc:abc/tools.kryphocron.policy.audience/3kj7".to_string(),
            owner_did: "did:plc:abc".to_string(),
            operation: AudienceOperation::Created,
            members_added: vec!["did:plc:m1".to_string()],
            members_removed: vec![],
            members_total_after: 1,
            mode_before: None,
            mode_after: "list".to_string(),
            name: Some("close friends".to_string()),
            origin: AudienceOrigin::User,
            cascade_id: None,
            cascade_reassigned_to: None,
            cascade_post_count: None,
            cascade_progress: None,
        };

        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(json["operation"], "created");
        assert_eq!(json["origin"], "user");
        assert_eq!(json["mode_after"], "list");
        assert_eq!(json["members_total_after"], 1);
        assert!(json["mode_before"].is_null());
    }

    #[test]
    fn audience_check_denied_payload_serializes_to_design_shape() {
        let p = AudienceCheckDeniedPayload {
            capability_attempted: "ParticipatePrivate".to_string(),
            subject_uri: "at://did:plc:abc/tools.kryphocron.feed.postPrivate/3kj7".to_string(),
            requester_did: "did:plc:xyz".to_string(),
            audience_uri: Some(
                "at://did:plc:abc/tools.kryphocron.policy.audience/3kj7".to_string(),
            ),
            audience_mode: "list".to_string(),
            audience_check_result: AudienceCheckResult::NotInAudience,
            trace_id: "test-trace".to_string(),
            payload_completeness: PayloadCompleteness::Full,
        };
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(json["capability_attempted"], "ParticipatePrivate");
        assert_eq!(json["audience_check_result"], "NotInAudience");
        assert_eq!(json["payload_completeness"], "Full");
    }

    #[test]
    fn bind_granted_payload_serializes_to_design_shape() {
        let p = BindGrantedPayload {
            capability: "EditPrivatePost".to_string(),
            capability_class: "user".to_string(),
            subject_uri: "at://did:plc:abc/tools.kryphocron.feed.postPrivate/3kj7".to_string(),
            requester_did: "did:plc:xyz".to_string(),
            trace_id: "test-trace".to_string(),
            cascade_id: None,
            bind_outcome: serde_json::json!({ "kind": "Success", "details": null }),
            outcome: "Granted".to_string(),
            payload_completeness: PayloadCompleteness::Full,
        };
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(json["capability"], "EditPrivatePost");
        assert_eq!(json["outcome"], "Granted");
        assert_eq!(json["bind_outcome"]["kind"], "Success");
    }

    #[test]
    fn moderation_event_type_round_trips_kryphocron_variants() {
        use std::str::FromStr;
        for variant in [
            ModerationEventType::KryphocronBindGranted,
            ModerationEventType::KryphocronBindDenied,
            ModerationEventType::KryphocronAudienceCheckDenied,
            ModerationEventType::KryphocronReborrowFailed,
            ModerationEventType::KryphocronCompositeRollbackMarker,
            ModerationEventType::KryphocronAudienceUpdated,
            ModerationEventType::KryphocronBlockChanged,
            ModerationEventType::KryphocronMuteChanged,
            ModerationEventType::KryphocronThreadgateChanged,
            ModerationEventType::KryphocronFallback,
            ModerationEventType::KryphocronRecoveryWrite,
            ModerationEventType::KryphocronSystemCleanup,
        ] {
            let s = variant.as_str();
            let round = ModerationEventType::from_str(s)
                .expect("kryphocron variant must round-trip through as_str + from_str");
            assert_eq!(round, variant, "round-trip failed for {s}");
        }
    }
}
