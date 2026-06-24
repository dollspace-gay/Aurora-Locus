/// Admin API Endpoints
/// Implements com.atproto.admin.* endpoints for server administration
use crate::{
    admin::{
        audit_chain::{self, AppendEntryParams},
        defs::{PaginationParams, Subject},
        operator_session::SessionCursor,
        InviteCode, OperatorSessionStore,
    },
    api::registry::{aurora_route_builder, CapsBuilder, Family, RouteRegistry},
    auth::AdminAuthContext,
    error::{PdsError, PdsResult},
    AppContext,
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::Duration;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Build admin API routes.
///
/// Two namespaces are mounted here:
///
/// - `com.atproto.admin.*` — moderation/admin-tier endpoints. After
///   Phase 2.4 (chainlink #85) this surface is exactly the
///   bsky-PDS-2025-Q1 parity baseline plus the parity gaps closed in
///   Phase 1; operator/infrastructure endpoints have been removed.
///
/// - `tools.aurora.ops.*` — operator/infrastructure tier (chainlink #84).
///   30 relocated endpoints from the legacy admin namespace plus 2
///   net-new ones (`listAccounts`, `getInstanceMetrics`). Scope-checked
///   to `atproto:admin.server` via the namespace middleware (e9b66b9).
///
/// `listRecentEvents` stays at `com.atproto.admin.*` — moderation-
/// flavored stream review, not operator infrastructure. Phase 3
/// considered relocating it under `tools.aurora.moderator.*` alongside
/// the other moderator-tier reads (`queryEvents`, `queryStatuses`),
/// but kept the legacy at:// path as the cleaner choice for the
/// streaming review surface: existing parity-tier consumers continue
/// reaching it without an NSID rename, and the richer per-event reads
/// that benefit from the new `tools.aurora.moderator.*` shape ship
/// there alongside the unrelocated stream.
/// Build an axum-friendly `(StatusCode, Json<serde_json::Value>)`
/// error pair carrying the `{error, message}` envelope the
/// federation/admin wire contract uses. Mirrors the `forbidden()`
/// pattern at [`aurora_admin.rs`](src/api/aurora_admin.rs) but
/// without the AuroraAdminError code-table coupling — accepts any
/// `&str` code so the call sites can name their own
/// wire-error variant. v0.6 batch tail G1.1 introduced this for
/// the grant_role/revoke_role reshape; available to any other
/// handler in this module that wants structured-error responses.
fn json_error(
    status: StatusCode,
    code: &str,
    message: impl Into<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    (
        status,
        Json(serde_json::json!({
            "error": code,
            "message": message.into(),
        })),
    )
}

pub fn routes() -> (Router<AppContext>, Arc<RouteRegistry>) {
    // Arc 8 Step 2 (chainlink #54): registration sites for the
    // four `tools.aurora.<family>.*` namespaces flow through
    // `aurora_route_builder()` so each admin-tier route emits a
    // `RouteEntry` alongside its `axum::Router` registration.
    // Non-admin-tier routes (`com.atproto.admin.*` and
    // `tools.aurora.describeCapabilities`) use the builder's
    // pass-through `.route(...)` — they live in the same router
    // but contribute zero registry entries (Step 0 Q6 List C).
    //
    // Extension attribution policy: each capability extension is
    // attributed to a single canonical endpoint (the
    // capability-introducing route for that phase). The wire
    // ordering is independent of per-route order — see
    // `crate::api::registry::WIRE_EXTENSION_ORDER` for the
    // declaration spec and the rationale.
    aurora_route_builder::<AppContext>()
        // ---- com.atproto.admin.* (moderation/admin tier) ----
        //
        // Step 0 Q6 List C: out-of-Aurora-scope namespace. These
        // routes register on the same Router but don't contribute
        // to `tools.aurora.describeCapabilities`.

        // Account read
        .route("/xrpc/com.atproto.admin.getUsers", get(get_users))
        // listAccounts here is the bsky-PDS-compat alias to getUsers; the
        // operator-flavored listAccounts (broader filters) lives at
        // /xrpc/tools.aurora.ops.listAccounts.
        .route("/xrpc/com.atproto.admin.listAccounts", get(get_users))
        .route("/xrpc/com.atproto.admin.getAccount", get(get_account))
        .route(
            "/xrpc/com.atproto.admin.searchAccounts",
            get(search_accounts),
        )
        .route(
            "/xrpc/com.atproto.admin.getAccountInfo",
            get(get_account_info),
        )
        .route(
            "/xrpc/com.atproto.admin.getAccountInfos",
            get(get_account_infos),
        )
        // Subject status (cross-cutting moderation surface)
        .route(
            "/xrpc/com.atproto.admin.updateSubjectStatus",
            post(update_subject_status),
        )
        .route(
            "/xrpc/com.atproto.admin.getSubjectStatus",
            get(get_subject_status),
        )
        // Invite codes
        .route(
            "/xrpc/com.atproto.admin.createInviteCode",
            post(create_invite_code),
        )
        .route(
            "/xrpc/com.atproto.admin.getInviteCodes",
            get(get_invite_codes),
        )
        .route(
            "/xrpc/com.atproto.admin.listInviteCodes",
            get(list_invite_codes),
        )
        .route(
            "/xrpc/com.atproto.admin.disableInviteCode",
            post(disable_invite_code),
        )
        .route(
            "/xrpc/com.atproto.admin.disableInviteCodes",
            post(disable_invite_codes),
        )
        .route(
            "/xrpc/com.atproto.admin.enableAccountInvites",
            post(enable_account_invites),
        )
        .route(
            "/xrpc/com.atproto.admin.disableAccountInvites",
            post(disable_account_invites),
        )
        // Role management
        // grantRole and revokeRole relocated to tools.aurora.superadmin.*
        // in Phase 3.6 (chainlink #103). listRoles stays at the
        // moderation tier — moderators may legitimately need to see
        // who has what role without being SuperAdmin themselves.
        .route("/xrpc/com.atproto.admin.listRoles", get(list_roles))
        // Account management
        .route(
            "/xrpc/com.atproto.admin.updateAccountEmail",
            post(update_account_email),
        )
        .route(
            "/xrpc/com.atproto.admin.updateAccountHandle",
            post(update_account_handle),
        )
        .route(
            "/xrpc/com.atproto.admin.updateAccountPassword",
            post(update_account_password),
        )
        .route(
            "/xrpc/com.atproto.admin.deleteAccount",
            post(admin_delete_account),
        )
        .route(
            "/xrpc/com.atproto.admin.updateAccountSigningKey",
            post(update_account_signing_key),
        )
        // Account moderation
        .route(
            "/xrpc/com.atproto.admin.takedownAccount",
            post(takedown_account),
        )
        .route(
            "/xrpc/com.atproto.admin.suspendAccount",
            post(suspend_account),
        )
        .route(
            "/xrpc/com.atproto.admin.restoreAccount",
            post(restore_account),
        )
        .route(
            "/xrpc/com.atproto.admin.getModerationHistory",
            get(get_moderation_history),
        )
        .route(
            "/xrpc/com.atproto.admin.getModerationQueue",
            get(get_moderation_queue),
        )
        // Labels
        .route("/xrpc/com.atproto.admin.applyLabel", post(apply_label))
        .route("/xrpc/com.atproto.admin.removeLabel", post(remove_label))
        // Reports
        .route("/xrpc/com.atproto.admin.submitReport", post(submit_report))
        .route(
            "/xrpc/com.atproto.admin.updateReportStatus",
            post(update_report_status),
        )
        .route("/xrpc/com.atproto.admin.listReports", get(list_reports))
        // Email
        .route("/xrpc/com.atproto.admin.sendEmail", post(send_email))
        // Audit logs
        .route("/xrpc/com.atproto.admin.getAuditLog", get(get_audit_log))
        // Sequencer event review (moderation-flavored; ops controls live
        // at tools.aurora.ops.{getSequencerStatus,pauseSequencer,...}).
        .route(
            "/xrpc/com.atproto.admin.listRecentEvents",
            get(list_recent_events),
        )
        // ---- tools.aurora.* top-level (chainlink #99 / Phase 3.2) ----
        //
        // Capability probe — clients call this to discover which
        // Aurora extensions this instance supports without trial-
        // and-error against individual endpoints. List C: meta-
        // endpoint that *describes* the registry, so it can't be
        // an entry in the registry it describes (Step 0 Q6).
        .route(
            "/xrpc/tools.aurora.describeCapabilities",
            get(describe_capabilities),
        )
        // ---- tools.aurora.ops.* (operator / infrastructure tier) ----
        //
        // Stats and account-listing.
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getStats",
            get(get_stats),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.listAccounts",
            get(ops_list_accounts),
            CapsBuilder::new(Family::Ops),
        )
        // Phase 2.3.8 — `getInstanceMetrics` is the
        // `instance-metrics-v1` introducer.
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getInstanceMetrics",
            get(ops_get_instance_metrics),
            CapsBuilder::new(Family::Ops).extensions(["instance-metrics-v1"]),
        )
        // Health, metrics, validation, nonce store.
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getValidationFailures",
            get(get_validation_failures),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getSystemHealth",
            get(get_system_health),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getDatabaseStatus",
            get(get_database_status),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getResourceUsage",
            get(get_resource_usage),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.listBackgroundJobs",
            get(list_background_jobs),
            CapsBuilder::new(Family::Ops),
        )
        // POST: this is an action that triggers health-check execution
        // (with side effects in the form of probe RPCs / DB queries),
        // not an idempotent property read. The other ops actions
        // (cleanupNonceStores, runBlobGC, pauseSequencer, etc.) all
        // use POST; this entry was an outlier that the admin UI's
        // SystemHealth page hit with POST and got a 405 in return.
        .route_with_caps(
            "/xrpc/tools.aurora.ops.runHealthChecks",
            post(run_health_checks),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getVersionInfo",
            get(get_version_info),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getSystemMetrics",
            get(get_system_metrics),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getNonceStoreStatus",
            get(get_nonce_store_status),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.cleanupNonceStores",
            post(cleanup_nonce_stores),
            CapsBuilder::new(Family::Ops),
        )
        // Blob storage.
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getBlobStatistics",
            get(get_blob_statistics),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.listBlobs",
            get(list_blobs),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.deleteBlob",
            post(delete_blob),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.quarantineBlob",
            post(quarantine_blob),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.restoreBlob",
            post(restore_blob),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.runBlobGC",
            post(run_blob_gc),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getBlobQuotas",
            get(get_blob_quotas),
            CapsBuilder::new(Family::Ops),
        )
        // Sequencer infrastructure.
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getSequencerStatus",
            get(get_sequencer_status),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.pauseSequencer",
            post(pause_sequencer),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.resumeSequencer",
            post(resume_sequencer),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.resetSequencerCursor",
            post(reset_sequencer_cursor),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.rebuildSequencer",
            post(rebuild_sequencer),
            CapsBuilder::new(Family::Ops),
        )
        // Rate limiting.
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getRateLimitConfig",
            get(get_rate_limit_config),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getRateLimitStatus",
            get(get_rate_limit_status),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.cleanupRateLimitState",
            post(cleanup_rate_limit_state),
            CapsBuilder::new(Family::Ops),
        )
        // Federation / relay.
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getFederationStatus",
            get(get_federation_status),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getRelayConfig",
            get(get_relay_config),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.listKnownInstances",
            get(list_known_instances),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.triggerPdsDiscovery",
            post(trigger_pds_discovery),
            CapsBuilder::new(Family::Ops),
        )
        // #344 — SuperAdmin read of the full deployment federation config (the
        // env-view the Configuration → Federation policy page renders). Distinct
        // from getFederationStatus (counts/connectivity); the handler gates
        // SuperAdmin and returns security-adjacent fields (peer allowlist) the
        // public describe endpoints intentionally omit.
        .route_with_caps(
            "/xrpc/tools.aurora.ops.getFederationPolicy",
            get(get_federation_policy),
            CapsBuilder::new(Family::Ops),
        )
        // v0.9 Federation Pattern-1 Phase B (#352) — peer-allowlist CRUD.
        // SuperAdmin-gated in-handler (the federation family lives under
        // Family::Ops alongside getFederationPolicy); mutates the runtime
        // federation.policy.peer-allowlist via CAS + audit emission.
        .route_with_caps(
            "/xrpc/tools.aurora.ops.addFederationPeer",
            post(add_federation_peer),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.removeFederationPeer",
            post(remove_federation_peer),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.modifyFederationPeer",
            post(modify_federation_peer),
            CapsBuilder::new(Family::Ops),
        )
        // v0.9 Federation Pattern-1 Phase C (#353) — discovery-mode +
        // pending-discovery dismissal.
        .route_with_caps(
            "/xrpc/tools.aurora.ops.setDiscoveryMode",
            post(set_discovery_mode),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.dismissPendingDiscovery",
            post(dismiss_pending_discovery),
            CapsBuilder::new(Family::Ops),
        )
        // v0.9 Federation Pattern-1 Phase D (#354) — relay runtime-switch.
        .route_with_caps(
            "/xrpc/tools.aurora.ops.addRelayUrl",
            post(add_relay_url),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.removeRelayUrl",
            post(remove_relay_url),
            CapsBuilder::new(Family::Ops),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.setFederationRelays",
            post(set_federation_relays),
            CapsBuilder::new(Family::Ops),
        )
        // ---- tools.aurora.moderator.* (chainlink #100 / Phase 3.3) ----
        //
        // Moderator-tier read endpoints. Five queries with shared
        // rich-context infrastructure (resolve_handles, etc.) in
        // src/api/aurora_moderator.rs. Auth: AdminAuthContext
        // (Moderator+); namespace middleware also gates
        // tools.aurora.moderator.* to atproto:admin.moderation.
        //
        // Extension attribution: `moderator-activity-v1` ships on
        // queryEvents (the primary activity query); `getEvent` and
        // `queryStatuses` share that capability without
        // re-declaring it. `subject-context-v1` is Phase 3.2's
        // capability-probe companion attributed to its endpoint.
        // `subject-history-v1` is Phase 3.3 attributed to its
        // endpoint.
        .route_with_caps(
            "/xrpc/tools.aurora.moderator.queryEvents",
            get(crate::api::aurora_moderator::query_events),
            CapsBuilder::new(Family::Moderator).extensions(["moderator-activity-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.moderator.getEvent",
            get(crate::api::aurora_moderator::get_event),
            CapsBuilder::new(Family::Moderator),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.moderator.queryStatuses",
            get(crate::api::aurora_moderator::query_statuses),
            CapsBuilder::new(Family::Moderator),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.moderator.getSubjectContext",
            get(crate::api::aurora_moderator::get_subject_context),
            CapsBuilder::new(Family::Moderator).extensions(["subject-context-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.moderator.getSubjectHistory",
            get(crate::api::aurora_moderator::get_subject_history),
            CapsBuilder::new(Family::Moderator).extensions(["subject-history-v1"]),
        )
        // ---- tools.aurora.moderator.* appeals reads (chainlink #101 / Phase 3.4) ----
        //
        // Two endpoints reusing 3.3's foundation types and rich-context
        // helpers (resolve_handles + new fetch_action_summaries batch
        // lookup). Auth: same AdminAuthContext + namespace scope as
        // the other moderator-tier endpoints. `appeals-v1` is
        // attributed to `listAppeals` (the first appeals route);
        // `getAppeal` shares the capability.
        .route_with_caps(
            "/xrpc/tools.aurora.moderator.listAppeals",
            get(crate::api::aurora_moderator::list_appeals),
            CapsBuilder::new(Family::Moderator).extensions(["appeals-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.moderator.getAppeal",
            get(crate::api::aurora_moderator::get_appeal),
            CapsBuilder::new(Family::Moderator),
        )
        // ---- tools.aurora.admin.* (chainlink #102 / Phase 3.5) ----
        //
        // Admin-tier action surface. emitEvent is the unified dispatch
        // for moderation actions per AURORA_ADMIN_UI_DESIGN.md §8.1;
        // per-action endpoints under com.atproto.admin.* stay live
        // for protocol-compatibility but the UI consumes emitEvent
        // exclusively post-3.5 (§9.2).
        //
        // Auth: AdminModeration scope (namespace middleware); within-
        // tier role checks happen at handler level (Moderator+ for
        // content actions, Admin+ for account-infrastructure actions).
        .route_with_caps(
            "/xrpc/tools.aurora.admin.emitEvent",
            post(crate::api::aurora_admin::emit_event),
            CapsBuilder::new(Family::Admin).extensions(["mod-events-emit-v1"]),
        )
        // Batch endpoints (Phase 3.5, §8.8–§8.13). Six atomic
        // multi-subject procedures driven by BulkActionPanel
        // (substrate primitive 4). 50-subject hard cap per design
        // doc. `batch-takedown-v1` is attributed to
        // `batchTakedownAccounts` (the first batch route in
        // registration order); the other five share the
        // capability.
        .route_with_caps(
            "/xrpc/tools.aurora.admin.batchTakedownAccounts",
            post(crate::api::aurora_admin::batch_takedown_accounts),
            CapsBuilder::new(Family::Admin).extensions(["batch-takedown-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.admin.batchSuspendAccounts",
            post(crate::api::aurora_admin::batch_suspend_accounts),
            CapsBuilder::new(Family::Admin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.admin.batchRestoreAccounts",
            post(crate::api::aurora_admin::batch_restore_accounts),
            CapsBuilder::new(Family::Admin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.admin.batchTakedownRecords",
            post(crate::api::aurora_admin::batch_takedown_records),
            CapsBuilder::new(Family::Admin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.admin.batchApplyLabel",
            post(crate::api::aurora_admin::batch_apply_label),
            CapsBuilder::new(Family::Admin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.admin.batchRemoveLabel",
            post(crate::api::aurora_admin::batch_remove_label),
            CapsBuilder::new(Family::Admin),
        )
        // triggerPasswordReset (Phase 3.5, §8.6). Admin+ role check
        // happens at handler level. Rationale recorded in the
        // hash-chained audit_chain_entry per design doc §3.4.
        .route_with_caps(
            "/xrpc/tools.aurora.admin.triggerPasswordReset",
            post(crate::api::aurora_admin::trigger_password_reset),
            CapsBuilder::new(Family::Admin).extensions(["trigger-password-reset-v1"]),
        )
        // ---- tools.aurora.lexicon.* (Arc 17 §17.3.7 / #133 pin) ----
        //
        // Three admin endpoints for the dynamic-lexicon resolver
        // cache. Auth: AdminAuthContext extractor + Admin+ role
        // (mirrors the AdminAll scope per Step 0.0h pin). When the
        // resolver is not wired on AppContext (PDS_LEXICON_ENABLED=
        // false, the v0.5 default) every handler responds with
        // HTTP 503 LexiconDisabled.
        //
        // Registered as plain (non-registry) routes: registering
        // under Family::Admin would advertise them as
        // tools.aurora.admin.<name> in describeCapabilities (wrong
        // namespace), and Family::Lexicon doesn't exist yet. The
        // §17.1.1 promised wire path is honored by the URL itself;
        // describeCapabilities will surface them once a future
        // cycle adds Family::Lexicon (snapshot tests at admin.rs:
        // 7726 will need refreshing then).
        .route(
            "/xrpc/tools.aurora.lexicon.getCacheState",
            get(crate::api::aurora_lexicon::get_cache_state),
        )
        .route(
            "/xrpc/tools.aurora.lexicon.evictCache",
            post(crate::api::aurora_lexicon::evict_cache),
        )
        .route(
            "/xrpc/tools.aurora.lexicon.fetchNow",
            post(crate::api::aurora_lexicon::fetch_now),
        )
        // Phase 3.7 (chainlink #104) — moderation aggregations.
        // Auth: AdminModeration scope, Moderator+ role enforced at
        // handler level. Powers Dashboard Moderator flavor + bell
        // badge. Per §8.2 / §8.3.
        //
        // Wire order quirk: `moderation-metrics-v1` precedes
        // `queue-stats-v1` in the curated extensions list, but
        // `getQueueStats` is registered before `getModerationMetrics`
        // (the natural alphabetical/grouped order from the original
        // hand-written router). Wire ordering is preserved by
        // WIRE_EXTENSION_ORDER rather than registration order, so
        // the per-route attribution below is the natural one.
        .route_with_caps(
            "/xrpc/tools.aurora.admin.getQueueStats",
            get(crate::api::aurora_admin::get_queue_stats),
            CapsBuilder::new(Family::Admin).extensions(["queue-stats-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.admin.getModerationMetrics",
            get(crate::api::aurora_admin::get_moderation_metrics),
            CapsBuilder::new(Family::Admin).extensions(["moderation-metrics-v1"]),
        )
        // Phase 3.8 (chainlink #105) — hash-chained audit trail.
        // Auth: AdminModeration scope, Moderator+ role at handler.
        // Per design doc §8.4: cursor-paginated newest-first; verified
        // flag computed at query time by re-hashing entry content.
        .route_with_caps(
            "/xrpc/tools.aurora.admin.getAuditTrail",
            get(crate::api::aurora_admin::get_audit_trail),
            CapsBuilder::new(Family::Admin).extensions(["audit-trail-v1"]),
        )
        // #302 — single-report fetch for the report-detail page. The store
        // method existed; this HTTP surface was never registered (the UI 404'd
        // on the legacy com.atproto.admin.getReport NSID). Moderator+; no
        // capability extension (a basic read, gated by role).
        .route_with_caps(
            "/xrpc/tools.aurora.admin.getReport",
            get(crate::api::aurora_admin::get_report),
            CapsBuilder::new(Family::Admin),
        )
        // Phase 3.8 (chainlink #105) — chain-of-custody forensic
        // export. AdminServer scope; Admin+ baseline at handler with
        // SuperAdmin gates on metadata + chain-inclusion params per
        // design doc §8.7.
        .route_with_caps(
            "/xrpc/tools.aurora.admin.exportAccountForensic",
            post(crate::api::aurora_admin::export_account_forensic),
            CapsBuilder::new(Family::Admin).extensions(["forensic-export-v1"]),
        )
        // Phase 3.9 (chainlink #106) — real-time subscription via
        // WebSocket. Auth: AdminModeration scope, Moderator+ role.
        // Polling-driven (5s tick) over moderation_event with
        // heartbeat at 30s; wire protocol per §8.5 message shapes.
        .route_with_caps(
            "/xrpc/tools.aurora.admin.subscribeModEvents",
            get(crate::api::aurora_subscribe::subscribe_mod_events),
            CapsBuilder::new(Family::Admin).extensions(["mod-events-stream-v1"]),
        )
        // Phase 3.10 (chainlink #117) — runtime settings infrastructure.
        // Two-tier config (runtime > file). Read at most-Admin-or-key-
        // dependent role; write SuperAdmin only with audit-chained
        // rationale per design doc §8.16. `runtime-settings-v1`
        // attributed to `getRuntimeSetting` (the read endpoint);
        // `setRuntimeSetting` shares the capability.
        .route_with_caps(
            "/xrpc/tools.aurora.admin.getRuntimeSetting",
            get(crate::api::aurora_admin::get_runtime_setting),
            CapsBuilder::new(Family::Admin).extensions(["runtime-settings-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.admin.setRuntimeSetting",
            post(crate::api::aurora_admin::set_runtime_setting),
            CapsBuilder::new(Family::Admin),
        )
        // ---- v0.9 Arc B theming substrate (§11.10.2) ----
        .route_with_caps(
            "/xrpc/tools.aurora.ops.themes.listInstalled",
            get(crate::api::aurora_admin::list_installed_themes),
            CapsBuilder::new(Family::Admin).extensions(["themes-v1"]),
        )
        // ---- v0.9 Arc D Kryphocron (§6.4.2) — Laquna rotation ----
        .route_with_caps(
            "/xrpc/tools.aurora.ops.kryphocron.triggerRotation",
            post(crate::api::aurora_admin::trigger_rotation),
            CapsBuilder::new(Family::Ops).extensions(["kryphocron-rotation-v1"]),
        )
        // ---- v0.9 Arc D Kryphocron (#225) — operator read cohort (§6.4, §6.5) ----
        // Ten read XRPC backing the Kryphocron domain pages. All attribute the
        // single `kryphocron-read-v1` cohort capability; role floor (Moderator+
        // vs Admin+) is enforced in-handler per design §6.4.x.
        .route_with_caps(
            "/xrpc/tools.aurora.ops.kryphocron.getSubstrateInfo",
            get(crate::api::aurora_kryphocron_ops::get_substrate_info),
            CapsBuilder::new(Family::Ops).extensions(["kryphocron-read-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.kryphocron.getTierStats",
            get(crate::api::aurora_kryphocron_ops::get_tier_stats),
            CapsBuilder::new(Family::Ops).extensions(["kryphocron-read-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.kryphocron.getOracleActivity",
            get(crate::api::aurora_kryphocron_ops::get_oracle_activity),
            CapsBuilder::new(Family::Ops).extensions(["kryphocron-read-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.kryphocron.getRotationStatus",
            get(crate::api::aurora_kryphocron_ops::get_rotation_status),
            CapsBuilder::new(Family::Ops).extensions(["kryphocron-read-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.kryphocron.getRotationProgress",
            get(crate::api::aurora_kryphocron_ops::get_rotation_progress),
            CapsBuilder::new(Family::Ops).extensions(["kryphocron-read-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.kryphocron.cancelRotation",
            post(crate::api::aurora_kryphocron_ops::cancel_rotation),
            CapsBuilder::new(Family::Ops).extensions(["kryphocron-read-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.kryphocron.listRotations",
            get(crate::api::aurora_kryphocron_ops::list_rotations),
            CapsBuilder::new(Family::Ops).extensions(["kryphocron-read-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.kryphocron.getAudienceAggregate",
            get(crate::api::aurora_kryphocron_ops::get_audience_aggregate),
            CapsBuilder::new(Family::Ops).extensions(["kryphocron-read-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.kryphocron.listAudiences",
            get(crate::api::aurora_kryphocron_ops::list_audiences),
            CapsBuilder::new(Family::Ops).extensions(["kryphocron-read-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.kryphocron.getBlockCascadeImpact",
            get(crate::api::aurora_kryphocron_ops::get_block_cascade_impact),
            CapsBuilder::new(Family::Ops).extensions(["kryphocron-read-v1"]),
        )
        // Per-account overrides (#316 / §6.6.2 item 4) — SuperAdmin (in-handler);
        // read + audited write of the per-account kryphocron override row.
        .route_with_caps(
            "/xrpc/tools.aurora.ops.kryphocron.getAccountOverrides",
            get(crate::api::aurora_kryphocron_ops::get_account_overrides),
            CapsBuilder::new(Family::Ops).extensions(["kryphocron-overrides-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.ops.kryphocron.setAccountOverride",
            post(crate::api::aurora_kryphocron_ops::set_account_override),
            CapsBuilder::new(Family::Ops).extensions(["kryphocron-overrides-v1"]),
        )
        // ---- tools.aurora.superadmin.* (chainlink #103 / Phase 3.6) ----
        //
        // Role management relocated from com.atproto.admin.* per design
        // doc §5.4. SuperAdmin scope check is enforced at handler level
        // (auth.role.can_act_as(Role::SuperAdmin)) — the namespace
        // alone doesn't gate this; the handler does. Per pre-deployment
        // framing, no deprecation aliases — clean wire-break.
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.grantRole",
            post(grant_role),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.revokeRole",
            post(revoke_role),
            CapsBuilder::new(Family::SuperAdmin),
        )
        // §5.5.4 Phase B (#346) — SuperAdmin manual reviewer reassignment
        // (§4.7): set a queue item's assignee with assignment_source =
        // 'manual_override'. Covers orphan-and-escalated recovery.
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.assignReviewer",
            post(assign_reviewer),
            CapsBuilder::new(Family::SuperAdmin),
        )
        // §5.5.4 Phase C (#347) — auto-label rule CRUD (§3.5).
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.createAutoLabelRule",
            post(create_auto_label_rule),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.editAutoLabelRule",
            post(edit_auto_label_rule),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.deleteAutoLabelRule",
            post(delete_auto_label_rule),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.listAutoLabelRules",
            get(list_auto_label_rules),
            CapsBuilder::new(Family::SuperAdmin),
        )
        // §5.5.4 Phase D (#348) — escalation rule CRUD + de-escalation (§5).
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.createEscalationRule",
            post(create_escalation_rule),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.editEscalationRule",
            post(edit_escalation_rule),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.deleteEscalationRule",
            post(delete_escalation_rule),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.listEscalationRules",
            get(list_escalation_rules),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.clearEscalation",
            post(clear_escalation),
            CapsBuilder::new(Family::SuperAdmin),
        )
        // §5.5.4 Phase E (#349) — composite-load of all four sub-surfaces (§6.5).
        // Design names it ops.moderation.getDefaultsState; kept under the
        // superadmin family for consistency with the §5.5.4 CRUD + the
        // SuperAdmin gate (namespace is cosmetic; the handler gates).
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.getDefaultsState",
            get(get_defaults_state),
            CapsBuilder::new(Family::SuperAdmin),
        )
        // v0.9 Integration hooks Phase A (#350) — declaration CRUD +
        // composite-load (declaration without execution).
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.createHook",
            post(create_hook),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.editHook",
            post(edit_hook),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.deleteHook",
            post(delete_hook),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.listHooks",
            get(list_hooks),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.getIntegrationHooksState",
            get(get_integration_hooks_state),
            CapsBuilder::new(Family::SuperAdmin),
        )
        // v0.9 (#329) — upload a login-splash branding asset (logo/banner)
        // directly; writes it under <data>/branding/ and repoints the
        // branding.login-* runtime setting. Raw-body upload (uploadBlob idiom).
        // Raise the body limit to cover the 5MB banner (axum defaults to 2MB);
        // the handler enforces the precise per-asset cap (1MB logo / 5MB banner).
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.uploadBrandingAsset",
            post(crate::api::aurora_admin::upload_branding_asset)
                .layer(axum::extract::DefaultBodyLimit::max(6 * 1024 * 1024)),
            CapsBuilder::new(Family::SuperAdmin),
        )
        // Repository rebuild preflight (§7.4.1 / #286) — non-destructive read.
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.preRebuildCheck",
            get(pre_rebuild_check),
            CapsBuilder::new(Family::SuperAdmin),
        )
        // Repository rebuild — destructive surface (§7.4.1 / #290): start a
        // background rebuild, poll its progress, cancel it. Reconstructs from
        // sequencer history → verify → atomic per-DID swap.
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.rebuildRepo",
            post(rebuild_repo),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.getRebuildProgress",
            get(get_rebuild_progress),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.cancelRebuild",
            post(cancel_rebuild),
            CapsBuilder::new(Family::SuperAdmin),
        )
        // Bulk repository repair — scan substrate (§7.4.3 / #291): start an
        // across-accounts inconsistency scan, poll its progress, cancel it,
        // and read the persisted findings.
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.scanReposForInconsistencies",
            post(scan_repos_for_inconsistencies),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.getScanProgress",
            get(get_scan_progress),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.cancelScan",
            post(cancel_scan),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.getRepoScanResults",
            get(get_repo_scan_results),
            CapsBuilder::new(Family::SuperAdmin),
        )
        // Bulk repository repair — repair substrate (§7.4.3 / #292): start a
        // bulk repair over the scan findings (or a subset), poll its bulk
        // progress, cancel it. Each account is rebuilt via the per-account
        // machinery.
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.repairRepos",
            post(repair_repos),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.getBulkRepairProgress",
            get(get_bulk_repair_progress),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.cancelBulkRepair",
            post(cancel_bulk_repair),
            CapsBuilder::new(Family::SuperAdmin),
        )
        // Sequencer recovery — escalation surface (§7.4.2 / #294): enumerate
        // available recovery operations + the sequencer state, run one (v0.9:
        // read-only deep integrity validation), poll its progress, cancel it.
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.sequencerRecoveryOptions",
            get(sequencer_recovery_options),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.runSequencerRecovery",
            post(run_sequencer_recovery),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.getSequencerRecoveryProgress",
            get(get_sequencer_recovery_progress),
            CapsBuilder::new(Family::SuperAdmin),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.superadmin.cancelSequencerRecovery",
            post(cancel_sequencer_recovery),
            CapsBuilder::new(Family::SuperAdmin),
        )
        // Per-operator session management (§8.1.7 / #273). Admin-tier
        // namespace; the handlers gate finer: any operator lists/revokes
        // their OWN sessions (self-service), SuperAdmin lists all and
        // force-logs-out any. The "session-management-v1" capability is
        // introduced on listSessions (the canonical introducer).
        .route_with_caps(
            "/xrpc/tools.aurora.admin.listSessions",
            get(list_sessions),
            CapsBuilder::new(Family::Admin).extensions(["session-management-v1"]),
        )
        .route_with_caps(
            "/xrpc/tools.aurora.admin.revokeSession",
            post(revoke_session),
            CapsBuilder::new(Family::Admin),
        )
        // #338 — SuperAdmin bulk force-logout of an operator's sessions. Same
        // session-management capability; the handler enforces the SuperAdmin floor.
        .route_with_caps(
            "/xrpc/tools.aurora.admin.revokeOperatorSessions",
            post(revoke_operator_sessions),
            CapsBuilder::new(Family::Admin),
        )
        .build()
}

// ============================================================================
// Admin Endpoints (OAuth Authentication via AdminAuthContext)
// ============================================================================

#[derive(Deserialize)]
struct CreateInviteCodeRequest {
    uses: Option<i32>,
    expires_days: Option<i64>,
    note: Option<String>,
    for_account: Option<String>,
}

/// Create an invite code
async fn create_invite_code(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<CreateInviteCodeRequest>,
) -> Result<Json<InviteCode>, (StatusCode, String)> {
    let uses = req.uses.unwrap_or(1);
    let expires_in = req.expires_days.map(Duration::days);

    // LB-1 Session 12 / chainlink #129: invite_code INSERT + chain
    // entry in one transaction.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let code = crate::admin::invites::InviteCodeManager::create_invite_in_tx(
        &mut tx,
        &auth.did,
        uses,
        expires_in,
        req.note.clone(),
        req.for_account.clone(),
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let rationale = format!(
        "created invite code {} (uses: {})",
        code.code, code.available
    );
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "invite.create",
            subject: None,
            rationale: &rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(code))
}

#[derive(Debug, Deserialize)]
struct GetInviteCodesQuery {
    /// `recent` (default) or `usage` per the lexicon's knownValues.
    #[serde(default)]
    sort: Option<String>,
    /// Page size, 1-500, default 100 per the lexicon.
    #[serde(default)]
    limit: Option<i64>,
    /// Opaque cursor produced by a previous response.
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct GetInviteCodesResponse {
    codes: Vec<InviteCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
}

/// Encode a typed cursor as base64url-no-pad JSON.
fn encode_invite_cursor(cursor: &crate::admin::invites::InviteCursor) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let json = serde_json::to_vec(cursor).expect("cursor enum is JSON-serialisable");
    URL_SAFE_NO_PAD.encode(json)
}

/// Decode a base64url-no-pad cursor, returning a 400 with `InvalidRequest`
/// shape if the cursor is malformed or the decoded sort doesn't match the
/// request's sort.
fn decode_invite_cursor(
    raw: &str,
    expected_sort: crate::admin::invites::InviteSortKey,
) -> Result<crate::admin::invites::InviteCursor, (StatusCode, String)> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let json = URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Malformed cursor".to_string()))?;
    let cursor: crate::admin::invites::InviteCursor = serde_json::from_slice(&json)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Malformed cursor".to_string()))?;
    if cursor.sort_key() != expected_sort {
        return Err((
            StatusCode::BAD_REQUEST,
            "cursor was issued for a different sort ordering".to_string(),
        ));
    }
    Ok(cursor)
}

/// Build a paginated invite-code response from a `Vec<(InviteCode, i64)>`
/// returned by the manager. Trims to `limit` and emits a cursor if more
/// results were available.
fn paginated_invite_response(
    mut rows: Vec<(InviteCode, i64)>,
    sort: crate::admin::invites::InviteSortKey,
    limit: i64,
) -> (Vec<InviteCode>, Option<String>) {
    use crate::admin::invites::InviteCursor;
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let next_cursor = if has_more {
        rows.last().map(|(code, use_count)| {
            let cur = match sort {
                crate::admin::invites::InviteSortKey::Recent => InviteCursor::Recent {
                    after_created_at: code.created_at.to_rfc3339(),
                    after_code: code.code.clone(),
                },
                crate::admin::invites::InviteSortKey::Usage => InviteCursor::Usage {
                    after_use_count: *use_count,
                    after_code: code.code.clone(),
                },
            };
            encode_invite_cursor(&cur)
        })
    } else {
        None
    };
    (rows.into_iter().map(|(c, _)| c).collect(), next_cursor)
}

/// Get an admin view of invite codes (lexicon `com.atproto.admin.getInviteCodes`).
///
/// Phase 1.10 (#65) wired up the lexicon's sort/limit/cursor parameters
/// and removed the legacy `includeDisabled` parameter. Disabled-only
/// filtering relocates to a `tools.aurora.ops.*` endpoint per the
/// assessment doc Phase 2.
async fn get_invite_codes(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<GetInviteCodesQuery>,
) -> Result<Json<GetInviteCodesResponse>, (StatusCode, String)> {
    let sort = crate::admin::invites::InviteSortKey::from_param(query.sort.as_deref())
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let limit = query.limit.unwrap_or(100);
    if !(1..=500).contains(&limit) {
        return Err((
            StatusCode::BAD_REQUEST,
            "limit must be between 1 and 500".to_string(),
        ));
    }
    let cursor = match query.cursor.as_deref() {
        Some(raw) => Some(decode_invite_cursor(raw, sort)?),
        None => None,
    };

    let rows = ctx
        .invite_manager
        .list_codes_paginated(sort, cursor.as_ref(), limit + 1)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (codes, cursor) = paginated_invite_response(rows, sort, limit);
    Ok(Json(GetInviteCodesResponse { codes, cursor }))
}

#[derive(Debug, Deserialize)]
struct ListInviteCodesQuery {
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct ListInviteCodesResponse {
    codes: Vec<InviteCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
}

/// List invite codes (Aurora-Locus surface paralleling `getInviteCodes`).
///
/// Phase 1.10 (#65) wired the limit/cursor params that were previously
/// accepted-and-ignored. Reuses `getInviteCodes`'s pagination machinery.
async fn list_invite_codes(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<ListInviteCodesQuery>,
) -> Result<Json<ListInviteCodesResponse>, (StatusCode, String)> {
    let sort = crate::admin::invites::InviteSortKey::from_param(query.sort.as_deref())
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let limit = query.limit.unwrap_or(100);
    if !(1..=500).contains(&limit) {
        return Err((
            StatusCode::BAD_REQUEST,
            "limit must be between 1 and 500".to_string(),
        ));
    }
    let cursor = match query.cursor.as_deref() {
        Some(raw) => Some(decode_invite_cursor(raw, sort)?),
        None => None,
    };

    let rows = ctx
        .invite_manager
        .list_codes_paginated(sort, cursor.as_ref(), limit + 1)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (codes, cursor) = paginated_invite_response(rows, sort, limit);
    Ok(Json(ListInviteCodesResponse { codes, cursor }))
}

/// Get server statistics
async fn get_stats(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Get statistics from database
    let total_users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM account")
        .fetch_one(&ctx.account_db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Note: Post count would require querying all actor databases - expensive
    // Set to 0 for now, can be improved later
    let total_posts: i64 = 0;

    // chainlink #95: bind RFC-3339 from app code (see jobs/tasks.rs for rationale).
    let now_rfc3339 = chrono::Utc::now().to_rfc3339();
    let active_sessions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM session WHERE expires_at > $1")
            .bind(&now_rfc3339)
            .fetch_one(&ctx.account_db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let pending_reports: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM report WHERE status = 'open'")
            .fetch_one(&ctx.account_db)
            .await
            .unwrap_or(0);

    Ok(Json(serde_json::json!({
        "totalUsers": total_users,
        "totalPosts": total_posts,
        "activeSessions": active_sessions,
        "pendingReports": pending_reports,
    })))
}

#[derive(Deserialize)]
struct GetUsersParams {
    limit: Option<i64>,
    cursor: Option<String>,
}

/// Get list of users
async fn get_users(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<GetUsersParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let limit = params.limit.unwrap_or(50).min(100);

    // Status is computed from actor state:
    //   takedown_ref present -> 'taken_down'
    //   deactivated_at present -> 'deactivated'
    //   otherwise -> 'active'
    let status_expr = "CASE \
        WHEN a.takedown_ref IS NOT NULL THEN 'taken_down' \
        WHEN a.deactivated_at IS NOT NULL THEN 'deactivated' \
        ELSE 'active' END";

    let users: Vec<serde_json::Value> = if let Some(cursor) = params.cursor {
        sqlx::query_as::<_, (String, String, Option<String>, String, String)>(&format!(
            "SELECT a.did, a.handle, ac.email, a.created_at, {} as status \
                 FROM actor a \
                 LEFT JOIN account ac ON a.did = ac.did \
                 WHERE a.did > ? ORDER BY a.did LIMIT ?",
            status_expr
        ))
        .bind(cursor)
        .bind(limit)
        .fetch_all(&ctx.account_db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        sqlx::query_as::<_, (String, String, Option<String>, String, String)>(&format!(
            "SELECT a.did, a.handle, ac.email, a.created_at, {} as status \
                 FROM actor a \
                 LEFT JOIN account ac ON a.did = ac.did \
                 ORDER BY a.did LIMIT ?",
            status_expr
        ))
        .bind(limit)
        .fetch_all(&ctx.account_db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    }
    .into_iter()
    .map(|(did, handle, email, created_at, status)| {
        serde_json::json!({
            "did": did,
            "handle": handle,
            "email": email,
            "createdAt": created_at,
            "status": status,
        })
    })
    .collect();

    let cursor = users
        .last()
        .and_then(|u| u.get("did"))
        .and_then(|d| d.as_str());

    Ok(Json(serde_json::json!({
        "users": users,
        "cursor": cursor,
    })))
}

// ============================================================================
// Role Management Endpoints
// ============================================================================

#[derive(Deserialize)]
struct GrantRoleRequest {
    did: String,
    role: String,
    #[serde(default)]
    rationale: Option<String>,
}

/// Output for `tools.aurora.superadmin.grantRole`. Surfaces
/// `auditEntryId` per the action-ID contract committed in
/// `crate::admin::audit_chain` (Arc 2 §6.4.2). Pre-Arc-2 this
/// handler returned `serde_json::json!(...)` ad-hoc with the
/// snake_case wire field `audit_entry_id`; Step 2 normalized
/// to a typed struct with camelCase wire fields throughout.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantRoleOutput {
    pub success: bool,
    pub did: String,
    pub role: String,
    /// Full role record as recorded in the `admin_roles` table.
    /// Wire field renamed from `admin_role` to `adminRole` as part
    /// of Arc 2's typed-struct conversion (`rename_all =
    /// "camelCase"`).
    pub admin_role: crate::admin::roles::AdminRole,
    /// Wire field renamed from `audit_entry_id` to `auditEntryId`
    /// as part of Arc 2's action-ID contract; emitted as a string
    /// to dodge JS-number-precision issues with large i64 ids.
    pub audit_entry_id: String,
}

/// Grant admin role to a user
async fn grant_role(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<GrantRoleRequest>,
) -> Result<Json<GrantRoleOutput>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;

    // SuperAdmin only — relocated to tools.aurora.superadmin.* in
    // Phase 3.6 (chainlink #103). Per design doc §5.4, role
    // management is structurally a SuperAdmin operation; the
    // namespace makes that boundary visible, this guard enforces it.
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!(
                "grantRole requires SuperAdmin role; have {}",
                auth.role.as_str()
            ),
        ));
    }

    // Rationale required — every role grant is an audit-chain
    // decision and must carry operator-supplied context. Pattern
    // matches §8.6's `rationale-required` for triggerPasswordReset.
    let rationale = req
        .rationale
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            json_error(StatusCode::BAD_REQUEST, "InvalidRequest", "rationale-required")
        })?;

    // Parse role
    let role: Role = req
        .role
        .parse()
        .map_err(|e: PdsError| {
            json_error(StatusCode::BAD_REQUEST, "InvalidRequest", e.to_string())
        })?;

    let subject = Subject::Repo {
        did: req.did.clone(),
    };
    let chain_rationale = format!("{} (role={})", rationale, req.role);

    // LB-1 / chainlink #122: grant + chain in one transaction so a
    // crash between the two leaves neither row, not a role grant
    // without an audit-chain breadcrumb.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| {
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
        })?;
    let admin_role = crate::admin::AdminRoleManager::grant_role_in_tx(
        &mut tx,
        &req.did,
        role,
        &auth.did,
        Some(rationale.to_string()),
    )
    .await
    .map_err(|e| {
        json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
    })?;
    // Audit chain entry — replaces the legacy admin_audit_log path.
    // Subject is the target DID; the role being granted lives in the
    // rationale + the moderation_event details rather than as a
    // first-class chain field (chain schema is intentionally narrow).
    let audit_entry_id = audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "role.grant",
            subject: Some(&subject),
            rationale: &chain_rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| {
        json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
    })?;
    // §5.5.4 Phase B §4.5/§2.7: an operator-set change (here, addition)
    // invalidates the round-robin/category/escalation cursors — reset them
    // single-step within this same mutation transaction (no CAS contention).
    crate::api::reviewer_assignment::reset_assignment_cursors_in_tx(&mut tx, &auth.did)
        .await
        .map_err(|e| {
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
        })?;
    tx.commit()
        .await
        .map_err(|e| {
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
        })?;

    Ok(Json(GrantRoleOutput {
        success: true,
        did: req.did,
        role: req.role,
        admin_role,
        audit_entry_id: audit_entry_id.to_string(),
    }))
}

/// Output for `tools.aurora.superadmin.revokeRole`. Surfaces
/// `auditEntryId` per the action-ID contract committed in
/// `crate::admin::audit_chain`. Pre-Arc-2 this handler returned
/// `serde_json::json!(...)` ad-hoc; Step 2 normalized to a typed
/// struct with camelCase wire fields throughout. Wire field
/// renamed from `audit_entry_id` to `auditEntryId`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokeRoleOutput {
    pub success: bool,
    pub did: String,
    /// Emitted as a string to dodge JS-number-precision issues
    /// with large i64 ids; field name renamed wire-side from
    /// `audit_entry_id` to `auditEntryId` as part of Arc 2's
    /// action-ID contract.
    pub audit_entry_id: String,
}

#[derive(Deserialize)]
struct RevokeRoleRequest {
    did: String,
    /// Operator rationale for the revocation. Required — every role
    /// revoke is an audit-chain decision. The historical `reason`
    /// field continues to deserialize for backward compatibility but
    /// the chain-required field is `rationale`.
    #[serde(default, alias = "reason")]
    rationale: Option<String>,
}

/// Revoke admin role from a user
async fn revoke_role(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RevokeRoleRequest>,
) -> Result<Json<RevokeRoleOutput>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;

    // SuperAdmin only — same rationale as grant_role above
    // (chainlink #103 / Phase 3.6).
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!(
                "revokeRole requires SuperAdmin role; have {}",
                auth.role.as_str()
            ),
        ));
    }

    let rationale = req
        .rationale
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            json_error(StatusCode::BAD_REQUEST, "InvalidRequest", "rationale-required")
        })?;

    let subject = Subject::Repo {
        did: req.did.clone(),
    };

    // LB-1 / chainlink #122: revoke + chain in one transaction. §5.5.4
    // Phase B (primary path): the §4.7 reviewer-routing cleanup (category-
    // map prune + per-item assignment reset + cursor reset + audit) rides
    // this same transaction — atomic with the revocation.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| {
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
        })?;
    // Stage 1 — the revocation itself. A precondition failure here (e.g.
    // no active role) is not a rollback of an in-progress revocation, so
    // it does NOT emit the rollback diagnostic.
    crate::admin::AdminRoleManager::revoke_role_in_tx(
        &mut tx,
        &req.did,
        &auth.did,
        Some(rationale.to_string()),
    )
    .await
    .map_err(|e| {
        json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
    })?;
    // Stage 2 — audit + cleanup + commit. A failure from here rolls back a
    // revocation that DID begin, so it emits `operator_revocation_rollback`
    // (§4.7 / §6.1 system_diagnostic) from outside the rolled-back tx.
    let staged: crate::error::PdsResult<i64> = async {
        let audit_entry_id = audit_chain::insert_chain_entry(
            &mut tx,
            ctx.config.database.backend,
            AppendEntryParams {
                source: "manual",
                payload: None,
                actor_did: &auth.did,
                action: "role.revoke",
                subject: Some(&subject),
                rationale,
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        )
        .await?;
        crate::api::reviewer_assignment::handle_operator_removal_in_tx(
            &mut tx,
            ctx.config.database.backend,
            &req.did,
            &auth.did,
        )
        .await?;
        tx.commit().await.map_err(crate::error::PdsError::from)?;
        Ok(audit_entry_id)
    }
    .await;
    // Release the chain guard before any rollback-diagnostic emit (which
    // re-acquires it on its own transaction).
    drop(_chain_guard);

    match staged {
        Ok(audit_entry_id) => Ok(Json(RevokeRoleOutput {
            success: true,
            did: req.did,
            audit_entry_id: audit_entry_id.to_string(),
        })),
        Err(e) => {
            let _ = crate::api::reviewer_assignment::emit_revocation_rollback(
                &ctx,
                &req.did,
                &e.to_string(),
            )
            .await;
            Err(json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError",
                e.to_string(),
            ))
        }
    }
}

#[derive(Deserialize)]
struct AssignReviewerRequest {
    report_id: i64,
    operator_did: String,
    #[serde(default)]
    rationale: Option<String>,
}

/// `tools.aurora.superadmin.assignReviewer` (§5.5.4 Phase B §4.7) — manual
/// reviewer reassignment. Sets the queue item's `assigned_operator_did` and
/// `assignment_source = 'manual_override'`, atomic with an audit entry.
///
/// Local-idiom translation (recorded): the design registers
/// `moderation_reviewer_assigned` + the report-ID subject convention but does
/// not pin an audit emit for the manual-reassignment affordance. Aurora-Locus
/// audits every operator mutation, so this emits `moderation_reviewer_assigned`
/// with `source = "manual"` (a non-substrate operator action) and the report's
/// content subject + `report_id` in the payload — consistent with §6.1's
/// subject convention.
async fn assign_reviewer(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<AssignReviewerRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;

    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!(
                "assignReviewer requires SuperAdmin role; have {}",
                auth.role.as_str()
            ),
        ));
    }
    let rationale = req
        .rationale
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("manual reviewer reassignment");

    // Resolve the report so the audit subject reflects the content it points
    // at (account/record), not the report row itself.
    let report = ctx
        .report_manager
        .get_report(req.report_id)
        .await
        .map_err(|e| json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string()))?
        .ok_or_else(|| {
            json_error(StatusCode::NOT_FOUND, "NotFound", format!("report {} not found", req.report_id))
        })?;
    let subject = report.subject_uri.as_deref().map_or_else(
        || report.subject_did.as_deref().map(|d| Subject::Repo { did: d.to_string() }),
        |uri| {
            Some(Subject::Record {
                uri: uri.to_string(),
                cid: report.subject_cid.clone().unwrap_or_default(),
            })
        },
    );

    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string()))?;
    sqlx::query(
        "UPDATE report SET assigned_operator_did = $1, assignment_source = 'manual_override' WHERE id = $2",
    )
    .bind(&req.operator_did)
    .bind(req.report_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string()))?;
    let payload = serde_json::json!({
        "report_id": req.report_id,
        "assigned_operator_did": req.operator_did,
    });
    let audit_entry_id = audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: Some(payload),
            actor_did: &auth.did,
            action: "moderation_reviewer_assigned",
            subject: subject.as_ref(),
            rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "reportId": req.report_id,
        "assignedOperatorDid": req.operator_did,
        "auditEntryId": audit_entry_id.to_string(),
    })))
}

// ---------------------------------------------------------------------------
// §5.5.4 Phase C — auto-label rule CRUD (#347, §3.5). SuperAdmin-gated.
// ---------------------------------------------------------------------------

/// Map the store layer's `(u16, String)` into a json_error response.
fn rule_err((code, msg): (u16, String)) -> (StatusCode, Json<serde_json::Value>) {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let label = if code == 400 {
        "InvalidRequest"
    } else if code == 404 {
        "NotFound"
    } else {
        "InternalServerError"
    };
    json_error(status, label, msg)
}

fn require_superadmin(auth: &AdminAuthContext) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if auth.role.can_act_as(Role::SuperAdmin) {
        Ok(())
    } else {
        Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!("requires SuperAdmin role; have {}", auth.role.as_str()),
        ))
    }
}

#[derive(Deserialize)]
struct CreateAutoLabelRuleRequest {
    trigger_type: String,
    trigger_params: serde_json::Value,
    label_value: String,
    subject_scope: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    rationale: Option<String>,
}

fn default_true() -> bool {
    true
}

async fn create_auto_label_rule(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<CreateAutoLabelRuleRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    require_superadmin(&auth)?;
    let rule = crate::api::auto_label_rules::create_rule(
        &ctx,
        &auth.did,
        &req.trigger_type,
        &req.trigger_params,
        &req.label_value,
        &req.subject_scope,
        req.enabled,
        req.rationale.as_deref(),
    )
    .await
    .map_err(rule_err)?;
    Ok(Json(serde_json::json!({ "rule": rule })))
}

#[derive(Deserialize)]
struct EditAutoLabelRuleRequest {
    id: String,
    trigger_type: String,
    trigger_params: serde_json::Value,
    label_value: String,
    subject_scope: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    rationale: Option<String>,
}

async fn edit_auto_label_rule(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<EditAutoLabelRuleRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    require_superadmin(&auth)?;
    crate::api::auto_label_rules::edit_rule(
        &ctx,
        &auth.did,
        &req.id,
        &req.trigger_type,
        &req.trigger_params,
        &req.label_value,
        &req.subject_scope,
        req.enabled,
        req.rationale.as_deref(),
    )
    .await
    .map_err(rule_err)?;
    Ok(Json(serde_json::json!({ "success": true, "id": req.id })))
}

#[derive(Deserialize)]
struct DeleteAutoLabelRuleRequest {
    id: String,
}

async fn delete_auto_label_rule(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<DeleteAutoLabelRuleRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    require_superadmin(&auth)?;
    crate::api::auto_label_rules::delete_rule(&ctx, &auth.did, &req.id)
        .await
        .map_err(rule_err)?;
    Ok(Json(serde_json::json!({ "success": true, "id": req.id })))
}

#[derive(Deserialize)]
struct ListAutoLabelRulesQuery {
    #[serde(default)]
    include_deleted: bool,
}

async fn list_auto_label_rules(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Query(q): Query<ListAutoLabelRulesQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    require_superadmin(&auth)?;
    let rules = crate::api::auto_label_rules::list_rules(&ctx, q.include_deleted)
        .await
        .map_err(rule_err)?;
    Ok(Json(serde_json::json!({ "rules": rules })))
}

// ---------------------------------------------------------------------------
// §5.5.4 Phase D — escalation rule CRUD + clearEscalation (#348, §5). SuperAdmin.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateEscalationRuleRequest {
    trigger_type: String,
    trigger_params: serde_json::Value,
    action_type: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    rationale: Option<String>,
}

async fn create_escalation_rule(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<CreateEscalationRuleRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    require_superadmin(&auth)?;
    let rule = crate::api::escalation_rules::create_rule(
        &ctx,
        &auth.did,
        &req.trigger_type,
        &req.trigger_params,
        &req.action_type,
        req.enabled,
        req.rationale.as_deref(),
    )
    .await
    .map_err(rule_err)?;
    Ok(Json(serde_json::json!({ "rule": rule })))
}

#[derive(Deserialize)]
struct EditEscalationRuleRequest {
    id: String,
    trigger_type: String,
    trigger_params: serde_json::Value,
    action_type: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    rationale: Option<String>,
}

async fn edit_escalation_rule(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<EditEscalationRuleRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    require_superadmin(&auth)?;
    crate::api::escalation_rules::edit_rule(
        &ctx,
        &auth.did,
        &req.id,
        &req.trigger_type,
        &req.trigger_params,
        &req.action_type,
        req.enabled,
        req.rationale.as_deref(),
    )
    .await
    .map_err(rule_err)?;
    Ok(Json(serde_json::json!({ "success": true, "id": req.id })))
}

async fn delete_escalation_rule(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<DeleteAutoLabelRuleRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    require_superadmin(&auth)?;
    crate::api::escalation_rules::delete_rule(&ctx, &auth.did, &req.id)
        .await
        .map_err(rule_err)?;
    Ok(Json(serde_json::json!({ "success": true, "id": req.id })))
}

async fn list_escalation_rules(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Query(q): Query<ListAutoLabelRulesQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    require_superadmin(&auth)?;
    let rules = crate::api::escalation_rules::list_rules(&ctx, q.include_deleted)
        .await
        .map_err(rule_err)?;
    Ok(Json(serde_json::json!({ "rules": rules })))
}

#[derive(Deserialize)]
struct ClearEscalationRequest {
    item_id: String,
    #[serde(default)]
    rationale: Option<String>,
}

async fn clear_escalation(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<ClearEscalationRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    require_superadmin(&auth)?;
    let rationale = req
        .rationale
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("de-escalated by SuperAdmin");
    crate::api::escalation_rules::clear_escalation(&ctx, &req.item_id, rationale, &auth.did)
        .await
        .map_err(rule_err)?;
    Ok(Json(serde_json::json!({ "success": true, "itemId": req.item_id })))
}

// ---------------------------------------------------------------------------
// §5.5.4 Phase E — composite-load (#349, §6.5). SuperAdmin.
// ---------------------------------------------------------------------------

fn section_ok(data: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "status": "ok", "data": data })
}

fn section_err(code: &str, message: String) -> serde_json::Value {
    serde_json::json!({ "status": "error", "error": { "code": code, "message": message } })
}

/// `tools.aurora.ops.moderation.getDefaultsState` (§6.5) — one SuperAdmin GET
/// returning all four §5.5.4 sub-surfaces with partial-success semantics: a
/// section's failure surfaces as `{status:"error"}` in its slot, the endpoint
/// still returns HTTP 200. Saves the ConfigModerationPolicy page four calls.
async fn get_defaults_state(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::api::aurora_admin::{
        resolve_runtime_setting, MODERATION_DEFAULTS_CATEGORY_MAP_KEY,
        MODERATION_DEFAULTS_REPORT_ACTION_KEY, MODERATION_DEFAULTS_STALE_DAYS_KEY,
        MODERATION_REVIEWER_CATEGORY_MAP_KEY, MODERATION_REVIEWER_MODE_KEY,
        MODERATION_REVIEWER_MODE_VERSION_KEY,
    };
    require_superadmin(&auth)?;

    let report_action = section_ok(serde_json::json!({
        "mode": resolve_runtime_setting(&ctx, MODERATION_DEFAULTS_REPORT_ACTION_KEY).await,
        "categoryMap": resolve_runtime_setting(&ctx, MODERATION_DEFAULTS_CATEGORY_MAP_KEY).await,
        "staleDays": resolve_runtime_setting(&ctx, MODERATION_DEFAULTS_STALE_DAYS_KEY).await,
    }));
    let reviewer_assignment = section_ok(serde_json::json!({
        "mode": resolve_runtime_setting(&ctx, MODERATION_REVIEWER_MODE_KEY).await,
        "categoryMap": resolve_runtime_setting(&ctx, MODERATION_REVIEWER_CATEGORY_MAP_KEY).await,
        "modeVersion": resolve_runtime_setting(&ctx, MODERATION_REVIEWER_MODE_VERSION_KEY).await,
    }));
    let auto_label_rules = match crate::api::auto_label_rules::list_rules(&ctx, false).await {
        Ok(rules) => section_ok(serde_json::json!(rules)),
        Err((_, msg)) => section_err("auto_label_load_failed", msg),
    };
    let escalation_rules = match crate::api::escalation_rules::list_rules(&ctx, false).await {
        Ok(rules) => section_ok(serde_json::json!(rules)),
        Err((_, msg)) => section_err("escalation_load_failed", msg),
    };

    Ok(Json(serde_json::json!({
        "reportAction": report_action,
        "reviewerAssignment": reviewer_assignment,
        "autoLabelRules": auto_label_rules,
        "escalationRules": escalation_rules,
    })))
}

// ---------------------------------------------------------------------------
// v0.9 Integration hooks Phase A (#350) — declaration CRUD. SuperAdmin.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateHookRequest {
    name: String,
    url: String,
    event_classes: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    rationale: Option<String>,
}

async fn create_hook(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<CreateHookRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    require_superadmin(&auth)?;
    let hook = crate::api::integration_hooks::create_hook(
        &ctx,
        &auth.did,
        &req.name,
        &req.url,
        &req.event_classes,
        req.description.as_deref(),
        req.enabled,
        req.rationale.as_deref(),
    )
    .await
    .map_err(rule_err)?;
    Ok(Json(serde_json::json!({ "hook": hook })))
}

#[derive(Deserialize)]
struct EditHookRequest {
    id: String,
    expected_last_modified_at: String,
    name: String,
    url: String,
    event_classes: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    rationale: Option<String>,
}

async fn edit_hook(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<EditHookRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    require_superadmin(&auth)?;
    crate::api::integration_hooks::edit_hook(
        &ctx,
        &auth.did,
        &req.id,
        &req.expected_last_modified_at,
        &req.name,
        &req.url,
        &req.event_classes,
        req.description.as_deref(),
        req.enabled,
        req.rationale.as_deref(),
    )
    .await
    .map_err(rule_err)?;
    Ok(Json(serde_json::json!({ "success": true, "id": req.id })))
}

async fn delete_hook(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<DeleteAutoLabelRuleRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    require_superadmin(&auth)?;
    crate::api::integration_hooks::delete_hook(&ctx, &auth.did, &req.id)
        .await
        .map_err(rule_err)?;
    Ok(Json(serde_json::json!({ "success": true, "id": req.id })))
}

async fn list_hooks(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Query(q): Query<ListAutoLabelRulesQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    require_superadmin(&auth)?;
    let hooks = crate::api::integration_hooks::list_hooks(&ctx, q.include_deleted)
        .await
        .map_err(rule_err)?;
    Ok(Json(serde_json::json!({ "hooks": hooks })))
}

async fn get_integration_hooks_state(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    require_superadmin(&auth)?;
    let state = crate::api::integration_hooks::integration_hooks_state(&ctx)
        .await
        .map_err(rule_err)?;
    Ok(Json(state))
}

// ---------------------------------------------------------------------------
// Per-operator session management (§8.1.7 / #273)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ListSessionsQuery {
    /// SuperAdmin only: scope the listing to one operator. Omitted by a
    /// SuperAdmin lists ALL operators' sessions; supplied (or any value) by
    /// a non-SuperAdmin is forced to their own did.
    did: Option<String>,
    #[serde(flatten)]
    pagination: PaginationParams,
}

/// `GET /xrpc/tools.aurora.admin.listSessions` — active operator sessions
/// (#273). Self-service for any operator (their own logins); SuperAdmin
/// overview across all operators. Returns newest-first, keyset-paginated;
/// each row flags `isCurrent` by matching the caller's own session id.
async fn list_sessions(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Query(q): Query<ListSessionsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;

    let is_superadmin = auth.role.can_act_as(Role::SuperAdmin);
    let limit = q.pagination.effective_limit();
    let cursor = match &q.pagination.cursor {
        Some(s) => Some(SessionCursor::decode(s).map_err(|_| {
            json_error(StatusCode::BAD_REQUEST, "OutdatedCursor", "invalid cursor")
        })?),
        None => None,
    };

    // Authorization + listing scope:
    //   SuperAdmin + no did → all operators; SuperAdmin + did → that operator;
    //   non-SuperAdmin → own sessions only (a foreign did is forbidden).
    let result = if is_superadmin {
        match &q.did {
            Some(d) => ctx.operator_session_store.list_by_did(d, limit, cursor).await,
            None => ctx.operator_session_store.list_all(limit, cursor).await,
        }
    } else {
        if let Some(d) = &q.did {
            if d != &auth.did {
                return Err(json_error(
                    StatusCode::FORBIDDEN,
                    "Forbidden",
                    "listing another operator's sessions requires SuperAdmin",
                ));
            }
        }
        ctx.operator_session_store
            .list_by_did(&auth.did, limit, cursor)
            .await
    };
    let (sessions, next) = result.map_err(|e| {
        json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
    })?;

    let current_sid = &auth.session.session_id;
    let items: Vec<serde_json::Value> = sessions
        .iter()
        .map(|s| {
            serde_json::json!({
                "sid": s.id,
                "did": s.did,
                "createdAt": s.created_at.to_rfc3339(),
                "lastActiveAt": s.last_active_at.to_rfc3339(),
                "expiresAt": s.expires_at.to_rfc3339(),
                "sourceIp": s.source_ip,
                "userAgent": s.user_agent,
                "isCurrent": &s.id == current_sid,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "sessions": items, "cursor": next })))
}

#[derive(Deserialize)]
struct RevokeSessionRequest {
    /// The session id (`sid`) to force-logout.
    sid: String,
    /// Required when revoking ANOTHER operator's session (a security event);
    /// optional for self-service logout.
    rationale: Option<String>,
}

#[derive(Serialize, Debug)]
struct RevokeSessionOutput {
    success: bool,
    sid: String,
    #[serde(rename = "auditEntryId")]
    audit_entry_id: String,
}

/// `POST /xrpc/tools.aurora.admin.revokeSession` — force-logout a session
/// (#273). Any operator may revoke their OWN session; SuperAdmin may revoke
/// any. The #271 per-request gate rejects the revoked session on its next
/// request, so the operator reauthenticates. Emits an audit-chain entry
/// (`session.revoke` for a cross-operator force-logout, `session.revoke_self`
/// for self-service) in the same transaction as the flag flip.
async fn revoke_session(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RevokeSessionRequest>,
) -> Result<Json<RevokeSessionOutput>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;

    // Resolve the target's owner to authorize + audit before mutating.
    let target = ctx
        .operator_session_store
        .get(&req.sid)
        .await
        .map_err(|e| {
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
        })?
        .ok_or_else(|| json_error(StatusCode::NOT_FOUND, "NotFound", "session not found"))?;

    let is_self = target.did == auth.did;
    if !is_self && !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "revoking another operator's session requires SuperAdmin",
        ));
    }

    let rationale = req
        .rationale
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    // A cross-operator force-logout is a security event — require a reason.
    if !is_self && rationale.is_none() {
        return Err(json_error(
            StatusCode::BAD_REQUEST,
            "InvalidRequest",
            "rationale-required",
        ));
    }
    let action = if is_self {
        "session.revoke_self"
    } else {
        "session.revoke"
    };
    let audit_rationale = format!(
        "{} (session {})",
        rationale.unwrap_or("operator self-service session revocation"),
        req.sid
    );
    let subject = Subject::Repo {
        did: target.did.clone(),
    };
    let now = chrono::Utc::now();

    // Flip + audit atomically (LB-1 pattern; mirrors revoke_role).
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx.account_db.begin().await.map_err(|e| {
        json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
    })?;
    OperatorSessionStore::revoke_in_tx(&mut tx, &req.sid, &auth.did, rationale, now)
        .await
        .map_err(|e| match e {
            PdsError::NotFound(_) => json_error(
                StatusCode::NOT_FOUND,
                "NotFound",
                "session not found or already revoked",
            ),
            other => json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError",
                other.to_string(),
            ),
        })?;
    let audit_entry_id = audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action,
            subject: Some(&subject),
            rationale: &audit_rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| {
        json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
    })?;
    tx.commit().await.map_err(|e| {
        json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
    })?;

    Ok(Json(RevokeSessionOutput {
        success: true,
        sid: req.sid,
        audit_entry_id: audit_entry_id.to_string(),
    }))
}

#[derive(Deserialize)]
struct RevokeOperatorSessionsRequest {
    /// The operator DID whose active sessions to force-logout.
    did: String,
    /// Required — a bulk force-logout of an operator is always a security event.
    rationale: Option<String>,
}

#[derive(Serialize, Debug)]
struct RevokeOperatorSessionsOutput {
    success: bool,
    did: String,
    /// Count of active sessions revoked (0 when the operator had none active).
    revoked: u64,
    #[serde(rename = "auditEntryId")]
    audit_entry_id: String,
}

/// `POST /xrpc/tools.aurora.admin.revokeOperatorSessions` — bulk force-logout of
/// EVERY active session for one operator (#338), for compromise / departure
/// response. SuperAdmin-only: revoking another operator's sessions
/// deployment-wide is escalation-of-trust territory. Rationale required (a
/// security event). Revokes all of `did`'s active sessions — the #271
/// per-request gate then rejects each on its next request — and audits the bulk
/// action with the count, in one transaction (the revoke_role / revoke_session
/// pattern). A zero count (operator had no active sessions) is still a success
/// and still audited, for forensic completeness. Targeting one's own DID is
/// permitted (it logs the caller out everywhere, current session included —
/// recoverable by re-login); session revocation never touches roles, so there
/// is no last-SuperAdmin lockout to guard (that guard is for role revocation).
async fn revoke_operator_sessions(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RevokeOperatorSessionsRequest>,
) -> Result<Json<RevokeOperatorSessionsOutput>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;

    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "bulk session revocation requires SuperAdmin",
        ));
    }
    let rationale = req
        .rationale
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| json_error(StatusCode::BAD_REQUEST, "InvalidRequest", "rationale-required"))?;
    let did = req.did.trim();
    if did.is_empty() {
        return Err(json_error(StatusCode::BAD_REQUEST, "InvalidRequest", "did-required"));
    }
    let subject = Subject::Repo { did: did.to_string() };
    let now = chrono::Utc::now();

    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx.account_db.begin().await.map_err(|e| {
        json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
    })?;
    let count = OperatorSessionStore::revoke_all_for_did_in_tx(
        &mut tx,
        did,
        &auth.did,
        Some(rationale),
        now,
    )
    .await
    .map_err(|e| {
        json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
    })?;
    let audit_rationale = format!("{} (bulk revoke: {} session(s))", rationale, count);
    let audit_entry_id = audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "session.revoke_all",
            subject: Some(&subject),
            rationale: &audit_rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| {
        json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
    })?;
    tx.commit().await.map_err(|e| {
        json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
    })?;

    Ok(Json(RevokeOperatorSessionsOutput {
        success: true,
        did: did.to_string(),
        revoked: count,
        audit_entry_id: audit_entry_id.to_string(),
    }))
}

// ---------------------------------------------------------------------------
// Repository rebuild — preflight (§7.4.1 / #286)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PreRebuildCheckParams {
    did: String,
    /// When `true`, additionally reconstruct the repo in memory from the full
    /// sequencer history and run it through proto-blue's `verify_repo`, reporting
    /// whether replay yields a coherent repo (`deepVerified`). Structural-only
    /// (no signature check) — that, plus the destructive swap, is #290. Off by
    /// default because reconstruction reads and decodes every commit's CAR slice.
    #[serde(default)]
    deep: bool,
}

/// `GET /xrpc/tools.aurora.superadmin.preRebuildCheck?did=<did>[&deep=true]` —
/// non-destructive repo-rebuild preflight (§7.4.1 / #286, #289). Walks the
/// account's full commit history and reports what a rebuild would reconstruct
/// (commit count, net live record count, the rev range, the head commit CID) so
/// a SuperAdmin can confirm scope before triggering the destructive rebuild
/// (#290). With `deep=true` it goes further: it reconstructs the repo in memory
/// and verifies it resolves via `verify_repo` — the same correctness gate the
/// destructive rebuild will use — so a failed reconstruction is surfaced as a
/// diagnostic *before* any swap. Read-only either way; touches no repo state.
async fn pre_rebuild_check(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Query(params): Query<PreRebuildCheckParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!(
                "preRebuildCheck requires SuperAdmin role; have {}",
                auth.role.as_str()
            ),
        ));
    }
    let pf = match ctx.sequencer.rebuild_preflight(&params.did).await {
        Ok(Some(pf)) => pf,
        Ok(None) => {
            return Err(json_error(
                StatusCode::NOT_FOUND,
                "NotFound",
                format!("no sequencer history for {} — nothing to rebuild", params.did),
            ))
        }
        Err(e) => {
            return Err(json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError",
                e.to_string(),
            ))
        }
    };

    let mut body = serde_json::json!({
        "did": params.did,
        "commitCount": pf.commit_count,
        "recordCount": pf.record_count,
        "creates": pf.creates,
        "deletes": pf.deletes,
        "headCommitCid": pf.head_commit_cid,
        "headRev": pf.head_rev,
        "firstRev": pf.first_rev,
    });

    if params.deep {
        let obj = body.as_object_mut().expect("json! built a map above");
        // Structural-only (signing_did_key = None); full signature verification
        // is part of the destructive rebuild (#290).
        match crate::rebuild::reconstruct_and_verify(&ctx.sequencer, &params.did, None).await {
            Ok(Some(vr)) => {
                obj.insert("deepVerified".into(), serde_json::Value::Bool(true));
                obj.insert(
                    "reconstructedHeadCid".into(),
                    serde_json::Value::String(vr.commit_cid.to_string()),
                );
                obj.insert(
                    "reconstructedRev".into(),
                    serde_json::Value::String(vr.rev().to_string()),
                );
            }
            // No history despite a preflight: treat as not-verifiable rather than 500.
            Ok(None) => {
                obj.insert("deepVerified".into(), serde_json::Value::Bool(false));
            }
            // Reconstruction failed verification — a real diagnostic, not a server
            // fault: replay would NOT produce a coherent repo. Report, don't 500.
            Err(e) => {
                obj.insert("deepVerified".into(), serde_json::Value::Bool(false));
                obj.insert("deepError".into(), serde_json::Value::String(e.to_string()));
            }
        }
    }

    Ok(Json(body))
}

// ---------------------------------------------------------------------------
// Repository rebuild — destructive surface (§7.4.1 / #290)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RebuildRepoRequest {
    did: String,
    /// Operator rationale — required (high-impact destructive action). Carried
    /// into the `RepoRebuilt` audit event on a successful swap.
    #[serde(default)]
    rationale: Option<String>,
}

/// `POST /xrpc/tools.aurora.superadmin.rebuildRepo` — start a background
/// repository rebuild for `did` (§7.4.1 / #290). Reconstructs the account's
/// repo from its full sequencer history in memory, verifies it (full signature
/// check) via `verify_repo`, and atomically swaps it into live storage in one
/// per-DID transaction. Returns the job-id to poll via `getRebuildProgress`.
///
/// Per-DID single-flight: a 409 if a rebuild is already in flight for the same
/// DID. SuperAdmin only; `rationale` required. The original repo is untouched
/// unless and until the atomic swap commits, so a failed/cancelled rebuild is
/// safe (shadow-then-swap). A no-history account surfaces as a `failed` job via
/// `getRebuildProgress` (run `preRebuildCheck` first to confirm scope).
async fn rebuild_repo(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RebuildRepoRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!(
                "rebuildRepo requires SuperAdmin role; have {}",
                auth.role.as_str()
            ),
        ));
    }
    let rationale = req
        .rationale
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            json_error(StatusCode::BAD_REQUEST, "InvalidRequest", "rationale-required")
        })?;

    let registry = ctx.rebuild_registry.clone();
    let job_id = registry
        .start(
            ctx.clone(),
            req.did.clone(),
            auth.did.clone(),
            rationale.to_string(),
        )
        .map_err(|e| match e {
            PdsError::Conflict(msg) => json_error(StatusCode::CONFLICT, "Conflict", msg),
            other => json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError",
                other.to_string(),
            ),
        })?;

    Ok(Json(serde_json::json!({
        "jobId": job_id,
        "did": req.did,
        "status": "started",
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetRebuildProgressParams {
    job_id: String,
}

/// Render a [`RebuildProgress`](crate::rebuild::RebuildProgress) as the
/// `getRebuildProgress` wire shape (camelCase; `SystemTime`s as unix millis).
fn rebuild_progress_json(p: &crate::rebuild::RebuildProgress) -> serde_json::Value {
    fn ms(t: Option<std::time::SystemTime>) -> Option<u64> {
        t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
    }
    serde_json::json!({
        "jobId": p.job_id,
        "did": p.did,
        "phase": p.phase.as_str(),
        "commitsTotal": p.commits_total,
        "commitsProcessed": p.commits_processed,
        "recordsWritten": p.records_written,
        "headCommitCidBefore": p.head_before,
        "headCommitCidAfter": p.head_after,
        "error": p.error,
        "cancelRequested": p.cancel_requested,
        "startedAt": ms(p.started_at),
        "finishedAt": ms(p.finished_at),
    })
}

/// `GET /xrpc/tools.aurora.superadmin.getRebuildProgress?jobId=<id>` — progress
/// for a rebuild job (§7.4.1 / #290): phase (walking / verifying / swapping /
/// completed / failed / cancelled), commits walked vs total, records written,
/// head CID before/after, and any failure diagnostic. SuperAdmin only; 404 on
/// an unknown job-id.
async fn get_rebuild_progress(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Query(params): Query<GetRebuildProgressParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!(
                "getRebuildProgress requires SuperAdmin role; have {}",
                auth.role.as_str()
            ),
        ));
    }
    match ctx.rebuild_registry.progress(&params.job_id) {
        Some(p) => Ok(Json(rebuild_progress_json(&p))),
        None => Err(json_error(
            StatusCode::NOT_FOUND,
            "NotFound",
            format!("no rebuild job {}", params.job_id),
        )),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelRebuildRequest {
    job_id: String,
}

/// `POST /xrpc/tools.aurora.superadmin.cancelRebuild` — request cancellation of
/// an in-flight rebuild (§7.4.1 / #290). Cancellation is observed at commit
/// boundaries and between phases; the atomic swap is the point of no return, so
/// a cancel that arrives during the swap is a no-op (the transaction is whole).
/// SuperAdmin only. 404 on an unknown job-id; 409 if the job already reached a
/// terminal phase ("nothing to cancel").
async fn cancel_rebuild(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<CancelRebuildRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!(
                "cancelRebuild requires SuperAdmin role; have {}",
                auth.role.as_str()
            ),
        ));
    }
    match ctx.rebuild_registry.request_cancel(&req.job_id) {
        None => Err(json_error(
            StatusCode::NOT_FOUND,
            "NotFound",
            format!("no rebuild job {}", req.job_id),
        )),
        Some(true) => Ok(Json(serde_json::json!({
            "jobId": req.job_id,
            "status": "cancelling",
        }))),
        Some(false) => Err(json_error(
            StatusCode::CONFLICT,
            "Conflict",
            "rebuild job already complete; nothing to cancel".to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Bulk repository repair — scan substrate (§7.4.3 / #291)
// ---------------------------------------------------------------------------

/// `POST /xrpc/tools.aurora.superadmin.scanReposForInconsistencies` — start a
/// background across-accounts scan (§7.4.3 / #291). Walks every account,
/// structurally reconstructs its repo from the sequencer, and persists any
/// repo-vs-sequencer inconsistency as a finding (reviewable via
/// getRepoScanResults). Read-only — it detects, it does not repair (#292).
/// Deployment single-flight (409 if a scan is already running). SuperAdmin.
async fn scan_repos_for_inconsistencies(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!(
                "scanReposForInconsistencies requires SuperAdmin role; have {}",
                auth.role.as_str()
            ),
        ));
    }
    let job = ctx.repo_scan_job.clone();
    match job.try_start(ctx.clone(), auth.did.clone()) {
        Some(scan_id) => Ok(Json(serde_json::json!({
            "scanId": scan_id,
            "status": "started",
        }))),
        None => Err(json_error(
            StatusCode::CONFLICT,
            "Conflict",
            "a repository scan is already in progress".to_string(),
        )),
    }
}

/// Render a [`ScanProgress`](crate::repo_scan::ScanProgress) as the
/// `getScanProgress` wire shape.
fn scan_progress_json(p: &crate::repo_scan::ScanProgress) -> serde_json::Value {
    fn ms(t: Option<std::time::SystemTime>) -> Option<u64> {
        t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
    }
    serde_json::json!({
        "running": p.running,
        "scanId": p.scan_id,
        "accountsScanned": p.accounts_scanned,
        "findingsHigh": p.counts.high,
        "findingsMedium": p.counts.medium,
        "findingsLow": p.counts.low,
        "findingsTotal": p.counts.total(),
        "startedAt": ms(p.started_at),
        "finishedAt": ms(p.finished_at),
        "cancelRequested": p.cancel_requested,
        "lastOutcome": p.last_outcome,
    })
}

/// `GET /xrpc/tools.aurora.superadmin.getScanProgress` — live progress of the
/// repository scan (§7.4.3 / #291): running flag, accounts scanned, the
/// severity breakdown so far, and the last run's outcome. SuperAdmin.
async fn get_scan_progress(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!("getScanProgress requires SuperAdmin role; have {}", auth.role.as_str()),
        ));
    }
    Ok(Json(scan_progress_json(&ctx.repo_scan_job.progress())))
}

/// `POST /xrpc/tools.aurora.superadmin.cancelScan` — request cancellation of
/// the in-flight scan (§7.4.3 / #291); the walk stops at the next account
/// boundary. SuperAdmin. 409 if no scan is in progress.
async fn cancel_scan(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!("cancelScan requires SuperAdmin role; have {}", auth.role.as_str()),
        ));
    }
    if ctx.repo_scan_job.request_cancel() {
        Ok(Json(serde_json::json!({ "status": "cancelling" })))
    } else {
        Err(json_error(
            StatusCode::CONFLICT,
            "Conflict",
            "no repository scan is in progress".to_string(),
        ))
    }
}

#[derive(Deserialize)]
struct GetRepoScanResultsParams {
    /// Filter by severity (`high` | `medium` | `low`); omitted = all.
    severity: Option<String>,
    limit: Option<i64>,
    /// Keyset cursor: the last did from the previous page.
    cursor: Option<String>,
}

/// `GET /xrpc/tools.aurora.superadmin.getRepoScanResults` — the latest scan's
/// findings (§7.4.3 / #291), optionally filtered by severity, keyset-paginated
/// by did, with the full severity breakdown. SuperAdmin.
async fn get_repo_scan_results(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Query(params): Query<GetRepoScanResultsParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!("getRepoScanResults requires SuperAdmin role; have {}", auth.role.as_str()),
        ));
    }
    let severity = match params.severity.as_deref() {
        None => None,
        Some(s) => match s.parse::<crate::repo_scan::Severity>() {
            Ok(sev) => Some(sev),
            Err(e) => return Err(json_error(StatusCode::BAD_REQUEST, "InvalidRequest", e.to_string())),
        },
    };
    let limit = params.limit.unwrap_or(100).clamp(1, 500);
    let findings = ctx
        .scan_findings_store
        .list(severity, limit, params.cursor.as_deref())
        .await
        .map_err(|e| json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string()))?;
    let counts = ctx
        .scan_findings_store
        .counts()
        .await
        .map_err(|e| json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string()))?;
    let next_cursor = if findings.len() as i64 == limit {
        findings.last().map(|f| f.did.clone())
    } else {
        None
    };
    let items: Vec<serde_json::Value> = findings
        .iter()
        .map(|f| {
            serde_json::json!({
                "did": f.did,
                "severity": f.severity.as_str(),
                "liveHead": f.live_head,
                "reconstructedHead": f.recon_head,
                "detail": f.detail,
                "scanId": f.scan_id,
                "createdAt": f.created_at,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({
        "findings": items,
        "counts": {
            "high": counts.high,
            "medium": counts.medium,
            "low": counts.low,
            "total": counts.total(),
        },
        "cursor": next_cursor,
    })))
}

// ---------------------------------------------------------------------------
// Bulk repository repair — repair substrate (§7.4.3 / #292)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RepairReposRequest {
    /// Explicit target DID subset. Ignored when `all` is true.
    #[serde(default)]
    dids: Vec<String>,
    /// Repair every account in the current scan findings.
    #[serde(default)]
    all: bool,
    /// Operator rationale — required (high-impact destructive action); carried
    /// into the BulkRepairInitiated envelope and each per-account RepoRebuilt.
    #[serde(default)]
    rationale: Option<String>,
}

/// `POST /xrpc/tools.aurora.superadmin.repairRepos` — start a bulk repair over
/// a set of accounts (§7.4.3 / #292): `all` = every account in the current scan
/// findings, or an explicit `dids` subset. Each target is rebuilt via the same
/// per-account machinery as `rebuildRepo` (single-flight, RepoRebuilt audit); a
/// per-account failure or conflict is skipped, not fatal. Deployment
/// single-flight (409 if a bulk repair is already running). SuperAdmin;
/// rationale required.
async fn repair_repos(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RepairReposRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!("repairRepos requires SuperAdmin role; have {}", auth.role.as_str()),
        ));
    }
    let rationale = req
        .rationale
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| json_error(StatusCode::BAD_REQUEST, "InvalidRequest", "rationale-required"))?;

    // Resolve the target DID set: `all` = every current finding, else the
    // explicit subset.
    let targets = if req.all {
        ctx.scan_findings_store.all_dids().await.map_err(|e| {
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
        })?
    } else {
        req.dids.clone()
    };
    if targets.is_empty() {
        return Err(json_error(
            StatusCode::BAD_REQUEST,
            "InvalidRequest",
            "no repair targets — pass `dids` or `all:true` with a non-empty findings set".to_string(),
        ));
    }

    let job = ctx.bulk_repair_job.clone();
    let target_count = targets.len();
    match job.try_start(ctx.clone(), targets, auth.did.clone(), rationale.to_string()) {
        Some(batch_id) => Ok(Json(serde_json::json!({
            "batchId": batch_id,
            "targetCount": target_count,
            "status": "started",
        }))),
        None => Err(json_error(
            StatusCode::CONFLICT,
            "Conflict",
            "a bulk repository repair is already in progress".to_string(),
        )),
    }
}

/// Render a [`BulkRepairProgress`](crate::repo_scan::BulkRepairProgress) as the
/// `getBulkRepairProgress` wire shape.
fn bulk_repair_progress_json(p: &crate::repo_scan::BulkRepairProgress) -> serde_json::Value {
    fn ms(t: Option<std::time::SystemTime>) -> Option<u64> {
        t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
    }
    serde_json::json!({
        "running": p.running,
        "batchId": p.batch_id,
        "targetsTotal": p.targets_total,
        "processed": p.processed,
        "repaired": p.repaired,
        "skipped": p.skipped,
        "failed": p.failed,
        "currentDid": p.current_did,
        "startedAt": ms(p.started_at),
        "finishedAt": ms(p.finished_at),
        "cancelRequested": p.cancel_requested,
        "lastOutcome": p.last_outcome,
    })
}

/// `GET /xrpc/tools.aurora.superadmin.getBulkRepairProgress` — live progress of
/// the bulk repair (§7.4.3 / #292): targets total, processed, the
/// repaired/skipped/failed tally, the current account, and the last outcome.
/// SuperAdmin.
async fn get_bulk_repair_progress(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!("getBulkRepairProgress requires SuperAdmin role; have {}", auth.role.as_str()),
        ));
    }
    Ok(Json(bulk_repair_progress_json(&ctx.bulk_repair_job.progress())))
}

/// `POST /xrpc/tools.aurora.superadmin.cancelBulkRepair` — request cancellation
/// of the in-flight bulk repair (§7.4.3 / #292); the loop stops before the next
/// account. A per-account rebuild already in flight finishes atomically.
/// SuperAdmin. 409 if no bulk repair is in progress.
async fn cancel_bulk_repair(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!("cancelBulkRepair requires SuperAdmin role; have {}", auth.role.as_str()),
        ));
    }
    if ctx.bulk_repair_job.request_cancel() {
        Ok(Json(serde_json::json!({ "status": "cancelling" })))
    } else {
        Err(json_error(
            StatusCode::CONFLICT,
            "Conflict",
            "no bulk repository repair is in progress".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Sequencer recovery — §7.4.2 / #294 (escalation surface; one operation:
// read-only deep integrity validation)
// ---------------------------------------------------------------------------

/// `GET /xrpc/tools.aurora.superadmin.sequencerRecoveryOptions` — the current
/// sequencer state (row counts, head/min seq) plus the recovery operations
/// available given that state (§7.4.2 / #294). v0.9 offers one operation,
/// `validate` (read-only deep integrity validation). SuperAdmin.
async fn sequencer_recovery_options(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!("sequencerRecoveryOptions requires SuperAdmin role; have {}", auth.role.as_str()),
        ));
    }
    let counts = ctx.sequencer.integrity_counts().await.map_err(|e| {
        json_error(StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", e.to_string())
    })?;
    let last = ctx.sequencer_recovery_job.progress();
    let last_validation = last.report.as_ref().map(|r| {
        serde_json::json!({
            "outcome": last.last_outcome,
            "malformedCount": r.malformed_count,
            "nonMonotonicCount": r.non_monotonic_count,
            "rowsScanned": r.rows_scanned,
        })
    });
    Ok(Json(serde_json::json!({
        "state": {
            "totalRows": counts.total_rows,
            "invalidatedRows": counts.invalidated_rows,
            "headSeq": counts.head_seq,
            "minSeq": counts.min_seq,
        },
        "operations": [
            {
                "id": crate::sequencer_recovery::OP_VALIDATE,
                "label": "Deep integrity validation",
                "destructive": false,
                "available": true,
                "description": "Walk the live sequencer log, decoding every event \
                    and checking per-DID rev monotonicity. Read-only.",
            }
        ],
        "running": last.running,
        "lastValidation": last_validation,
    })))
}

#[derive(Deserialize)]
struct RunSequencerRecoveryRequest {
    operation: String,
    /// Accepted but unused for the read-only `validate` operation; reserved for
    /// future destructive recovery operations.
    #[serde(default)]
    #[allow(dead_code)]
    rationale: Option<String>,
}

/// `POST /xrpc/tools.aurora.superadmin.runSequencerRecovery` — dispatch a
/// sequencer recovery operation (§7.4.2 / #294). v0.9 accepts `operation:
/// "validate"` (read-only). Deployment single-flight (409 if one is already
/// running). SuperAdmin.
async fn run_sequencer_recovery(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RunSequencerRecoveryRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!("runSequencerRecovery requires SuperAdmin role; have {}", auth.role.as_str()),
        ));
    }
    match req.operation.as_str() {
        crate::sequencer_recovery::OP_VALIDATE => {
            let job = ctx.sequencer_recovery_job.clone();
            match job.try_start_validate(ctx.clone(), auth.did.clone()) {
                Some(job_id) => Ok(Json(serde_json::json!({
                    "jobId": job_id,
                    "operation": crate::sequencer_recovery::OP_VALIDATE,
                    "status": "started",
                }))),
                None => Err(json_error(
                    StatusCode::CONFLICT,
                    "Conflict",
                    "a sequencer recovery operation is already in progress".to_string(),
                )),
            }
        }
        other => Err(json_error(
            StatusCode::BAD_REQUEST,
            "InvalidRequest",
            format!("unknown or unavailable recovery operation: {other}"),
        )),
    }
}

/// Render a [`RecoveryProgress`](crate::sequencer_recovery::RecoveryProgress) as
/// the `getSequencerRecoveryProgress` wire shape.
fn sequencer_recovery_progress_json(
    p: &crate::sequencer_recovery::RecoveryProgress,
) -> serde_json::Value {
    fn ms(t: Option<std::time::SystemTime>) -> Option<u64> {
        t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
    }
    let report = p.report.as_ref().map(|r| {
        serde_json::json!({
            "totalRows": r.total_rows,
            "invalidatedRows": r.invalidated_rows,
            "headSeq": r.head_seq,
            "minSeq": r.min_seq,
            "rowsScanned": r.rows_scanned,
            "malformedCount": r.malformed_count,
            "malformed": r.malformed.iter().map(|m| serde_json::json!({
                "seq": m.seq, "did": m.did, "eventType": m.event_type,
            })).collect::<Vec<_>>(),
            "nonMonotonicCount": r.non_monotonic_count,
            "nonMonotonic": r.non_monotonic.iter().map(|n| serde_json::json!({
                "did": n.did, "seq": n.seq, "rev": n.rev, "prevRev": n.prev_rev,
            })).collect::<Vec<_>>(),
        })
    });
    serde_json::json!({
        "running": p.running,
        "operation": p.operation,
        "jobId": p.job_id,
        "rowsScanned": p.rows_scanned,
        "startedAt": ms(p.started_at),
        "finishedAt": ms(p.finished_at),
        "cancelRequested": p.cancel_requested,
        "lastOutcome": p.last_outcome,
        "error": p.error,
        "report": report,
    })
}

/// `GET /xrpc/tools.aurora.superadmin.getSequencerRecoveryProgress` — live
/// progress + the validation report once complete (§7.4.2 / #294). SuperAdmin.
async fn get_sequencer_recovery_progress(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!("getSequencerRecoveryProgress requires SuperAdmin role; have {}", auth.role.as_str()),
        ));
    }
    Ok(Json(sequencer_recovery_progress_json(
        &ctx.sequencer_recovery_job.progress(),
    )))
}

/// `POST /xrpc/tools.aurora.superadmin.cancelSequencerRecovery` — request
/// cancellation of the in-flight recovery operation (§7.4.2 / #294); the walk
/// stops at the next page boundary and returns its partial report. SuperAdmin.
/// 409 if none is in progress.
async fn cancel_sequencer_recovery(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "Forbidden",
            format!("cancelSequencerRecovery requires SuperAdmin role; have {}", auth.role.as_str()),
        ));
    }
    if ctx.sequencer_recovery_job.request_cancel() {
        Ok(Json(serde_json::json!({ "status": "cancelling" })))
    } else {
        Err(json_error(
            StatusCode::CONFLICT,
            "Conflict",
            "no sequencer recovery operation is in progress".to_string(),
        ))
    }
}

#[derive(Deserialize)]
struct ListRolesQuery {
    did: Option<String>,
}

/// List admin roles
async fn list_roles(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<ListRolesQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if let Some(did) = query.did {
        // Get role for specific user
        let role_record = ctx
            .admin_role_manager
            .get_role(&did)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        Ok(Json(serde_json::json!({
            "did": did,
            "role": role_record,
        })))
    } else {
        // List all active role assignments
        let assignments = ctx
            .admin_role_manager
            .list_active_roles()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        Ok(Json(serde_json::json!({
            "roles": assignments,
        })))
    }
}

// ============================================================================
// Account Management Endpoints
// ============================================================================

/// Resolve the spec's `account` (at-identifier) field and Aurora's legacy
/// `did` field down to a canonical DID. Established by Phase 1.7 (chainlink
/// #62) and reused across the deprecation-alias rollout.
///
/// Behavior:
/// - exactly-one validation: providing both or neither returns 400
/// - `account`: if DID-form, returned as-is; if handle-form, resolved via
///   the local actor table (no external DNS/.well-known resolution, which
///   would be wrong for admin operations on local users)
/// - `did` (legacy): DID-form only, retains the historical behavior
///
/// Note: spec for `disableAccountInvites` and `enableAccountInvites` declares
/// `account` as `format=did`, while `updateAccountEmail` declares it as
/// `format=at-identifier`. This helper uniformly accepts either form on the
/// `account` field — spec-compliant clients that only ever pass DID still
/// work; operators that pass handles to the invites endpoints get a more
/// permissive (non-rejecting) experience than strict spec.
async fn resolve_account_or_did(
    ctx: &AppContext,
    account: Option<&str>,
    did: Option<&str>,
) -> Result<String, (StatusCode, String)> {
    match (account, did) {
        (Some(_), Some(_)) => Err((
            StatusCode::BAD_REQUEST,
            "Provide exactly one of `account` or `did` (legacy)".to_string(),
        )),
        (None, None) => Err((
            StatusCode::BAD_REQUEST,
            "Missing required field: `account`".to_string(),
        )),
        (Some(at_id), None) => ctx
            .account_manager
            .resolve_at_identifier_to_did(at_id)
            .await
            .map_err(|e| {
                if matches!(e, PdsError::NotFound(_)) {
                    (
                        StatusCode::NOT_FOUND,
                        format!("Account not found for identifier: {}", at_id),
                    )
                } else {
                    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                }
            }),
        (None, Some(did_str)) => {
            if !did_str.starts_with("did:") {
                return Err((StatusCode::BAD_REQUEST, "Invalid DID format".to_string()));
            }
            Ok(did_str.to_string())
        }
    }
}

#[derive(Deserialize)]
struct UpdateAccountEmailRequest {
    /// Account at-identifier (handle or DID) per the lexicon. Required if
    /// the legacy `did` field is not provided.
    #[serde(default)]
    account: Option<String>,
    /// DEPRECATED: legacy `did` field retained for back-compat. Use
    /// `account` instead. Continues to accept DID-form only. To be
    /// removed in a later minor version.
    #[serde(default)]
    did: Option<String>,
    /// New email address
    email: String,
}

/// Update account email address
async fn update_account_email(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<UpdateAccountEmailRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let canonical_did =
        resolve_account_or_did(&ctx, req.account.as_deref(), req.did.as_deref()).await?;

    // v0.8 arc 3 (#184) — reject ':' in the email (see
    // AccountManager::validate_email). Separate guard before the existing
    // check so the charset-specific message fires (M-5).
    if req.email.contains(':') {
        return Err((
            StatusCode::BAD_REQUEST,
            "Email address must not contain ':'".to_string(),
        ));
    }

    if !req.email.contains('@') || req.email.len() < 5 {
        return Err((StatusCode::BAD_REQUEST, "Invalid email format".to_string()));
    }

    let subject = Subject::Repo {
        did: canonical_did.clone(),
    };
    // Snapshot the pre-mutation state outside the tx — the
    // snapshot is immutable evidence of state-at-decision; a
    // vestigial snapshot row that doesn't end up referenced by a
    // chain row is harmless.
    let snapshot_id = audit_chain::capture_snapshot(&ctx.account_db, &subject)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let rationale = format!("change email to {}", req.email);

    // LB-1 / chainlink #128: the email UPDATE and chain entry land
    // in one transaction. Multi-store side effects (token store
    // invalidation, queueing a confirmation email) remain
    // post-commit best-effort per §3.4 reading — the LB-1
    // commitment scopes to chain-entry atomicity with the primary
    // `account` table mutation. See update_email_in_tx's doc
    // comment for the boundary.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::account::AccountManager::update_email_in_tx(&mut tx, &canonical_did, &req.email)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    format!("Account not found: {}", canonical_did),
                )
            } else if matches!(e, PdsError::Validation(_)) {
                (StatusCode::CONFLICT, e.to_string())
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "account.update_email",
            subject: Some(&subject),
            rationale: &rationale,
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
struct UpdateAccountHandleRequest {
    /// Account DID
    did: String,
    /// New handle
    handle: String,
}

/// Update account handle
async fn update_account_handle(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<UpdateAccountHandleRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Validate DID format
    if !req.did.starts_with("did:") {
        return Err((StatusCode::BAD_REQUEST, "Invalid DID format".to_string()));
    }

    // Validate handle format (basic check)
    if req.handle.is_empty() || req.handle.len() > 253 {
        return Err((StatusCode::BAD_REQUEST, "Invalid handle format".to_string()));
    }

    let subject = Subject::Repo {
        did: req.did.clone(),
    };
    let snapshot_id = audit_chain::capture_snapshot(&ctx.account_db, &subject)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let rationale = format!("change handle to {}", req.handle);

    // LB-1 Session 12 / chainlink #129: handle UPDATE + chain entry
    // in one transaction so a crash between the two leaves neither row.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::account::AccountManager::update_handle_in_tx(&mut tx, &req.did, &req.handle)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    format!("Account not found: {}", req.did),
                )
            } else if matches!(e, PdsError::Validation(_) | PdsError::Conflict(_)) {
                (StatusCode::CONFLICT, e.to_string())
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "account.update_handle",
            subject: Some(&subject),
            rationale: &rationale,
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
struct UpdateAccountPasswordRequest {
    /// Account DID
    did: String,
    /// New password
    password: String,
}

/// Update account password (admin override)
async fn update_account_password(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<UpdateAccountPasswordRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Validate DID format
    if !req.did.starts_with("did:") {
        return Err((StatusCode::BAD_REQUEST, "Invalid DID format".to_string()));
    }

    // Validate password (minimum length)
    if req.password.len() < 8 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Password must be at least 8 characters".to_string(),
        ));
    }

    // Rationale is fixed text — under no circumstances does the password
    // value (raw or otherwise) get committed to the chain.
    let subject = Subject::Repo {
        did: req.did.clone(),
    };
    let snapshot_id = audit_chain::capture_snapshot(&ctx.account_db, &subject)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // LB-1 Session 12 / chainlink #129: password UPDATE +
    // session/refresh_token DELETE + chain entry in one transaction.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::account::AccountManager::update_password_in_tx(&mut tx, &req.did, &req.password)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    format!("Account not found: {}", req.did),
                )
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "account.reset_password",
            subject: Some(&subject),
            rationale: "reset account password",
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
struct DeleteAccountRequest {
    /// Account DID
    did: String,
}

/// Delete account permanently (admin operation)
async fn admin_delete_account(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<DeleteAccountRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Validate DID format
    if !req.did.starts_with("did:") {
        return Err((StatusCode::BAD_REQUEST, "Invalid DID format".to_string()));
    }

    // Capture the snapshot BEFORE the delete so the chain row still has
    // a meaningful snapshot of the account row that's about to disappear.
    let subject = Subject::Repo {
        did: req.did.clone(),
    };
    let snapshot_id = audit_chain::capture_snapshot(&ctx.account_db, &subject)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let rationale = format!("permanently delete account {}", req.did);

    // LB-1 Session 12 / chainlink #129: cascade DELETE + chain entry
    // in one transaction so the audit row lands together with the
    // account vanishing.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::account::AccountManager::delete_account_permanent_in_tx(&mut tx, &req.did)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    format!("Account not found: {}", req.did),
                )
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "account.delete",
            subject: Some(&subject),
            rationale: &rationale,
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateAccountSigningKeyRequest {
    /// DID of the account whose signing key is being updated
    did: String,
    /// New signing key in did:key: format (per the lexicon)
    signing_key: String,
}

/// Update an account's signing key in the PLC directory
///
/// Implements `com.atproto.admin.updateAccountSigningKey`. Submits a PLC
/// operation rotating the `verificationMethods.atproto` entry to the supplied
/// did:key value, then advances the repository commit chain with an empty
/// commit and sequences an identity event so federation peers learn of the
/// change.
///
/// Aurora-Locus runs in a single-operator-key model: the operator's
/// `authentication.repo_signing_key` is the only private key the PDS can sign
/// commits with. Rotating to any other public key would leave the account
/// unable to produce new commits, so this handler enforces strict-mode
/// validation: the supplied `signingKey` must match the operator's configured
/// key. The lexicon contract permits arbitrary `signingKey` values; the
/// strict-mode check is an Aurora-architecture safety constraint.
async fn update_account_signing_key(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<UpdateAccountSigningKeyRequest>,
) -> Result<StatusCode, axum::response::Response> {
    use crate::actor_store::repository::RepositoryManager;
    use crate::crypto::{
        plc::PlcSigner,
        plc_client::{PlcClient, PlcClientConfig},
        proto_blue_signer::RepoSigner,
    };
    use crate::sequencer::events::IdentityEvent;
    use axum::response::IntoResponse;

    fn plain_err(status: StatusCode, msg: impl Into<String>) -> axum::response::Response {
        (status, msg.into()).into_response()
    }
    fn xrpc_err(
        status: StatusCode,
        error: &str,
        message: impl Into<String>,
    ) -> axum::response::Response {
        (
            status,
            Json(serde_json::json!({
                "error": error,
                "message": message.into(),
            })),
        )
            .into_response()
    }

    if !req.did.starts_with("did:plc:") {
        return Err(plain_err(
            StatusCode::BAD_REQUEST,
            "did must be a did:plc identifier",
        ));
    }
    if !req.signing_key.starts_with("did:key:") {
        return Err(plain_err(
            StatusCode::BAD_REQUEST,
            "signingKey must be in did:key: format",
        ));
    }

    // Strict-mode validation: the supplied signingKey must match the operator's
    // configured repo_signing_key. Aurora-Locus has a single operator-level
    // private key; any other rotation target would leave the account unable to
    // sign new commits.
    //
    // TODO: Relax this check when Aurora-Locus supports per-account signing
    // keys. The lexicon contract permits arbitrary signingKey values; this
    // strict-mode validation is a safety check appropriate to Aurora's
    // current single-key architecture.
    let repo_signer = PlcSigner::from_hex(&ctx.config.authentication.repo_signing_key)
        .map_err(|e| {
            plain_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Operator repo signing key not configured: {}", e),
            )
        })?;
    let operator_did_key = repo_signer.public_key_did_key();
    if req.signing_key != operator_did_key {
        return Err(xrpc_err(
            StatusCode::BAD_REQUEST,
            "InvalidRequest",
            "signingKey does not match operator's configured signing key. \
             Aurora-Locus uses a single operator-level signing key model; \
             the provided signingKey must match the operator's \
             repo_signing_key config.",
        ));
    }

    let plc_client = PlcClient::new(PlcClientConfig {
        plc_url: ctx.config.identity.did_plc_url.clone(),
        timeout_secs: 30,
    })
    .map_err(|e| {
        plain_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("PLC client init failed: {}", e),
        )
    })?;

    let rotation_signer = PlcSigner::from_hex(&ctx.config.authentication.plc_rotation_key)
        .map_err(|e| {
            plain_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("PLC rotation key not configured: {}", e),
            )
        })?;

    // Compare against the current PLC document. Aurora's PlcClient::get_signing_key
    // returns multibase form (the bare `z...` prefix); the request's signingKey is
    // in did:key form. Strip the prefix for comparison so we don't submit a no-op
    // PLC operation when the keys already match.
    let current_doc = plc_client.get_document(&req.did).await.map_err(|e| {
        if matches!(e, PdsError::IdentityResolution(_)) {
            plain_err(
                StatusCode::NOT_FOUND,
                format!("DID document not found: {}", e),
            )
        } else {
            plain_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    })?;
    let current_key_multibase = plc_client
        .get_signing_key(&current_doc)
        .map_err(|e| plain_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let new_key_multibase = req
        .signing_key
        .strip_prefix("did:key:")
        .unwrap_or(&req.signing_key);

    if plc_client.keys_match(&current_key_multibase, new_key_multibase) {
        tracing::debug!(did = %req.did, "Signing key already up to date; skipping PLC submission");
        return Ok(StatusCode::OK);
    }

    // Submit PLC update with the did:key form so the entry stores the canonical
    // verificationMethods.atproto value.
    plc_client
        .update_signing_key(&req.did, &req.signing_key, &rotation_signer)
        .await
        .map_err(|e| plain_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Advance the repository commit chain with an empty commit so the rotation
    // is reflected in repository state, not just the DID document. Mirrors the
    // CLI rotation flow in src/cli/rotate_keys.rs. Strict-mode validation
    // guarantees the operator's repo_signing_key matches the new PLC entry, so
    // the commit signature will verify against the newly-installed key.
    let repo_mgr = RepositoryManager::with_sequencer(
        req.did.clone(),
        (*ctx.actor_store).clone(),
        ctx.sequencer.clone(),
    )
    .with_blob_store(ctx.blob_store.clone());
    let repo_signer_pb: std::sync::Arc<dyn proto_blue::crypto::Signer> = {
        let key_bytes = hex::decode(&ctx.config.authentication.repo_signing_key).map_err(|e| {
            plain_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Invalid hex repo signing key: {}", e),
            )
        })?;
        let s = RepoSigner::from_bytes(&key_bytes).map_err(|e| {
            plain_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to build repo signer: {}", e),
            )
        })?;
        std::sync::Arc::new(s)
    };
    let (commit_cid, rev) = repo_mgr
        .apply_writes(
            vec![],
            repo_signer_pb,
            std::sync::Arc::new(crate::blob_store::StrictPromoter),
        )
        .await
        .map_err(|e| plain_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tracing::info!(
        did = %req.did,
        commit_cid = %commit_cid,
        rev = %rev,
        "Created empty commit for signing key rotation"
    );

    // Announce the change via an identity event.
    let account = ctx.account_manager.get_account(&req.did).await.map_err(|e| {
        if matches!(e, PdsError::NotFound(_)) {
            plain_err(
                StatusCode::NOT_FOUND,
                format!("Account not found: {}", req.did),
            )
        } else {
            plain_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    })?;
    let identity_evt = IdentityEvent::new(req.did.clone(), account.handle);
    ctx.sequencer
        .sequence_identity(identity_evt)
        .await
        .map_err(|e| plain_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let subject = Subject::Repo {
        did: req.did.clone(),
    };
    let snapshot_id = audit_chain::capture_snapshot(&ctx.account_db, &subject)
        .await
        .map_err(|e| plain_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let rationale = format!("rotate signing key to {}", req.signing_key);

    // LB-1 Session 12 / chainlink #129: signing-key rotation's actor
    // mutations live in actor_store (separate DB) and the PLC
    // directory (HTTP), with the operator's repo_signing_key as the
    // only signing material in account_db (read-only here). The chain
    // entry is the only account_db write and the tx wrapper is
    // structurally consistent with the rest of Session 12 (no other
    // writes share the tx).
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| plain_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "account.update_signing_key",
            subject: Some(&subject),
            rationale: &rationale,
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| plain_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| plain_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tracing::info!(
        admin = %auth.did,
        did = %req.did,
        "Updated account signing key via XRPC"
    );

    Ok(StatusCode::OK)
}

// ============================================================================
// Account Moderation Endpoints
// ============================================================================

#[derive(Deserialize)]
struct TakedownAccountRequest {
    did: String,
    reason: String,
    #[serde(default)]
    notes: Option<String>,
}

/// Takedown an account (remove from public view)
async fn takedown_account(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<TakedownAccountRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::admin::moderation::{ApplyActionParams, ModerationAction, ModerationManager};

    let subject = Subject::Repo {
        did: req.did.clone(),
    };
    let snapshot_id = audit_chain::capture_snapshot(&ctx.account_db, &subject)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // LB-1 Session 12 / chainlink #129: moderation row + actor
    // takedown UPDATE + chain entry all in one transaction.
    // ModerationManager::apply_action_in_tx threads the tx through
    // to AccountManager::takedown_account_in_tx for the actor mutation.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let record = ModerationManager::apply_action_in_tx(
        &mut tx,
        ApplyActionParams {
            did: &req.did,
            action: ModerationAction::Takedown,
            reason: &req.reason,
            moderated_by: &auth.did,
            expires_in: None,
            report_id: None,
            notes: req.notes.clone(),
        },
    )
    .await
    .map_err(|e| {
        if matches!(e, PdsError::NotFound(_)) {
            (StatusCode::NOT_FOUND, format!("Account not found: {}", req.did))
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    })?;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "account.takedown",
            subject: Some(&subject),
            rationale: &req.reason,
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Arc 15 §8.3.6: emit Takendown #account event (Pattern B).
    // Reverse-takedown deferred to a future cycle per §8.1.2.
    let acc_post = ctx
        .account_manager
        .get_account(&req.did)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let (active, status) = crate::api::sync_helpers::get_account_status(&acc_post);
    debug_assert_eq!(
        status,
        Some(crate::sequencer::events::AccountStatus::Takendown)
    );
    ctx.sequencer
        .sequence_account(crate::sequencer::events::AccountEvent {
            did: req.did.clone(),
            active,
            status,
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "moderation_id": record.id,
        "did": req.did,
        "action": "takedown",
    })))
}

#[derive(Deserialize)]
struct SuspendAccountRequest {
    did: String,
    reason: String,
    #[serde(default)]
    duration_days: Option<i64>,
    #[serde(default)]
    notes: Option<String>,
}

/// Suspend an account temporarily
async fn suspend_account(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<SuspendAccountRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::admin::moderation::{ApplyActionParams, ModerationAction, ModerationManager};

    let expires_in = req.duration_days.map(Duration::days);
    let subject = Subject::Repo {
        did: req.did.clone(),
    };
    let snapshot_id = audit_chain::capture_snapshot(&ctx.account_db, &subject)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // LB-1 Session 12 / chainlink #129: suspend has no per-DID actor
    // mutation today (the moderation_event row IS the suspension
    // record), but the chain entry + moderation_event INSERT still
    // benefit from being atomic.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let record = ModerationManager::apply_action_in_tx(
        &mut tx,
        ApplyActionParams {
            did: &req.did,
            action: ModerationAction::Suspend,
            reason: &req.reason,
            moderated_by: &auth.did,
            expires_in,
            report_id: None,
            notes: req.notes.clone(),
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "account.suspend",
            subject: Some(&subject),
            rationale: &req.reason,
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "moderation_id": record.id,
        "did": req.did,
        "action": "suspend",
        "expires_at": record.expires_at,
    })))
}

#[derive(Deserialize)]
struct RestoreAccountRequest {
    did: String,
    moderation_id: i64,
    reason: String,
}

/// Restore an account after takedown/suspension
async fn restore_account(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RestoreAccountRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::admin::moderation::ModerationManager;

    let subject = Subject::Repo {
        did: req.did.clone(),
    };
    let snapshot_id = audit_chain::capture_snapshot(&ctx.account_db, &subject)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // LB-1 Session 12 / chainlink #129: moderation reversal +
    // actor activate (clears takedown_ref) + chain entry in one
    // transaction. ModerationManager::reverse_action_in_tx threads
    // the tx through to AccountManager::activate_account_in_tx.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    ModerationManager::reverse_action_in_tx(&mut tx, req.moderation_id, &auth.did, &req.reason)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (StatusCode::NOT_FOUND, e.to_string())
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "account.restore",
            subject: Some(&subject),
            rationale: &req.reason,
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // chainlink #179: emit #account (active=true) after restore, symmetrizing
    // with the takedown direction's emit. Runs post-commit on the durable
    // post-restore row state (Pattern B). restore_account only succeeds on a
    // real moderation reversal (reverse_action_in_tx errors NotFound otherwise),
    // so there is no spurious emit on a no-op restore.
    let acc_post = ctx
        .account_manager
        .get_account(&req.did)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let (active, status) = crate::api::sync_helpers::get_account_status(&acc_post);
    ctx.sequencer
        .sequence_account(crate::sequencer::events::AccountEvent {
            did: req.did.clone(),
            active,
            status,
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "did": req.did,
    })))
}

#[derive(Deserialize)]
struct GetModerationHistoryQuery {
    did: String,
}

/// Get moderation history for an account
async fn get_moderation_history(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<GetModerationHistoryQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let history = ctx
        .moderation_manager
        .get_history(&query.did)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "did": query.did,
        "history": history,
    })))
}

// ============================================================================
// Label Management Endpoints
// ============================================================================

#[derive(Deserialize)]
struct ApplyLabelRequest {
    uri: String,
    #[serde(default)]
    cid: Option<String>,
    val: String,
    #[serde(default)]
    expires_days: Option<i64>,
}

/// Apply a label to content
async fn apply_label(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<ApplyLabelRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let expires_in = req.expires_days.map(Duration::days);

    // Empty-cid case = label applies to the URI version-agnostically;
    // the chain row keeps subject_uri populated and subject_cid as the
    // empty string so the Record variant is well-formed.
    let subject = Subject::Record {
        uri: req.uri.clone(),
        cid: req.cid.clone().unwrap_or_default(),
    };
    let snapshot_id = audit_chain::capture_snapshot(&ctx.account_db, &subject)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let rationale = format!("apply label '{}' to {}", req.val, req.uri);
    let server_did = format!("did:web:{}", ctx.config.service.hostname);

    // LB-1 Session 12 / chainlink #129: label INSERT + chain entry
    // in one transaction.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let label = crate::admin::LabelManager::apply_label_in_tx(
        &mut tx,
        &server_did,
        &req.uri,
        req.cid.as_deref(),
        &req.val,
        &auth.did,
        expires_in,
        "manual",
        None,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .label;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "label.apply",
            subject: Some(&subject),
            rationale: &rationale,
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "label": label,
    })))
}

#[derive(Deserialize)]
struct RemoveLabelRequest {
    uri: String,
    #[serde(default)]
    cid: Option<String>,
    val: String,
}

/// Remove a label from content
async fn remove_label(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RemoveLabelRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let subject = Subject::Record {
        uri: req.uri.clone(),
        cid: req.cid.clone().unwrap_or_default(),
    };
    let snapshot_id = audit_chain::capture_snapshot(&ctx.account_db, &subject)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let rationale = format!("remove label '{}' from {}", req.val, req.uri);
    let server_did = format!("did:web:{}", ctx.config.service.hostname);

    // LB-1 Session 12 / chainlink #129: negative-label INSERT +
    // chain entry in one transaction.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let label = crate::admin::LabelManager::remove_label_in_tx(
        &mut tx,
        &server_did,
        &req.uri,
        req.cid.as_deref(),
        &req.val,
        &auth.did,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "label.remove",
            subject: Some(&subject),
            rationale: &rationale,
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "label": label,
    })))
}

// ============================================================================
// Report Management Endpoints
// ============================================================================

#[derive(Deserialize)]
struct SubmitReportRequest {
    #[serde(default)]
    subject_did: Option<String>,
    #[serde(default)]
    subject_uri: Option<String>,
    #[serde(default)]
    subject_cid: Option<String>,
    reason_type: String,
    #[serde(default)]
    reason: Option<String>,
}

/// Submit a report
async fn submit_report(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<SubmitReportRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::admin::reports::ReportReason;

    // Parse reason type
    let reason_type: ReportReason = req
        .reason_type
        .parse()
        .map_err(|e: PdsError| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Submit report
    let report = ctx
        .report_manager
        .submit_report(
            req.subject_did.as_deref(),
            req.subject_uri.as_deref(),
            req.subject_cid.as_deref(),
            reason_type,
            req.reason.as_deref(),
            &auth.did,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // §5.5.4 Phase A: apply the configured default action (full tier
    // only). Best-effort — the report is already persisted.
    if let Err(e) =
        crate::api::moderation_defaults::apply_report_default(&ctx, &report).await
    {
        tracing::warn!(
            error = %e,
            report_id = report.id,
            "moderation default-action consumer failed on submitReport intake"
        );
    }
    // §5.5.4 Phase B: reviewer routing (Pipeline A §4), best-effort.
    if let Err(e) =
        crate::api::reviewer_assignment::assign_reviewer_on_intake(&ctx, &report).await
    {
        tracing::warn!(
            error = %e,
            report_id = report.id,
            "reviewer-assignment consumer failed on submitReport intake"
        );
    }
    // §5.5.4 Phase C: Pipeline A report-count auto-label rules. Best-effort.
    if let Err(e) = crate::api::auto_label_rules::evaluate_pipeline_a(&ctx, &report).await {
        tracing::warn!(
            error = %e,
            report_id = report.id,
            "auto-label Pipeline A failed on submitReport intake"
        );
    }
    // §5.5.4 Phase D: Pipeline A escalation rules. Best-effort.
    if let Err(e) = crate::api::escalation_rules::evaluate_pipeline_a(&ctx, &report).await {
        tracing::warn!(
            error = %e,
            report_id = report.id,
            "escalation Pipeline A failed on submitReport intake"
        );
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "report": report,
    })))
}

#[derive(Deserialize)]
struct UpdateReportStatusRequest {
    report_id: i64,
    status: String,
    #[serde(default)]
    resolution: Option<String>,
}

/// Update report status
async fn update_report_status(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<UpdateReportStatusRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::admin::reports::ReportStatus;

    // Parse status
    let status: ReportStatus = req
        .status
        .parse()
        .map_err(|e: PdsError| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Look up the report so the chain entry's subject reflects the
    // thing the report points at (account/record/blob), not the report
    // row itself. Reports without subject info fall through to a
    // None-subject chain entry — the action is still audited but the
    // chain row is server-level.
    let report = ctx
        .report_manager
        .get_report(req.report_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let subject = report.as_ref().and_then(|r| {
        Subject::from_columns(
            r.subject_did.as_deref(),
            r.subject_uri.as_deref(),
            r.subject_cid.as_deref(),
        )
    });
    let snapshot_id = if let Some(s) = &subject {
        audit_chain::capture_snapshot(&ctx.account_db, s)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        None
    };
    let rationale = req.resolution.clone().unwrap_or_default();

    // LB-1 Session 12 / chainlink #129: report UPDATE + chain entry
    // in one transaction.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::admin::ReportManager::update_status_in_tx(
        &mut tx,
        req.report_id,
        status,
        &auth.did,
        req.resolution.as_deref(),
    )
    .await
    .map_err(|e| {
        if matches!(e, PdsError::NotFound(_)) {
            (StatusCode::NOT_FOUND, e.to_string())
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    })?;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "report.update",
            subject: subject.as_ref(),
            rationale: &rationale,
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "report_id": req.report_id,
        "status": req.status,
    })))
}

#[derive(Deserialize)]
struct ListReportsQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

/// List reports
async fn list_reports(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Query(query): Query<ListReportsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::admin::reports::{AssignmentScope, ReportStatus};
    use crate::admin::roles::Role;

    // Parse status filter if provided
    let status_filter = if let Some(status_str) = query.status {
        Some(
            status_str
                .parse::<ReportStatus>()
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?,
        )
    } else {
        None
    };

    // §5.5.4 §4.5 queue scope (same as getModerationQueue).
    let scope = if auth.role.can_act_as(Role::SuperAdmin) {
        AssignmentScope::All
    } else {
        AssignmentScope::AssignedTo(&auth.did)
    };

    // List reports
    let reports = ctx
        .report_manager
        .list_reports_scoped(status_filter, query.limit, scope)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "reports": reports,
    })))
}

// ============================================================================
// Email Endpoints
// ============================================================================

/// Per spec: `subject` is optional, `senderDid` is required. Aurora retains
/// a permissive extension allowing `senderDid` to be omitted (defaults to
/// the authenticated admin's DID). Spec-compliant callers passing both
/// fields work unchanged.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendEmailRequest {
    /// DID of the recipient (spec-required).
    recipient_did: String,
    /// Email body content (spec-required).
    content: String,
    /// Optional email subject. Phase 1.8 (#63) flipped this from required
    /// to optional to match the lexicon. When omitted, a placeholder
    /// subject is used at the SMTP layer.
    #[serde(default)]
    subject: Option<String>,
    /// Aurora-permissive extension: spec marks `senderDid` as required, but
    /// Aurora defaults to the authenticated admin's DID when omitted.
    /// Spec-compliant callers pass an explicit value.
    #[serde(default)]
    sender_did: Option<String>,
    /// Optional sender comment used for audit context (spec-optional).
    #[serde(default)]
    comment: Option<String>,
}

/// Send email response per ATProto spec
#[derive(Debug, serde::Serialize)]
struct SendEmailResponse {
    sent: bool,
}

/// Default subject line used when the spec-optional `subject` field is omitted.
/// `send_admin_email` needs a non-empty string for the SMTP `Subject:` header;
/// "(no subject)" matches the conventional MUA fallback.
const DEFAULT_EMPTY_SUBJECT: &str = "(no subject)";

/// Send an email to a user
///
/// Allows admins to send emails to users for moderation notices,
/// warnings, or other administrative purposes.
async fn send_email(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<SendEmailRequest>,
) -> Result<Json<SendEmailResponse>, (StatusCode, String)> {
    // Get the recipient account to find their email
    let account = ctx
        .account_manager
        .get_account(&req.recipient_did)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, format!("Account not found: {}", e)))?;

    // Check if account has email
    let to_email = account.email.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "Account has no email address".to_string(),
        )
    })?;

    let effective_subject = req.subject.as_deref().unwrap_or(DEFAULT_EMPTY_SUBJECT);

    // Aurora's permissive extension: when senderDid is omitted, attribute
    // the action to the authenticated admin.
    let sender = req.sender_did.as_deref().unwrap_or(&auth.did);
    let subject_ref = Subject::Repo {
        did: req.recipient_did.clone(),
    };
    let snapshot_id = audit_chain::capture_snapshot(&ctx.account_db, &subject_ref)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let rationale = match (req.subject.as_deref(), req.comment.as_deref()) {
        (Some(s), Some(c)) => format!("{}: {}", s, c),
        (Some(s), None) => s.to_string(),
        (None, Some(c)) => c.to_string(),
        (None, None) => effective_subject.to_string(),
    };

    // LB-1 Session 12 / chainlink #129: chain-first ordering. Pre-fix
    // the mailer dispatched first and the chain entry wrote after; if
    // the chain append failed, the email was sent without an audit
    // trail (a §3.4 violation: operator's intent un-recorded). Now
    // the chain entry records intent first inside its own transaction;
    // the mailer side effect runs post-commit best-effort. If the
    // mailer fails the audit entry remains as evidence of the
    // operator's request, and the operator can retry via the chain id.
    //
    // Behavior change: mailer failure no longer aborts the handler
    // with 500; instead the response signals the failure via
    // `sent: false`. Spec-compliant clients reading `sent` see the
    // outcome unchanged on success and a clear failure signal on
    // mailer error.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: sender,
            action: "email.send",
            subject: Some(&subject_ref),
            rationale: &rationale,
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Mailer dispatch (post-commit best-effort). On failure the chain
    // entry remains as evidence of the operator's request and the
    // response signals the failure via `sent: false`.
    let sent = match ctx
        .mailer
        .send_admin_email(&to_email, effective_subject, &req.content)
        .await
    {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(
                "send_email: chain entry recorded but mailer failed for {}: {}",
                req.recipient_did,
                e
            );
            false
        }
    };

    tracing::info!(
        "Admin {} sent email to {} ({}): {} (sent={})",
        auth.did,
        req.recipient_did,
        to_email,
        effective_subject,
        sent
    );

    Ok(Json(SendEmailResponse { sent }))
}

// ============================================================================
// Audit Log Endpoints
// ============================================================================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetAuditLogQuery {
    /// Filter by admin DID
    #[serde(default)]
    admin_did: Option<String>,
    /// Filter by action type (e.g., "account.takedown", "label.apply")
    #[serde(default)]
    action: Option<String>,
    /// Filter by subject DID
    #[serde(default)]
    subject_did: Option<String>,
    /// Maximum number of entries to return (default 50, max 100)
    #[serde(default)]
    limit: Option<i64>,
    /// Cursor for pagination (ID of last entry from previous page)
    #[serde(default)]
    cursor: Option<i64>,
}

/// Audit log entry response
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditLogEntryResponse {
    id: i64,
    admin_did: String,
    action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    subject_did: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
    timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ip_address: Option<String>,
}

/// Audit log response
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GetAuditLogResponse {
    entries: Vec<AuditLogEntryResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<i64>,
    total_count: i64,
}

/// Get audit log entries
///
/// Returns a paginated list of admin action audit log entries. Now reads
/// from the hash-chained `audit_chain_entry` table (the legacy
/// `admin_audit_log` table is gone). Response shape is preserved for
/// back-compat: `details` maps to chain `rationale`, `admin_did` maps to
/// chain `actor_did`, and `ip_address` is always omitted (chain rows
/// don't carry that field; `getAuditTrail` is the richer surface for
/// chain-aware consumers).
///
/// Can be filtered by admin DID, action type, or subject DID. Pre-Phase-3.8
/// `pre-chain` sentinel rows are surfaced like any other entry — clients
/// that care about hash verification should use `getAuditTrail` instead.
async fn get_audit_log(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<GetAuditLogQuery>,
) -> Result<Json<GetAuditLogResponse>, (StatusCode, String)> {
    use sqlx::Row;

    // Clamp limit to reasonable range
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let fetch_limit = limit + 1; // Fetch one extra to check if there are more

    // Build the WHERE clause incrementally so each filter contributes a
    // bound parameter. Cursor is a chain-row `id` (descending pagination
    // matches the legacy admin_audit_log behavior).
    let mut sql = String::from(
        "SELECT id, actor_did, action, subject_did, rationale, created_at \
         FROM audit_chain_entry WHERE 1=1",
    );
    let mut admin_filter: Option<String> = None;
    let mut action_filter: Option<String> = None;
    let mut subject_filter: Option<String> = None;
    let mut cursor_filter: Option<i64> = None;

    if let Some(did) = &query.admin_did {
        sql.push_str(" AND actor_did = ?");
        admin_filter = Some(did.clone());
    }
    if let Some(act) = &query.action {
        sql.push_str(" AND action = ?");
        action_filter = Some(act.clone());
    }
    if let Some(did) = &query.subject_did {
        sql.push_str(" AND subject_did = ?");
        subject_filter = Some(did.clone());
    }
    if let Some(cur) = query.cursor {
        sql.push_str(" AND id < ?");
        cursor_filter = Some(cur);
    }
    sql.push_str(" ORDER BY id DESC LIMIT ?");

    let mut q = sqlx::query(&sql);
    if let Some(v) = &admin_filter {
        q = q.bind(v);
    }
    if let Some(v) = &action_filter {
        q = q.bind(v);
    }
    if let Some(v) = &subject_filter {
        q = q.bind(v);
    }
    if let Some(v) = cursor_filter {
        q = q.bind(v);
    }
    q = q.bind(fetch_limit);

    let rows = q
        .fetch_all(&ctx.account_db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Total count (unfiltered) — matches the legacy reader's contract.
    let total_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_chain_entry")
        .fetch_one(&ctx.account_db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let has_more = rows.len() as i64 > limit;
    let trimmed: Vec<_> = rows.into_iter().take(limit as usize).collect();

    let response_entries: Vec<AuditLogEntryResponse> = trimmed
        .iter()
        .map(|row| AuditLogEntryResponse {
            id: row.get::<i64, _>("id"),
            admin_did: row.get::<String, _>("actor_did"),
            action: row.get::<String, _>("action"),
            subject_did: row.try_get::<Option<String>, _>("subject_did").ok().flatten(),
            details: Some(row.get::<String, _>("rationale")),
            timestamp: row.get::<String, _>("created_at"),
            ip_address: None,
        })
        .collect();

    let next_cursor = if has_more {
        response_entries.last().map(|e| e.id)
    } else {
        None
    };

    Ok(Json(GetAuditLogResponse {
        entries: response_entries,
        cursor: next_cursor,
        total_count,
    }))
}

// ============================================================================
// Additional Endpoints for Admin Panel Compatibility
// ============================================================================

#[derive(Deserialize)]
struct GetAccountQuery {
    did: String,
}

/// Get single account details
async fn get_account(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<GetAccountQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let account = ctx
        .account_manager
        .get_account(&query.did)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, format!("Account not found: {}", e)))?;

    Ok(Json(serde_json::json!({
        "did": account.did,
        "handle": account.handle,
        "email": account.email,
        "created_at": account.created_at,
        "email_confirmed": account.email_confirmed_at.is_some(),
        "takedown": account.takedown_ref.is_some(),
    })))
}

#[derive(Deserialize)]
struct GetAccountInfosQuery {
    /// DIDs to look up. Decoded from repeated `?dids=...&dids=...` query
    /// parameters via `axum_extra::extract::Query`. Phase 1.9 (#64) replaced
    /// the legacy comma-separated single-string encoding with the
    /// lexicon-conformant repeated-param form; behavior change is documented
    /// in the commit that introduced this struct.
    dids: Vec<String>,
}

/// Account info for batch responses (lexicon `com.atproto.admin.defs#accountView`).
///
/// `handle` is required per the lexicon. Phase 1.9 (#64) flipped it from
/// `Option<String>` to `String`; the underlying `actor.handle` column is
/// `NOT NULL` in the schema, so the backing data is always present.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountInfo {
    did: String,
    handle: String,
    email: Option<String>,
    indexed_at: String,
    email_confirmed_at: Option<String>,
    invited_by: Option<InviteCodeInfo>,
    invites: Vec<InviteCodeInfo>,
    invites_disabled: bool,
    invite_note: Option<String>,
    deactivated_at: Option<String>,
    threat_signatures: Vec<ThreatSignature>,
}

/// Invite code info embedded in account info
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InviteCodeInfo {
    code: String,
    available: i32,
    disabled: bool,
    for_account: String,
    created_by: String,
    created_at: String,
    uses: Vec<InviteCodeUse>,
}

/// Record of invite code usage
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InviteCodeUse {
    used_by: String,
    used_at: String,
}

/// Threat signature (for future anti-spam/abuse detection)
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreatSignature {
    property: String,
    value: String,
}

/// Response for getAccountInfos
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GetAccountInfosResponse {
    infos: Vec<AccountInfo>,
}

/// Build an `AccountInfo` (lexicon `accountView`) for a single DID.
///
/// Shared helper for `get_account_info` (singular) and `get_account_infos`
/// (plural). Returns `PdsError::NotFound` when the account does not exist;
/// callers map that to 404 / `RepoNotFound` for the singular endpoint or to
/// silent skip for the plural endpoint.
///
/// Future shape fixes tracked in chainlink #64 (Phase 1.9 — getAccountInfos
/// param encoding + handle field) will land in this single helper and
/// propagate to both endpoints simultaneously.
async fn build_account_info(ctx: &AppContext, did: &str) -> PdsResult<AccountInfo> {
    let account = ctx.account_manager.get_account(did).await?;

    let invited_by = ctx
        .invite_manager
        .get_invite_for_account(did)
        .await
        .ok()
        .flatten()
        .map(|inv| InviteCodeInfo {
            code: inv.code.clone(),
            available: inv.available,
            disabled: inv.disabled,
            for_account: inv.for_account.clone().unwrap_or_default(),
            created_by: inv.created_by.clone(),
            created_at: inv.created_at.to_rfc3339(),
            uses: vec![],
        });

    let account_invites = ctx
        .invite_manager
        .get_codes_created_by(did)
        .await
        .unwrap_or_default();

    let invites: Vec<InviteCodeInfo> = account_invites
        .into_iter()
        .map(|inv| InviteCodeInfo {
            code: inv.code.clone(),
            available: inv.available,
            disabled: inv.disabled,
            for_account: inv.for_account.clone().unwrap_or_default(),
            created_by: inv.created_by.clone(),
            created_at: inv.created_at.to_rfc3339(),
            uses: vec![],
        })
        .collect();

    Ok(AccountInfo {
        did: account.did.clone(),
        // `actor.handle` is NOT NULL in schema; the Option on ActorAccount is
        // Rust-side defensiveness. Default to empty string only as a
        // belt-and-suspenders fallback for a row that violates the invariant.
        handle: account.handle.clone().unwrap_or_default(),
        email: account.email.clone(),
        indexed_at: account.created_at.to_rfc3339(),
        email_confirmed_at: account.email_confirmed_at.map(|dt| dt.to_rfc3339()),
        invited_by,
        invites,
        invites_disabled: account.invites_disabled.unwrap_or(false),
        invite_note: None,
        deactivated_at: account.deactivated_at.map(|dt| dt.to_rfc3339()),
        threat_signatures: vec![],
    })
}

/// Get multiple account details in batch
///
/// Batch lookup of multiple account details by DIDs. Accepts repeated
/// `?dids=...&dids=...` query parameters per the lexicon. Returns information
/// for all found accounts (missing DIDs are silently skipped). Uses
/// `axum_extra::extract::Query` rather than the default `axum::extract::Query`
/// because the latter's `serde_urlencoded` backend collapses repeated keys
/// to the last value.
async fn get_account_infos(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    axum_extra::extract::Query(query): axum_extra::extract::Query<GetAccountInfosQuery>,
) -> Result<Json<GetAccountInfosResponse>, (StatusCode, String)> {
    if query.dids.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "No DIDs provided".to_string()));
    }

    // Limit batch size to prevent abuse
    const MAX_BATCH_SIZE: usize = 100;
    if query.dids.len() > MAX_BATCH_SIZE {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Too many DIDs (max {})", MAX_BATCH_SIZE),
        ));
    }

    let mut infos = Vec::new();
    for did in &query.dids {
        if !did.starts_with("did:") {
            continue;
        }
        if let Ok(info) = build_account_info(&ctx, did).await {
            infos.push(info);
        }
    }

    Ok(Json(GetAccountInfosResponse { infos }))
}

#[derive(Deserialize)]
struct GetAccountInfoQuery {
    /// DID of the account to look up
    did: String,
}

#[derive(Deserialize)]
struct SearchAccountsQuery {
    /// Free-text search term (#315) — case-insensitive substring over handle,
    /// DID, and email. This is what the admin UI's Accounts search box sends;
    /// it was previously absent from this struct, so `Query<>` silently dropped
    /// it and every account came back regardless of the term.
    #[serde(default)]
    q: Option<String>,
    /// Optional email to filter by (exact, case-insensitive)
    #[serde(default)]
    email: Option<String>,
    /// Pagination cursor (opaque to clients; server treats it as the
    /// last DID returned by the previous page)
    #[serde(default)]
    cursor: Option<String>,
    /// Page size, 1-100, default 50 per lexicon
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(serde::Serialize)]
struct SearchAccountsResponse {
    /// Required per lexicon — always present, possibly empty
    accounts: Vec<AccountInfo>,
    /// Present only when more pages remain
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
}

/// Search accounts by email with cursor pagination
/// (lexicon `com.atproto.admin.searchAccounts`).
///
/// Reuses the `build_account_info` helper with `get_account_info` and
/// `get_account_infos` so the `accountView` shape stays consistent across
/// all three endpoints. Cursor pagination uses the trailing DID as an
/// opaque cursor; the same scheme will be reused by Phase 1.10 (#65) when
/// it backfills pagination on `listAccounts` and `getInviteCodes`.
async fn search_accounts(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<SearchAccountsQuery>,
) -> Result<Json<SearchAccountsResponse>, (StatusCode, String)> {
    // Lexicon: limit is integer 1-100, default 50.
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err((
            StatusCode::BAD_REQUEST,
            "limit must be between 1 and 100".to_string(),
        ));
    }

    // Fetch limit+1 to detect whether more pages remain.
    let rows = ctx
        .account_manager
        .search_accounts(
            query.email.as_deref(),
            query.q.as_deref(),
            query.cursor.as_deref(),
            limit + 1,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let has_more = rows.len() as i64 > limit;
    let page: Vec<_> = rows.into_iter().take(limit as usize).collect();
    let next_cursor = if has_more {
        page.last().map(|a| a.did.clone())
    } else {
        None
    };

    let mut accounts = Vec::with_capacity(page.len());
    for actor in &page {
        // Reuse the shared accountView builder. Errors here mean the
        // account row was deleted between the search and the per-DID
        // lookup — extremely rare, but skip rather than fail the page.
        if let Ok(info) = build_account_info(&ctx, &actor.did).await {
            accounts.push(info);
        }
    }

    Ok(Json(SearchAccountsResponse {
        accounts,
        cursor: next_cursor,
    }))
}

// ---- tools.aurora.describeCapabilities (chainlink #99 / Phase 3.2) ----
//
// Top-level capability probe. Static at compile time (open question
// §9.4 resolved as Option A): the response reflects what's
// structurally present in this build, not what's wired-and-ready at
// runtime. Future sub-phases (3.5 event-variants, 3.8 hash-chained-
// audit, 3.9 realtime-events) extend the static lists below as they
// land.
//
// Auth: AdminAuthContext (Moderator+) — matches Phase 2.3 ops
// convention. Capability advertisement is a privileged operation
// because it surfaces operational structure that could inform
// targeted attacks; we don't gate the wire format on an unauth
// probe.

/// Response shape for `tools.aurora.admin.describeCapabilities`.
///
/// Per `docs/V03_DESIGN.md` §6.3.1: field stability is committed.
/// New fields may be added; existing field names and shapes do not
/// change across releases. Capability strings within `extensions`
/// follow the `<kebab-family>-v<integer>` versioning convention
/// (committed on `crate::api::registry::WIRE_EXTENSION_ORDER` per
/// Step 4).
///
/// Snapshot test: `describe_capabilities_snapshot` in this file's
/// test module pins the wire format and catches drift loudly.
/// Both top-level fields (alphabetical via canonical-JSON) and
/// inner namespace keys (alphabetical via `serde_json::Map`'s
/// default `BTreeMap` backing) sort deterministically; endpoint
/// arrays preserve the per-family `registration_order` of the
/// `RouteRegistry` populated by `aurora_route_builder()`.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DescribeCapabilitiesResponse {
    families: serde_json::Value,
    extensions: Vec<CapabilityExtension>,
    implementation: &'static str,
    version: &'static str,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CapabilityExtension {
    name: String,
    /// Optional structured value (e.g. `event-variants` carries the list of
    /// supported ModEvent variant names). Omitted when not applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<serde_json::Value>,
}

/// Build the `families` `serde_json::Value` by walking
/// `RouteRegistry::advertised_by_family`.
///
/// Family iteration order: `advertised_by_family()` returns a
/// `BTreeMap<Family, _>` which iterates in `Family::Ord` order —
/// alphabetical per the enum declaration. Inserting into a
/// `serde_json::Map` (BTreeMap-backed) is also alphabetical by
/// the namespace string. Both orderings coincide because the
/// `Family` enum's variant order and its `Display` strings are
/// alphabetical (`admin` < `moderator` < `ops` < `superadmin`).
///
/// Endpoint iteration order: each family's `Vec<&RouteEntry>` is
/// pre-sorted by `registration_order` (per Step 0 Q5 disposition
/// (a): freeze accidental orderings to the source declaration
/// order in `admin::routes()`).
///
/// Method-name extraction: every admin-tier route is an XRPC
/// path shaped `/xrpc/<namespace>.<method>`. `rsplit('.').next()`
/// returns the trailing segment after the last dot. The `.path`
/// fallback only triggers if a future route deviates from this
/// shape — in which case the snapshot test will fail loudly
/// rather than silently shipping a malformed wire entry.
fn build_families_value(
    registry: &crate::api::registry::RouteRegistry,
) -> serde_json::Value {
    let by_family = registry.advertised_by_family();
    let mut map = serde_json::Map::new();
    for (family, entries) in by_family.iter() {
        let endpoints: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                let method = e
                    .path
                    .strip_prefix("/xrpc/")
                    .and_then(|p| p.rsplit('.').next())
                    .unwrap_or(&e.path);
                serde_json::Value::String(method.to_string())
            })
            .collect();
        map.insert(family.to_string(), serde_json::Value::Array(endpoints));
    }
    serde_json::Value::Object(map)
}

/// `tools.aurora.describeCapabilities` — top-level probe.
///
/// Reads from `ctx.route_registry` populated by
/// `aurora_route_builder()` in `admin::routes()`. Wire output is
/// byte-identical to the prior hand-curated implementation — the
/// snapshot test `describe_capabilities_snapshot` pins this.
async fn describe_capabilities(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<DescribeCapabilitiesResponse>, (StatusCode, String)> {
    let registry = ctx.route_registry.as_ref();
    let families = build_families_value(registry);
    let extensions = registry
        .advertised_extensions()
        .into_iter()
        .map(|name| CapabilityExtension { name, value: None })
        .collect();
    Ok(Json(DescribeCapabilitiesResponse {
        families,
        extensions,
        implementation: "aurora-locus",
        // Cargo.toml's package version. Bumped as part of release work.
        version: env!("CARGO_PKG_VERSION"),
    }))
}

// ---- tools.aurora.ops.listAccounts (chainlink #84 / Phase 2.3.7) ----

/// Query parameters for tools.aurora.ops.listAccounts.
///
/// Operator-facing account listing with broader filters than
/// com.atproto.admin.searchAccounts. See AccountManager::ops_list_accounts
/// for the full filter semantics.
#[derive(Deserialize)]
struct OpsListAccountsQuery {
    /// Lower bound for `actor.created_at` (inclusive), RFC3339.
    #[serde(rename = "signupDateFrom", default)]
    signup_date_from: Option<String>,
    /// Upper bound for `actor.created_at` (inclusive), RFC3339.
    #[serde(rename = "signupDateTo", default)]
    signup_date_to: Option<String>,
    /// Filter to accounts onboarded via an invite code created by this DID.
    #[serde(rename = "inviteSource", default)]
    invite_source: Option<String>,
    /// Status filter: `active` | `deactivated` | `takedown` | `suspended`.
    #[serde(default)]
    status: Option<String>,
    /// Pagination cursor: trailing DID from previous page (opaque to clients).
    #[serde(default)]
    cursor: Option<String>,
    /// Page size, 1-100, default 50.
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, serde::Serialize)]
struct OpsListAccountsResponse {
    /// Required, possibly empty.
    accounts: Vec<AccountInfo>,
    /// Present only when more pages remain.
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
}

/// Operator-facing account listing.
///
/// Preserves Aurora-Locus's broader filtering capability beyond bsky-PDS's
/// `searchAccounts`. Filters on signup date range, invite source DID, and
/// status; cursor + limit pagination. Returns paginated `accountView[]`
/// using the same `build_account_info` helper as the other admin
/// account endpoints so the wire shape stays consistent.
async fn ops_list_accounts(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<OpsListAccountsQuery>,
) -> Result<Json<OpsListAccountsResponse>, (StatusCode, String)> {
    // Validate limit.
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err((
            StatusCode::BAD_REQUEST,
            "limit must be between 1 and 100".to_string(),
        ));
    }

    // Validate status enum if provided. Anything else is a client bug; reject
    // explicitly so callers don't quietly get unfiltered results.
    if let Some(s) = query.status.as_deref() {
        if !matches!(s, "active" | "deactivated" | "takedown" | "suspended") {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "status must be one of: active, deactivated, takedown, suspended (got {})",
                    s
                ),
            ));
        }
    }

    // Validate dates as RFC3339 (failure here means client typo, not server
    // problem; reject upfront rather than letting the SQL string-compare
    // through).
    for (label, val) in [
        ("signupDateFrom", &query.signup_date_from),
        ("signupDateTo", &query.signup_date_to),
    ] {
        if let Some(s) = val {
            if chrono::DateTime::parse_from_rfc3339(s).is_err() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("{} must be RFC3339 datetime", label),
                ));
            }
        }
    }

    // Validate inviteSource is a DID-looking string.
    if let Some(d) = query.invite_source.as_deref() {
        if !d.starts_with("did:") {
            return Err((
                StatusCode::BAD_REQUEST,
                "inviteSource must be a DID identifier".to_string(),
            ));
        }
    }

    // Fetch limit+1 to detect more pages.
    let rows = ctx
        .account_manager
        .ops_list_accounts(
            query.signup_date_from.as_deref(),
            query.signup_date_to.as_deref(),
            query.invite_source.as_deref(),
            query.status.as_deref(),
            query.cursor.as_deref(),
            limit + 1,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let has_more = rows.len() as i64 > limit;
    let page: Vec<_> = rows.into_iter().take(limit as usize).collect();
    let next_cursor = if has_more {
        page.last().map(|a| a.did.clone())
    } else {
        None
    };

    let mut accounts = Vec::with_capacity(page.len());
    for actor in &page {
        if let Ok(info) = build_account_info(&ctx, &actor.did).await {
            accounts.push(info);
        }
    }

    Ok(Json(OpsListAccountsResponse {
        accounts,
        cursor: next_cursor,
    }))
}

// ---- tools.aurora.ops.getInstanceMetrics (chainlink #84 / Phase 2.3.8) ----

/// Aggregated operator-flavored metrics for the instance.
///
/// Fields that aren't populated from existing instrumentation are omitted
/// rather than zero-filled, so absence is meaningful (e.g. no relay client
/// configured → federation_health.relay_connected is false, but the field
/// itself is always present; cpu_seconds_total may be None on platforms
/// where prometheus doesn't surface process-level counters).
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OpsInstanceMetrics {
    system_health: OpsSystemHealth,
    resource_usage: OpsResourceUsage,
    account_growth: OpsAccountGrowth,
    federation_health: OpsFederationHealth,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OpsSystemHealth {
    /// "healthy" if a SELECT 1 against the account DB succeeds.
    status: &'static str,
    version: String,
    uptime_seconds: f64,
    active_http_requests: i64,
    active_sessions: i64,
    active_background_jobs: i64,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OpsResourceUsage {
    /// Process resident memory in bytes (None when prometheus collector
    /// hasn't surfaced this counter — uncommon).
    #[serde(skip_serializing_if = "Option::is_none")]
    memory_resident_bytes: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cpu_seconds_total: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    open_fds: Option<i64>,
    db_pool_size: u32,
    db_pool_idle_connections: u32,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OpsAccountGrowth {
    signups_last_24h: i64,
    signups_last_7d: i64,
    signups_last_30d: i64,
    total_accounts: i64,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OpsFederationHealth {
    federation_enabled: bool,
    relay_connected: bool,
    /// Known peer count from the federation registry; 0 when federation
    /// is disabled or the registry is empty.
    known_instances: i64,
}

/// Operator-facing aggregate metrics endpoint.
///
/// Aggregates from sources Aurora-Locus already tracks (metrics module,
/// prometheus gauges, db pool stats, simple SQL counts). No new
/// instrumentation is added here — fields that aren't tracked end up as
/// `None` (omitted) rather than zero-filled.
async fn ops_get_instance_metrics(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<OpsInstanceMetrics>, (StatusCode, String)> {
    use crate::metrics;

    // ---- System health ----
    let db_healthy = sqlx::query("SELECT 1")
        .fetch_one(&ctx.account_db)
        .await
        .is_ok();
    let system_health = OpsSystemHealth {
        status: if db_healthy { "healthy" } else { "unhealthy" },
        version: ctx.config.service.version.clone(),
        uptime_seconds: metrics::UPTIME_SECONDS.get(),
        active_http_requests: metrics::HTTP_REQUESTS_ACTIVE.get(),
        active_sessions: metrics::SESSIONS_ACTIVE.get(),
        active_background_jobs: metrics::BACKGROUND_JOBS_ACTIVE.get(),
    };

    // ---- Resource usage (prometheus process metrics) ----
    let metric_families = prometheus::gather();
    let mut memory_resident_bytes = None;
    let mut cpu_seconds_total = None;
    let mut open_fds = None;
    for mf in &metric_families {
        match mf.name() {
            "process_resident_memory_bytes" => {
                if let Some(m) = mf.get_metric().first() {
                    memory_resident_bytes = Some(m.get_gauge().value());
                }
            }
            "process_cpu_seconds_total" => {
                if let Some(m) = mf.get_metric().first() {
                    cpu_seconds_total = Some(m.get_counter().value());
                }
            }
            "process_open_fds" => {
                if let Some(m) = mf.get_metric().first() {
                    open_fds = Some(m.get_gauge().value() as i64);
                }
            }
            _ => {}
        }
    }
    let resource_usage = OpsResourceUsage {
        memory_resident_bytes,
        cpu_seconds_total,
        open_fds,
        db_pool_size: ctx.account_db.size(),
        db_pool_idle_connections: ctx.account_db.num_idle() as u32,
    };

    // ---- Account growth (windowed counts) ----
    let now = chrono::Utc::now();
    let cutoff_24h = (now - chrono::Duration::hours(24)).to_rfc3339();
    let cutoff_7d = (now - chrono::Duration::days(7)).to_rfc3339();
    let cutoff_30d = (now - chrono::Duration::days(30)).to_rfc3339();

    let signups_last_24h: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM actor WHERE created_at > ?",
    )
    .bind(&cutoff_24h)
    .fetch_one(&ctx.account_db)
    .await
    .unwrap_or(0);
    let signups_last_7d: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM actor WHERE created_at > ?",
    )
    .bind(&cutoff_7d)
    .fetch_one(&ctx.account_db)
    .await
    .unwrap_or(0);
    let signups_last_30d: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM actor WHERE created_at > ?",
    )
    .bind(&cutoff_30d)
    .fetch_one(&ctx.account_db)
    .await
    .unwrap_or(0);
    let total_accounts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM actor")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap_or(0);

    let account_growth = OpsAccountGrowth {
        signups_last_24h,
        signups_last_7d,
        signups_last_30d,
        total_accounts,
    };

    // ---- Federation health ----
    // Known instances come from the in-memory pds_discovery registry
    // (not a SQL table). 0 when federation is disabled or discovery is
    // not configured.
    let known_instances = if let Some(ref discovery) = ctx.pds_discovery {
        discovery.get_known_instances().await.len() as i64
    } else {
        0
    };

    let federation_health = OpsFederationHealth {
        federation_enabled: ctx.config.federation.enabled,
        relay_connected: ctx.relay_client.is_some(),
        known_instances,
    };

    Ok(Json(OpsInstanceMetrics {
        system_health,
        resource_usage,
        account_growth,
        federation_health,
    }))
}

/// Get details about a single account (lexicon `com.atproto.admin.getAccountInfo`).
///
/// Thin wrapper around the same `build_account_info` helper used by
/// `get_account_infos`. The `accountView` shape is shared so future fixes
/// from chainlink #64 (Phase 1.9) propagate to both endpoints at once.
async fn get_account_info(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<GetAccountInfoQuery>,
) -> Result<Json<AccountInfo>, axum::response::Response> {
    use axum::response::IntoResponse;

    if !query.did.starts_with("did:") {
        return Err((
            StatusCode::BAD_REQUEST,
            "did must be a DID identifier".to_string(),
        )
            .into_response());
    }

    match build_account_info(&ctx, &query.did).await {
        Ok(info) => Ok(Json(info)),
        Err(PdsError::NotFound(_)) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "RepoNotFound",
                "message": format!("Account not found: {}", query.did),
            })),
        )
            .into_response()),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()),
    }
}

/// Polymorphic subject for `updateSubjectStatus` and `getSubjectStatus`.
///
/// Lexicon-conformant union of `com.atproto.admin.defs#repoRef`,
/// `com.atproto.repo.strongRef`, and `com.atproto.admin.defs#repoBlobRef`,
/// internally-tagged via the `$type` discriminator per the ATProto JSON
/// convention. Phase 1.6 (#61) introduced this; the existing `SubjectRef`
/// struct used by `getSubjectStatus`'s response is left in place since
/// changing its shape would touch separate scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "$type")]
// Variant names mirror the lexicon's union member names verbatim;
// clippy's enum-variant-names lint flags the shared `Ref` postfix but
// renaming would diverge from the spec namespace.
#[allow(clippy::enum_variant_names)]
enum SubjectUnion {
    #[serde(rename = "com.atproto.admin.defs#repoRef")]
    RepoRef { did: String },
    #[serde(
        rename = "com.atproto.repo.strongRef",
        rename_all = "camelCase"
    )]
    StrongRef { uri: String, cid: String },
    // §6.4.0.5: emit `record_uri` (snake_case) on the wire to
    // byte-match `Subject::Blob` in src/admin/defs.rs:37-43.
    // Shipped Aurora-namespace handlers (getAuditTrail,
    // subscribeModEvents, etc.) emit Subject::Blob's snake_case
    // shape; SubjectUnion is the input/parsing dual on
    // updateSubjectStatus and must accept and re-emit the same
    // wire bytes. `rename_all = "camelCase"` would re-emit
    // `recordUri`, drifting from the shipped contract — drop it.
    // The other fields in this variant (did, cid) are single
    // words and unaffected by the absent `rename_all`.
    #[serde(rename = "com.atproto.admin.defs#repoBlobRef")]
    RepoBlobRef {
        did: String,
        cid: String,
        // Arc 6 Step 7 dual-shape acceptance (V04_DESIGN §5.3.6):
        // accept both the canonical `record_uri` (snake_case, the
        // v0.3 wire byte form) and the legacy `recordUri` (camelCase,
        // v0.2 wire byte form). The `alias` attribute makes serde
        // parse either form into this single field; serialization
        // continues to emit only `record_uri` per the byte-equality
        // contract above. Detection of WHICH form was used happens
        // at the request level in `UpdateSubjectStatusRequest`'s
        // custom Deserialize, which inspects the raw JSON.
        #[serde(default, alias = "recordUri", skip_serializing_if = "Option::is_none")]
        record_uri: Option<String>,
    },
}

/// Request shape for `com.atproto.admin.updateSubjectStatus` (Phase 1.6).
///
/// Replaces the legacy imperative `{subject: string, action, duration}`
/// shape with the spec-conformant declarative status-patch model. Both
/// `takedown` and `deactivated` are optional patches; restore is implicit
/// via `takedown: {applied: false}`.
///
/// **Dual-shape acceptance** (Arc 6 Step 7, V04_DESIGN §5.3.6):
/// `RepoBlobRef` subjects accept both the canonical `record_uri`
/// (snake_case) and the legacy `recordUri` (camelCase) field
/// names. The custom Deserialize impl peeks at the raw JSON to
/// detect which form was used, sets `legacy_record_uri_used`
/// accordingly, and rejects requests that include both forms
/// simultaneously. The handler reads the flag to record a
/// legacy-wire-shape counter increment.
#[derive(Debug)]
struct UpdateSubjectStatusRequest {
    subject: SubjectUnion,
    takedown: Option<StatusAttr>,
    deactivated: Option<StatusAttr>,
    /// True when the request's `RepoBlobRef` subject (if any) used
    /// the legacy `recordUri` camelCase field rather than the
    /// canonical `record_uri` snake_case. Not part of the wire
    /// shape; set by the Deserialize impl; consumed by the handler
    /// to record a [`crate::metrics::record_legacy_wire_ingest`]
    /// increment.
    legacy_record_uri_used: bool,
}

/// Wire-side scaffold for [`UpdateSubjectStatusRequest`]'s custom
/// Deserialize. Mirrors the original derive without the legacy flag.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSubjectStatusRequestRaw {
    subject: SubjectUnion,
    #[serde(default)]
    takedown: Option<StatusAttr>,
    #[serde(default)]
    deactivated: Option<StatusAttr>,
}

impl<'de> Deserialize<'de> for UpdateSubjectStatusRequest {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        // Parse to Value first so we can peek at the raw subject
        // object's key set BEFORE serde normalizes the `recordUri`
        // alias into `record_uri`. The alias attribute on the
        // RepoBlobRef variant accepts both names transparently, but
        // it does NOT tell us which one was actually present —
        // that's what this peek is for.
        let value = serde_json::Value::deserialize(d)?;

        // Look at the subject object's keys when the variant is
        // RepoBlobRef. If both `record_uri` and `recordUri` are
        // present, reject — the alias would otherwise silently pick
        // one and the operator wouldn't see the ambiguity.
        let (legacy_record_uri_used, has_both) = value
            .as_object()
            .and_then(|obj| obj.get("subject"))
            .and_then(|s| s.as_object())
            .map(|s| {
                let is_blob = s.get("$type").and_then(|v| v.as_str())
                    == Some("com.atproto.admin.defs#repoBlobRef");
                if !is_blob {
                    return (false, false);
                }
                let has_canonical = s.contains_key("record_uri");
                let has_legacy = s.contains_key("recordUri");
                (has_legacy && !has_canonical, has_canonical && has_legacy)
            })
            .unwrap_or((false, false));

        if has_both {
            return Err(D::Error::custom(
                "RepoBlobRef subject accepts either canonical 'record_uri' \
                 (snake_case) or legacy 'recordUri' (camelCase), not both; \
                 pick exactly one shape per request",
            ));
        }

        // Standard deserialize via the scaffold; the `alias` attribute
        // on RepoBlobRef.record_uri handles the field-name folding.
        let raw: UpdateSubjectStatusRequestRaw =
            serde_json::from_value(value).map_err(D::Error::custom)?;
        Ok(UpdateSubjectStatusRequest {
            subject: raw.subject,
            takedown: raw.takedown,
            deactivated: raw.deactivated,
            legacy_record_uri_used,
        })
    }
}

/// Response shape for `com.atproto.admin.updateSubjectStatus`.
///
/// Per the lexicon: subject (required) plus an optional `takedown` echoed
/// back. The lexicon does *not* echo `deactivated` in the output — we
/// match the spec exactly.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSubjectStatusResponse {
    subject: SubjectUnion,
    #[serde(skip_serializing_if = "Option::is_none")]
    takedown: Option<StatusAttr>,
}

/// Update the service-specific admin status of a subject (lexicon
/// `com.atproto.admin.updateSubjectStatus`).
///
/// Phase 1.6 (#61) replaced the imperative-action model with the
/// declarative status-patch model per spec. Subject dispatch:
/// - `repoRef`: account-level. Both `takedown` and `deactivated` patches
///   are honored, mapped to `account_manager` setters.
/// - `repoBlobRef`: blob-level. `takedown` is honored via `BlobQuarantine`;
///   `deactivated` is rejected (400 InvalidRequest) since it isn't
///   applicable to blobs.
/// - `strongRef`: record-level. `takedown` returns 501 (no setter exists
///   yet — tracked under a follow-up); `deactivated` is rejected (400
///   InvalidRequest) since records aren't a deactivable concept.
async fn update_subject_status(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    crate::api::extractors::AuroraJson(req): crate::api::extractors::AuroraJson<UpdateSubjectStatusRequest>,
) -> Result<Json<UpdateSubjectStatusResponse>, axum::response::Response> {
    use axum::response::IntoResponse;

    // Arc 6 Step 7: legacy wire-shape observability. When the request
    // used the legacy camelCase `recordUri` on a RepoBlobRef subject,
    // record a counter + structured-log line. See the matching
    // emit_event handler comment for the deviation from the kickoff's
    // response-header pattern.
    if req.legacy_record_uri_used {
        crate::metrics::record_legacy_wire_ingest(
            "com.atproto.admin.updateSubjectStatus",
            "v0.2_camelCase_record_uri",
            "recordUri",
        );
        tracing::info!(
            endpoint = "com.atproto.admin.updateSubjectStatus",
            shape = "v0.2_camelCase_record_uri",
            field = "recordUri",
            "legacy_wire_shape_ingested"
        );
    }

    let UpdateSubjectStatusRequest {
        subject,
        takedown,
        deactivated,
        legacy_record_uri_used: _,
    } = req;

    // Per §3.4 "one decision = one chain entry": each updateSubjectStatus
    // call produces exactly one chain row whose rationale lists the
    // patches applied. Account branch may have both takedown and
    // deactivated effects; the chain entry coalesces them.
    //
    // Compute chain_subject upfront so we can capture the snapshot
    // BEFORE opening the wrapping tx. Otherwise on a single-connection
    // pool (SQLite test config) the snapshot's connection acquisition
    // deadlocks against the held tx connection.
    let chain_subject = match &subject {
        SubjectUnion::RepoRef { did } => Subject::Repo { did: did.clone() },
        SubjectUnion::RepoBlobRef { did, cid, .. } => Subject::Blob {
            did: did.clone(),
            cid: cid.clone(),
            record_uri: None,
        },
        SubjectUnion::StrongRef { uri, cid } => Subject::Record {
            uri: uri.clone(),
            cid: cid.clone(),
        },
    };
    let snapshot_id = audit_chain::capture_snapshot(&ctx.account_db, &chain_subject)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;

    // LB-1 Session 12 / chainlink #129: account branch + chain entry
    // wrapped in one transaction. Blob branch's quarantine writes
    // live in a separate store layer (BlobQuarantine manages its own
    // tx) so the wrapping tx covers only the chain entry on that
    // path — the blob quarantine remains atomic at its own layer per
    // §3.4's "primary mutation" reading.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;

    let (response_takedown, effects) = match &subject {
        SubjectUnion::RepoRef { did } => apply_account_status_in_tx(
            &mut tx,
            &auth,
            did,
            takedown.as_ref(),
            deactivated.as_ref(),
        )
        .await
        .map_err(|(s, m)| (s, m).into_response())?,

        SubjectUnion::RepoBlobRef { did, cid, .. } => {
            // Blobs don't have a deactivation concept — reject so the
            // caller learns their patch wasn't silently dropped.
            if deactivated.is_some() {
                return Err(xrpc_invalid_request_error(
                    "deactivated patch is not applicable to blob subjects; \
                     only takedown applies to blobs",
                ));
            }
            // Arc 4 §8.4.0.5 / chainlink #131: thread the wrapping
            // tx into apply_blob_status so the quarantine/restore
            // operations + the chain entry land atomically. Pre-Arc-4
            // this site released the held tx, called the pool-API
            // quarantine layer (which opened its own tx), then
            // reopened a fresh tx for the chain entry — fragile and
            // non-atomic.
            let (resp, fx) =
                apply_blob_status(&ctx, &mut tx, &auth, did, cid, takedown.as_ref()).await?;
            (resp, fx)
        }

        SubjectUnion::StrongRef { .. } => {
            if deactivated.is_some() {
                return Err(xrpc_invalid_request_error(
                    "deactivated patch is not applicable to record subjects; \
                     only takedown applies to records",
                ));
            }
            return Err((
                StatusCode::NOT_IMPLEMENTED,
                "Record-level (strongRef) subject takedown is not yet implemented; \
                 the actor-store record.takedown_ref column exists but has no setter.",
            )
                .into_response());
        }
    };

    // Skip the chain write when no patches were supplied — the call was a
    // no-op (the response still echoes current takedown state, which is
    // useful, but there's no decision to record).
    if !effects.is_empty() {
        let rationale = effects.join("; ");
        audit_chain::insert_chain_entry(
            &mut tx,
            ctx.config.database.backend,
            AppendEntryParams {
                source: "manual",
                payload: None,
                actor_did: &auth.did,
                action: "subject.update_status",
                subject: Some(&chain_subject),
                rationale: &rationale,
                snapshot_id,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;
    }

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;

    // Arc 15 §8.3.4 / §8.3.5 / §8.3.6 — chainlink #85: fire
    // sequencer emits for account-level state mutations applied via
    // this endpoint. Pre-fix, the canonical lex path bypassed the
    // handler-level emits wired into `deactivate_account` /
    // `activate_account` / `takedown_account`, so downstream
    // federation peers wouldn't see takedown / deactivate applied
    // via updateSubjectStatus.
    //
    // Emits run post-commit (matching the handler-level pattern)
    // and operate on the post-patch row state (Pattern B). Only the
    // RepoRef branch is account-level; RepoBlobRef and StrongRef
    // are blob/record concerns with no #account emit.
    //
    // Reverse-takedown (takedown.applied: false) is the documented
    // §8.1.2 v0.5 deferral — no emit fires. Tracked for v0.6.
    if let SubjectUnion::RepoRef { did } = &subject {
        // Read post-patch row once; we may use it for two emits below.
        let acc_post_opt = ctx.account_manager.get_account(did).await.ok();

        // Takedown apply path → #account Takendown.
        if let Some(td) = &takedown {
            if td.applied {
                if let Some(ref acc_post) = acc_post_opt {
                    let (active, status) =
                        crate::api::sync_helpers::get_account_status(acc_post);
                    if let Err(e) = ctx
                        .sequencer
                        .sequence_account(crate::sequencer::events::AccountEvent {
                            did: did.clone(),
                            active,
                            status,
                        })
                        .await
                    {
                        tracing::warn!(
                            did = %did,
                            error = %e,
                            "updateSubjectStatus: takedown emit failed (state mutated OK)"
                        );
                    }
                }
            } else {
                // chainlink #179: reverse-takedown (takedown.applied = false)
                // now emits #account (active=true post-restore), symmetrizing
                // with the takedown-apply emit above. Previously a §8.1.2 v0.5
                // deferral that left downstream subscribers in stale-takedown
                // state after a restore.
                if let Some(ref acc_post) = acc_post_opt {
                    let (active, status) =
                        crate::api::sync_helpers::get_account_status(acc_post);
                    if let Err(e) = ctx
                        .sequencer
                        .sequence_account(crate::sequencer::events::AccountEvent {
                            did: did.clone(),
                            active,
                            status,
                        })
                        .await
                    {
                        tracing::warn!(
                            did = %did,
                            error = %e,
                            "updateSubjectStatus: reverse-takedown emit failed (state mutated OK)"
                        );
                    }
                }
            }
        }

        // Deactivate / reactivate path.
        if let Some(d) = &deactivated {
            if d.applied {
                // §8.3.4 deactivate → #account Deactivated.
                if let Some(ref acc_post) = acc_post_opt {
                    let (active, status) =
                        crate::api::sync_helpers::get_account_status(acc_post);
                    if let Err(e) = ctx
                        .sequencer
                        .sequence_account(crate::sequencer::events::AccountEvent {
                            did: did.clone(),
                            active,
                            status,
                        })
                        .await
                    {
                        tracing::warn!(
                            did = %did,
                            error = %e,
                            "updateSubjectStatus: deactivate emit failed (state mutated OK)"
                        );
                    }
                }
            } else {
                // §8.3.5 reactivate → three-emit sequence
                // (account → identity → sync).
                if let Some(ref acc_post) = acc_post_opt {
                    let (active, status) =
                        crate::api::sync_helpers::get_account_status(acc_post);
                    if let Err(e) = ctx
                        .sequencer
                        .sequence_account(crate::sequencer::events::AccountEvent {
                            did: did.clone(),
                            active,
                            status,
                        })
                        .await
                    {
                        tracing::warn!(
                            did = %did,
                            error = %e,
                            "updateSubjectStatus: reactivate account emit failed"
                        );
                    }
                    if let Err(e) = ctx
                        .sequencer
                        .sequence_identity(crate::sequencer::events::IdentityEvent {
                            did: did.clone(),
                            handle: acc_post.handle.clone(),
                        })
                        .await
                    {
                        tracing::warn!(
                            did = %did,
                            error = %e,
                            "updateSubjectStatus: reactivate identity emit failed"
                        );
                    }
                    let repo_mgr = crate::actor_store::RepositoryManager::with_sequencer(
                        did.clone(),
                        ctx.actor_store.as_ref().clone(),
                        ctx.sequencer.clone(),
                    );
                    match repo_mgr.current_sync_event_data().await {
                        Ok(sync_data) => {
                            match crate::sequencer::events::SyncEvent::from_sync_data(
                                did.clone(),
                                sync_data,
                            ) {
                                Ok(sync_evt) => {
                                    if let Err(e) =
                                        ctx.sequencer.sequence_sync(sync_evt).await
                                    {
                                        tracing::warn!(
                                            did = %did,
                                            error = %e,
                                            "updateSubjectStatus: reactivate sync emit failed"
                                        );
                                    }
                                }
                                Err(e) => tracing::warn!(
                                    did = %did,
                                    error = %e,
                                    "updateSubjectStatus: reactivate sync formatter failed"
                                ),
                            }
                        }
                        Err(e) => tracing::warn!(
                            did = %did,
                            error = %e,
                            "updateSubjectStatus: reactivate no current sync data — skipping #sync"
                        ),
                    }
                }
            }
        }
    }

    Ok(Json(UpdateSubjectStatusResponse {
        subject,
        takedown: response_takedown,
    }))
}

/// One element of the rationale string for a `subject.update_status`
/// chain entry. Renders to a compact "<aspect>:<verb>[:<ref>]" form so
/// the rationale is parseable when later inspected.
fn render_status_effect(aspect: &str, applied: bool, ref_field: Option<&str>) -> String {
    let verb = if applied { "apply" } else { "remove" };
    match ref_field {
        Some(r) if !r.is_empty() => format!("{}:{}:{}", aspect, verb, r),
        _ => format!("{}:{}", aspect, verb),
    }
}

/// Build a structured XRPC `InvalidRequest` 400 response per the atproto
/// error convention `{"error": "InvalidRequest", "message": "..."}`.
fn xrpc_invalid_request_error(message: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": "InvalidRequest",
            "message": message,
        })),
    )
        .into_response()
}

/// Build a structured XRPC `BlobNotFound` 404 response.
fn xrpc_blob_not_found_error(cid: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": "BlobNotFound",
            "message": format!("Blob not found: cid={}", cid),
        })),
    )
        .into_response()
}

/// Apply takedown/deactivated patches to an account. Returns the post-patch
/// takedown status (to echo back in the response) plus a list of patch
/// effects (rendered strings) for the caller to roll into a single chain
/// entry. Per §3.4, the chain row belongs at the handler level so the
/// "one decision = one chain entry" framing holds even when both patches
/// are present.
async fn apply_account_status_in_tx<'c>(
    tx: &mut sqlx::Transaction<'c, sqlx::Any>,
    auth: &AdminAuthContext,
    did: &str,
    takedown: Option<&StatusAttr>,
    deactivated: Option<&StatusAttr>,
) -> Result<(Option<StatusAttr>, Vec<String>), (StatusCode, String)> {
    let mut effects: Vec<String> = Vec::new();

    if let Some(td) = takedown {
        if td.applied {
            // Use the caller-supplied `ref` if present, otherwise generate one
            // from the admin DID + timestamp so audit trails always have a key.
            let takedown_ref = td.ref_field.clone().unwrap_or_else(|| {
                format!("auto-{}-{}", chrono::Utc::now().timestamp(), auth.did)
            });
            crate::account::AccountManager::takedown_account_in_tx(tx, did, &takedown_ref)
                .await
                .map_err(map_account_err(did))?;
            effects.push(render_status_effect(
                "takedown",
                true,
                Some(&takedown_ref),
            ));
        } else {
            crate::account::AccountManager::activate_account_in_tx(tx, did)
                .await
                .map_err(map_account_err(did))?;
            effects.push(render_status_effect(
                "takedown",
                false,
                td.ref_field.as_deref(),
            ));
        }
    }

    if let Some(d) = deactivated {
        if d.applied {
            crate::account::AccountManager::deactivate_account_in_tx(tx, did)
                .await
                .map_err(map_account_err(did))?;
        } else {
            crate::account::AccountManager::reactivate_account_in_tx(tx, did)
                .await
                .map_err(map_account_err(did))?;
        }
        effects.push(render_status_effect(
            "deactivate",
            d.applied,
            d.ref_field.as_deref(),
        ));
    }

    // Read fresh state so the response reflects post-patch reality.
    // Reading from the same tx (via SELECT ... FROM actor) sees the
    // patches we just applied.
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT takedown_ref FROM actor WHERE did = $1")
            .bind(did)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let takedown_ref = row.and_then(|(t,)| t);
    Ok((
        Some(StatusAttr {
            applied: takedown_ref.is_some(),
            ref_field: takedown_ref,
        }),
        effects,
    ))
}

/// Apply a takedown patch to a blob via the existing quarantine machinery.
///
/// Verifies the blob exists in `BlobStore` before any quarantine action so
/// that operating on a non-existent CID returns 404 BlobNotFound rather
/// than silently no-op'ing through the idempotency path. Already-in-state
/// cases (already quarantined when applying, not quarantined when removing)
/// are treated as idempotent success.
async fn apply_blob_status<'tx>(
    ctx: &AppContext,
    tx: &mut sqlx::Transaction<'tx, sqlx::Any>,
    auth: &AdminAuthContext,
    did: &str,
    cid: &str,
    takedown: Option<&StatusAttr>,
) -> Result<(Option<StatusAttr>, Vec<String>), axum::response::Response> {
    use crate::blob_store::quarantine::{BlobQuarantine, QuarantineReason};
    use axum::response::IntoResponse;

    // Suppress the unused-warning when no caller currently needs `did` —
    // it's the chain-entry's subject DID and lives in update_subject_status.
    let _ = did;

    // Establish that the blob actually exists. `BlobStore::get_metadata`
    // returns Some(_) iff the blob is registered; missing → 404.
    // (Pool-API read; safe outside the wrapping tx because we're only
    // reading metadata, not branching on quarantine state.)
    let exists = ctx
        .blob_store
        .get_metadata(cid)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?
        .is_some();
    if !exists {
        return Err(xrpc_blob_not_found_error(cid));
    }

    let mut effects: Vec<String> = Vec::new();

    // Track post-state to populate the response without a stale read
    // against the pool (an `is_quarantined` SELECT against `&self.db`
    // wouldn't see the wrapping tx's pending writes). When `takedown`
    // is None, no patch is applied and the blob's pre-call state is
    // returned via the post-tx-commit pool read at the call site.
    let mut post_state_taken_down: Option<bool> = None;

    if let Some(td) = takedown {
        if td.applied {
            // Already-quarantined → Conflict from the quarantine layer →
            // idempotent success since the desired post-state already obtains.
            // Arc 4 §8.4.0.5: in-tx variant so the quarantine row + the
            // chain entry the caller writes land atomically.
            match BlobQuarantine::quarantine_blob_in_tx(
                tx,
                cid,
                QuarantineReason::Other,
                td.ref_field.as_deref(),
                &auth.did,
                None,
            )
            .await
            {
                Ok(_) | Err(PdsError::Conflict(_)) => {}
                Err(e) => {
                    return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())
                }
            }
            post_state_taken_down = Some(true);
        } else {
            // Not-currently-quarantined → NotFound from `restore_blob` →
            // idempotent success (operator wanted "ensure not quarantined";
            // we already are).
            match BlobQuarantine::restore_blob_in_tx(tx, cid, &auth.did).await {
                Ok(_) | Err(PdsError::NotFound(_)) => {}
                Err(e) => {
                    return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())
                }
            }
            post_state_taken_down = Some(false);
        }
        effects.push(render_status_effect(
            "takedown",
            td.applied,
            td.ref_field.as_deref().or(Some(cid)),
        ));
    }

    // Determine the wire response's `applied` flag. When a takedown
    // patch was applied, we know the post-state from the operation
    // (quarantine call set true; restore call set false). When
    // takedown is None, query the pool for the current state — no
    // pending writes exist on this tx for this cid.
    let is_taken_down = match post_state_taken_down {
        Some(state) => state,
        None => {
            let quarantine = BlobQuarantine::new(ctx.account_db.clone());
            quarantine
                .is_quarantined(cid)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?
        }
    };
    Ok((
        Some(StatusAttr {
            applied: is_taken_down,
            ref_field: takedown.and_then(|td| td.ref_field.clone()),
        }),
        effects,
    ))
}

/// Map an account-manager error to an HTTP status, matching the pattern
/// established by other admin handlers (NotFound → 404, otherwise 500).
fn map_account_err(did: &str) -> impl Fn(PdsError) -> (StatusCode, String) + '_ {
    move |e| {
        if matches!(e, PdsError::NotFound(_)) {
            (
                StatusCode::NOT_FOUND,
                format!("Account not found: {}", did),
            )
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    }
}

#[derive(Deserialize)]
struct GetSubjectStatusQuery {
    /// The DID or AT-URI of the subject to query
    #[serde(default)]
    did: Option<String>,
    /// The AT-URI of the subject (alternative to did for record-level status)
    #[serde(default)]
    uri: Option<String>,
    /// The CID of the blob (for blob-level status)
    #[serde(default)]
    blob: Option<String>,
}

/// Subject status response matching ATProto spec
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SubjectStatusResponse {
    subject: SubjectRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    takedown: Option<StatusAttr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deactivated: Option<StatusAttr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    suspended: Option<StatusAttr>,
}

/// Reference to the subject (repo, record, or blob)
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SubjectRef {
    #[serde(rename = "$type")]
    type_field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    did: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cid: Option<String>,
}

/// Status attribute with applied flag and optional reference
/// (lexicon `com.atproto.admin.defs#statusAttr`).
///
/// Used on the request side for `updateSubjectStatus` (per Phase 1.6 / #61)
/// and on the response side for both `getSubjectStatus` and
/// `updateSubjectStatus`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusAttr {
    applied: bool,
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    ref_field: Option<String>,
}

/// Get subject status (takedown/deactivation status of account or record)
///
/// This endpoint returns the current moderation status of a subject,
/// including whether it's been taken down, deactivated, or suspended.
async fn get_subject_status(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(query): Query<GetSubjectStatusQuery>,
) -> Result<Json<SubjectStatusResponse>, (StatusCode, String)> {
    // Determine subject type and extract DID
    let (subject_type, did, uri) = if let Some(ref did_str) = query.did {
        // Direct DID query - repo subject
        if !did_str.starts_with("did:") {
            return Err((StatusCode::BAD_REQUEST, "Invalid DID format".to_string()));
        }
        ("com.atproto.admin.defs#repoRef", did_str.clone(), None)
    } else if let Some(ref uri_str) = query.uri {
        // AT-URI query - record subject
        if !uri_str.starts_with("at://") {
            return Err((StatusCode::BAD_REQUEST, "Invalid AT-URI format".to_string()));
        }
        // Extract DID from AT-URI
        let did = uri_str
            .trim_start_matches("at://")
            .split('/')
            .next()
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "Invalid AT-URI format".to_string()))?
            .to_string();
        if !did.starts_with("did:") {
            return Err((
                StatusCode::BAD_REQUEST,
                "AT-URI must contain a DID".to_string(),
            ));
        }
        ("com.atproto.repo.strongRef", did, Some(uri_str.clone()))
    } else if let Some(ref _blob_cid) = query.blob {
        // Blob query - not yet implemented
        return Err((
            StatusCode::NOT_IMPLEMENTED,
            "Blob status queries not yet implemented".to_string(),
        ));
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Must provide did, uri, or blob parameter".to_string(),
        ));
    };

    // Get account info from account manager
    let account = ctx.account_manager.get_account(&did).await.map_err(|e| {
        if matches!(e, PdsError::NotFound(_)) {
            (StatusCode::NOT_FOUND, format!("Subject not found: {}", did))
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    })?;

    // Check moderation status
    let is_suspended = ctx
        .moderation_manager
        .is_suspended(&did)
        .await
        .unwrap_or(false);

    // Build response
    let subject = SubjectRef {
        type_field: subject_type.to_string(),
        did: if query.did.is_some() {
            Some(did.clone())
        } else {
            None
        },
        uri,
        cid: None,
    };

    let takedown = if account.takedown_ref.is_some() {
        Some(StatusAttr {
            applied: true,
            ref_field: account.takedown_ref.clone(),
        })
    } else {
        Some(StatusAttr {
            applied: false,
            ref_field: None,
        })
    };

    let deactivated = if account.deactivated_at.is_some() {
        Some(StatusAttr {
            applied: true,
            ref_field: account
                .deactivated_at
                .map(|dt: chrono::DateTime<chrono::Utc>| dt.to_rfc3339()),
        })
    } else {
        Some(StatusAttr {
            applied: false,
            ref_field: None,
        })
    };

    let suspended = if is_suspended {
        Some(StatusAttr {
            applied: true,
            ref_field: None,
        })
    } else {
        Some(StatusAttr {
            applied: false,
            ref_field: None,
        })
    };

    Ok(Json(SubjectStatusResponse {
        subject,
        takedown,
        deactivated,
        suspended,
    }))
}

#[derive(Deserialize)]
struct GetModerationQueueQuery {
    /// Queue header status filter (#209). Absent → open-only (prior default);
    /// `all` → every status; otherwise a report status, else `400`.
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

/// Get moderation queue (reports needing review)
async fn get_moderation_queue(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Query(query): Query<GetModerationQueueQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::admin::reports::{AssignmentScope, ReportStatus};
    use crate::admin::roles::Role;

    // Resolve the header status filter (#209). No param preserves the prior
    // hardcoded open-only queue; `all` widens to every status; an unknown
    // value is a 400 rather than a silently-ignored decorative filter.
    let status_filter = ReportStatus::queue_filter_from_param(query.status.as_deref())
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // §5.5.4 §4.5 queue scope: SuperAdmin sees every item; everyone else
    // sees items assigned to them plus the unassigned pool.
    let scope = if auth.role.can_act_as(Role::SuperAdmin) {
        AssignmentScope::All
    } else {
        AssignmentScope::AssignedTo(&auth.did)
    };

    let reports = ctx
        .report_manager
        .list_reports_scoped(status_filter, query.limit, scope)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "queue": reports,
        "count": reports.len(),
    })))
}

#[derive(Deserialize)]
struct DisableInviteCodeRequest {
    code: String,
}

/// Disable an invite code
async fn disable_invite_code(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<DisableInviteCodeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let rationale = format!("disable invite code {}", req.code);

    // LB-1 Session 12 / chainlink #129: invite_code UPDATE + chain
    // entry in one transaction.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::admin::invites::InviteCodeManager::disable_code_in_tx(&mut tx, &req.code)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (StatusCode::NOT_FOUND, e.to_string())
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "invite.disable",
            subject: None,
            rationale: &rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "code": req.code,
    })))
}

#[derive(Deserialize)]
struct DisableInviteCodesRequest {
    /// Specific invite codes to disable. Missing codes are silently skipped.
    #[serde(default)]
    codes: Vec<String>,
    /// Account DIDs whose issued invite codes should all be disabled.
    /// Matches `invite_code.for_account` (the intended recipient).
    #[serde(default)]
    accounts: Vec<String>,
}

/// Disable a batch of invite codes and/or all codes issued for a set of
/// accounts (lexicon `com.atproto.admin.disableInviteCodes`).
///
/// Updates run in a single SQLite transaction so a moderator working through
/// a spam ring gets all-or-nothing semantics rather than a partial commit.
/// Empty `codes` and `accounts` is a successful no-op per the lexicon (both
/// fields are optional with no `required` array).
async fn disable_invite_codes(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<DisableInviteCodesRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Empty input is a successful no-op per the lexicon — skip the
    // tx entirely.
    if req.codes.is_empty() && req.accounts.is_empty() {
        return Ok(StatusCode::OK);
    }

    let rationale = format!(
        "disable {} invite code(s){}",
        req.codes.len(),
        if !req.accounts.is_empty() {
            format!(" + all codes for {} account(s)", req.accounts.len())
        } else {
            String::new()
        }
    );

    // LB-1 Session 12 / chainlink #129: batch disable + chain entry
    // in one transaction.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::admin::invites::InviteCodeManager::disable_codes_batch_in_tx(
        &mut tx,
        &req.codes,
        &req.accounts,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "invite.disable_batch",
            subject: None,
            rationale: &rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
struct AccountInvitesRequest {
    /// Account at-identifier (handle or DID) per the lexicon. Required if
    /// the legacy `did` field is not provided.
    #[serde(default)]
    account: Option<String>,
    /// DEPRECATED: legacy `did` field retained for back-compat. Use
    /// `account` instead. Continues to accept DID-form only. To be
    /// removed in a later minor version.
    #[serde(default)]
    did: Option<String>,
    /// Optional reason for the invites change (per lexicon). Persisted to
    /// the admin audit log.
    #[serde(default)]
    note: Option<String>,
}

/// Enable invite code creation for an account
async fn enable_account_invites(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<AccountInvitesRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let canonical_did =
        resolve_account_or_did(&ctx, req.account.as_deref(), req.did.as_deref()).await?;

    let subject = Subject::Repo {
        did: canonical_did.clone(),
    };
    // Snapshot the pre-mutation actor state outside the tx — the
    // snapshot is immutable evidence of state-at-decision; whether
    // the chain entry lands is a separate question. Snapshot rows
    // that aren't referenced from any chain row are vestigial but
    // harmless.
    let snapshot_id = audit_chain::capture_snapshot(&ctx.account_db, &subject)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let rationale = req.note.clone().unwrap_or_default();

    // LB-1 / chainlink #122: the actor mutation and chain entry
    // run in one transaction, with the in-process append guard
    // held across the commit so concurrent appenders serialize.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::account::AccountManager::enable_account_invites_in_tx(&mut tx, &canonical_did)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    format!("Account not found: {}", canonical_did),
                )
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "account.invites.enable",
            subject: Some(&subject),
            rationale: &rationale,
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::OK)
}

/// Disable invite code creation for an account
async fn disable_account_invites(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<AccountInvitesRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let canonical_did =
        resolve_account_or_did(&ctx, req.account.as_deref(), req.did.as_deref()).await?;

    let subject = Subject::Repo {
        did: canonical_did.clone(),
    };
    let snapshot_id = audit_chain::capture_snapshot(&ctx.account_db, &subject)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let rationale = req.note.clone().unwrap_or_default();

    // LB-1 / chainlink #122 — see enable_account_invites for
    // the atomicity rationale.
    let _chain_guard = audit_chain::AppendChainGuard::acquire().await;
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::account::AccountManager::disable_account_invites_in_tx(&mut tx, &canonical_did)
        .await
        .map_err(|e| {
            if matches!(e, PdsError::NotFound(_)) {
                (
                    StatusCode::NOT_FOUND,
                    format!("Account not found: {}", canonical_did),
                )
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;
    audit_chain::insert_chain_entry(
        &mut tx,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "account.invites.disable",
            subject: Some(&subject),
            rationale: &rationale,
            snapshot_id,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::OK)
}

// ============================================================================
// Validation Failures
// ============================================================================

#[derive(Debug, Deserialize)]
struct GetValidationFailuresQuery {
    did: String,
    collection: Option<String>,
    limit: Option<i64>,
}

async fn get_validation_failures(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<GetValidationFailuresQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let failures = ctx
        .actor_store
        .get_validation_failures(&params.did, params.collection.as_deref(), params.limit)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "did": params.did,
        "failures": failures,
        "count": failures.len(),
    })))
}

// ============================================================================
// System Health and Diagnostics Endpoints
// ============================================================================

/// Get overall system health status
async fn get_system_health(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::metrics;

    let uptime = metrics::UPTIME_SECONDS.get();

    // Check critical services
    let db_healthy = sqlx::query("SELECT 1")
        .fetch_one(&ctx.account_db)
        .await
        .is_ok();

    let sequencer_healthy = true; // Sequencer is always available if context exists

    // Check optional services
    let relay_connected = ctx.relay_client.is_some();
    let federation_enabled = ctx.config.federation.enabled;

    // Determine overall health
    let status = if db_healthy && sequencer_healthy {
        "healthy"
    } else {
        "unhealthy"
    };

    Ok(Json(serde_json::json!({
        "status": status,
        "version": ctx.config.service.version,
        "uptime_seconds": uptime,
        "services": {
            "database": if db_healthy { "healthy" } else { "unhealthy" },
            "sequencer": if sequencer_healthy { "healthy" } else { "unhealthy" },
            "relay": if relay_connected { "connected" } else { "disconnected" },
            "federation": if federation_enabled { "enabled" } else { "disabled" },
        },
        "active_http_requests": metrics::HTTP_REQUESTS_ACTIVE.get(),
        "active_sessions": metrics::SESSIONS_ACTIVE.get(),
    })))
}

/// Get database connection pool status
async fn get_database_status(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Get pool statistics
    let pool_size = ctx.account_db.size();
    let pool_connections = ctx.account_db.num_idle();

    // Try a test query to measure latency
    let start = std::time::Instant::now();
    let query_ok = sqlx::query("SELECT 1")
        .fetch_one(&ctx.account_db)
        .await
        .is_ok();
    let query_latency_ms = start.elapsed().as_millis();

    // Get database-level statistics
    let db_stats = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM account")
        .fetch_one(&ctx.account_db)
        .await
        .map(|(count,)| count)
        .unwrap_or(0);

    // chainlink #95: bind RFC-3339 from app code (see jobs/tasks.rs for rationale).
    let now_rfc3339 = chrono::Utc::now().to_rfc3339();
    let session_count = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM session WHERE expires_at > $1",
    )
    .bind(&now_rfc3339)
    .fetch_one(&ctx.account_db)
    .await
    .map(|(count,)| count)
    .unwrap_or(0);

    Ok(Json(serde_json::json!({
        "status": if query_ok { "healthy" } else { "unhealthy" },
        "pool": {
            "size": pool_size,
            "idle_connections": pool_connections,
            "active_connections": pool_size as i64 - pool_connections as i64,
        },
        "latency_ms": query_latency_ms,
        "statistics": {
            "total_accounts": db_stats,
            "active_sessions": session_count,
        }
    })))
}

/// Get resource usage metrics (CPU, memory)
async fn get_resource_usage(
    State(_ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Get process metrics from prometheus
    let metric_families = prometheus::gather();

    let mut memory_bytes: Option<f64> = None;
    let mut cpu_seconds_total: Option<f64> = None;
    let mut open_fds: Option<f64> = None;

    // Extract process metrics
    for mf in &metric_families {
        match mf.name() {
            "process_resident_memory_bytes" => {
                if let Some(m) = mf.get_metric().first() {
                    memory_bytes = Some(m.get_gauge().value());
                }
            }
            "process_cpu_seconds_total" => {
                if let Some(m) = mf.get_metric().first() {
                    cpu_seconds_total = Some(m.get_counter().value());
                }
            }
            "process_open_fds" => {
                if let Some(m) = mf.get_metric().first() {
                    open_fds = Some(m.get_gauge().value());
                }
            }
            _ => {}
        }
    }

    Ok(Json(serde_json::json!({
        "memory": {
            "resident_bytes": memory_bytes.unwrap_or(0.0),
            "resident_mb": memory_bytes.unwrap_or(0.0) / 1024.0 / 1024.0,
        },
        "cpu": {
            "seconds_total": cpu_seconds_total.unwrap_or(0.0),
        },
        "file_descriptors": {
            "open": open_fds.unwrap_or(0.0) as i64,
        }
    })))
}

/// List background jobs status
async fn list_background_jobs(
    State(_ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::metrics;

    // Get metrics about background jobs
    let active_jobs = metrics::BACKGROUND_JOBS_ACTIVE.get();

    // Get job execution counts from prometheus
    let metric_families = prometheus::gather();
    let mut job_stats = std::collections::HashMap::new();

    for mf in &metric_families {
        if mf.name() == "background_jobs_total" {
            for metric in mf.get_metric() {
                let mut job_type = "unknown";
                let mut status = "unknown";

                for label in metric.get_label() {
                    if label.name() == "job_type" {
                        job_type = label.value();
                    } else if label.name() == "status" {
                        status = label.value();
                    }
                }

                let count = metric.get_counter().value() as i64;
                let entry = job_stats.entry(job_type).or_insert_with(|| {
                    serde_json::json!({
                        "type": job_type,
                        "success": 0,
                        "failure": 0,
                        "total": 0,
                    })
                });

                if let Some(obj) = entry.as_object_mut() {
                    obj["total"] = serde_json::json!(obj["total"].as_i64().unwrap_or(0) + count);
                    if status == "success" {
                        obj["success"] = serde_json::json!(count);
                    } else if status == "failure" {
                        obj["failure"] = serde_json::json!(count);
                    }
                }
            }
        }
    }

    let jobs: Vec<_> = job_stats.values().cloned().collect();

    Ok(Json(serde_json::json!({
        "active_jobs": active_jobs,
        "job_statistics": jobs,
    })))
}

/// Run comprehensive health checks
async fn run_health_checks(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let start = std::time::Instant::now();
    let mut checks = Vec::new();

    // Database check
    let db_start = std::time::Instant::now();
    let db_healthy = sqlx::query("SELECT 1")
        .fetch_one(&ctx.account_db)
        .await
        .is_ok();
    checks.push(serde_json::json!({
        "component": "database",
        "status": if db_healthy { "healthy" } else { "unhealthy" },
        "response_time_ms": db_start.elapsed().as_millis(),
    }));

    // Blob storage check
    let blob_start = std::time::Instant::now();
    let _ = &ctx.blob_store; // Just verify it exists
    checks.push(serde_json::json!({
        "component": "blob_storage",
        "status": "healthy",
        "response_time_ms": blob_start.elapsed().as_millis(),
    }));

    // Sequencer check
    let seq_start = std::time::Instant::now();
    let _ = &ctx.sequencer;
    checks.push(serde_json::json!({
        "component": "sequencer",
        "status": "healthy",
        "response_time_ms": seq_start.elapsed().as_millis(),
    }));

    // Identity resolver check
    let identity_start = std::time::Instant::now();
    let _ = &ctx.identity_resolver;
    checks.push(serde_json::json!({
        "component": "identity_resolver",
        "status": "healthy",
        "response_time_ms": identity_start.elapsed().as_millis(),
    }));

    // Relay check (if enabled)
    if let Some(ref _relay) = ctx.relay_client {
        checks.push(serde_json::json!({
            "component": "relay_client",
            "status": "connected",
            "response_time_ms": 0,
        }));
    }

    // Email service check (if configured)
    let email_configured = ctx.config.email.is_some();
    if email_configured {
        checks.push(serde_json::json!({
            "component": "email_service",
            "status": "configured",
            "response_time_ms": 0,
        }));
    }

    // Determine overall status
    let all_healthy = checks.iter().all(|c| {
        c["status"] == "healthy" || c["status"] == "connected" || c["status"] == "configured"
    });

    Ok(Json(serde_json::json!({
        "overall_status": if all_healthy { "healthy" } else { "degraded" },
        "checks": checks,
        "total_duration_ms": start.elapsed().as_millis(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })))
}

/// Get version and build information
async fn get_version_info(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    Ok(Json(serde_json::json!({
        "version": ctx.config.service.version,
        "service_did": ctx.config.service.service_did,
        "hostname": ctx.config.service.hostname,
        "port": ctx.config.service.port,
        "rust_version": env!("CARGO_PKG_RUST_VERSION"),
        "build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "features": {
            "federation": ctx.config.federation.enabled,
            "invites_required": ctx.config.invites.required,
            "rate_limiting": ctx.config.rate_limit.enabled,
            "email": ctx.config.email.is_some(),
        }
    })))
}

/// Get comprehensive system metrics
async fn get_system_metrics(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::metrics;

    // Gather all Prometheus metrics
    let metric_families = prometheus::gather();

    // Extract key metrics
    let mut http_requests_total: i64 = 0;
    let mut db_queries_total: i64 = 0;
    let mut cache_hits: i64 = 0;
    let mut cache_misses: i64 = 0;
    let mut sequencer_current_seq: i64 = 0;
    let mut relay_events_total: i64 = 0;

    for mf in &metric_families {
        match mf.name() {
            "http_requests_total" => {
                for m in mf.get_metric() {
                    http_requests_total += m.get_counter().value() as i64;
                }
            }
            "db_queries_total" => {
                for m in mf.get_metric() {
                    db_queries_total += m.get_counter().value() as i64;
                }
            }
            "cache_hits_total" => {
                for m in mf.get_metric() {
                    cache_hits += m.get_counter().value() as i64;
                }
            }
            "cache_misses_total" => {
                for m in mf.get_metric() {
                    cache_misses += m.get_counter().value() as i64;
                }
            }
            "sequencer_current_seq" => {
                if let Some(m) = mf.get_metric().first() {
                    sequencer_current_seq = m.get_gauge().value() as i64;
                }
            }
            "relay_events_total" => {
                for m in mf.get_metric() {
                    relay_events_total += m.get_counter().value() as i64;
                }
            }
            _ => {}
        }
    }

    let cache_total = cache_hits + cache_misses;
    let cache_hit_rate = if cache_total > 0 {
        (cache_hits as f64 / cache_total as f64) * 100.0
    } else {
        0.0
    };

    Ok(Json(serde_json::json!({
        "uptime_seconds": metrics::UPTIME_SECONDS.get(),
        "http": {
            "requests_total": http_requests_total,
            "active_requests": metrics::HTTP_REQUESTS_ACTIVE.get(),
        },
        "database": {
            "queries_total": db_queries_total,
            "active_connections": metrics::DB_CONNECTIONS_ACTIVE.get(),
            "pool_size": ctx.account_db.size(),
        },
        "cache": {
            "hits": cache_hits,
            "misses": cache_misses,
            "hit_rate_percent": cache_hit_rate,
        },
        "sequencer": {
            "current_sequence": sequencer_current_seq,
            "events_total": metrics::SEQUENCER_EVENTS_TOTAL.with_label_values(&["commit"]).get() +
                           metrics::SEQUENCER_EVENTS_TOTAL.with_label_values(&["identity"]).get() +
                           metrics::SEQUENCER_EVENTS_TOTAL.with_label_values(&["account"]).get(),
        },
        "relay": {
            "events_received": relay_events_total,
            "connection_status": metrics::RELAY_CONNECTION_STATUS.get(),
        },
        "accounts": {
            "total": metrics::ACCOUNTS_TOTAL.get(),
            "active_sessions": metrics::SESSIONS_ACTIVE.get(),
        },
        "background_jobs": {
            "active": metrics::BACKGROUND_JOBS_ACTIVE.get(),
        }
    })))
}

// ============================================================================
// Blob Storage Management Endpoints
// ============================================================================

/// Query parameters for listBlobs endpoint
#[derive(Deserialize)]
struct ListBlobsQuery {
    did: Option<String>,
    #[serde(default = "default_blob_limit")]
    limit: i64,
    cursor: Option<String>,
}

fn default_blob_limit() -> i64 {
    100
}

/// Get blob storage statistics
async fn get_blob_statistics(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Get total blob count and size
    let stats = sqlx::query_as::<_, (i64, i64)>(
        "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM blob_metadata",
    )
    .fetch_one(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let (total_count, total_size) = stats;

    // Get orphaned temp blobs count
    let orphaned_temp = ctx
        .blob_store
        .list_orphaned_temp_blobs(24)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let orphaned_count = orphaned_temp.len() as i64;

    // Get blob count by MIME type
    let mime_stats = sqlx::query_as::<_, (String, i64)>(
        "SELECT mime_type, COUNT(*) as count FROM blob_metadata GROUP BY mime_type ORDER BY count DESC LIMIT 10"
    )
    .fetch_all(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mime_distribution: Vec<serde_json::Value> = mime_stats
        .iter()
        .map(|(mime_type, count)| {
            serde_json::json!({
                "mime_type": mime_type,
                "count": count
            })
        })
        .collect();

    // Get top users by blob count
    let top_users = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT creator_did, COUNT(*) as count, SUM(size) as total_size FROM blob_metadata GROUP BY creator_did ORDER BY count DESC LIMIT 10"
    )
    .fetch_all(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let user_stats: Vec<serde_json::Value> = top_users
        .iter()
        .map(|(did, count, size)| {
            serde_json::json!({
                "did": did,
                "blob_count": count,
                "total_size": size
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "total_blobs": total_count,
        "total_size_bytes": total_size,
        "total_size_mb": total_size as f64 / 1024.0 / 1024.0,
        "orphaned_temp_blobs": orphaned_count,
        "mime_type_distribution": mime_distribution,
        "top_users_by_blob_count": user_stats,
    })))
}

/// List blobs with optional filtering
async fn list_blobs(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<ListBlobsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let limit = params.limit.min(500); // Cap at 500

    let blobs = if let Some(did) = params.did {
        // List blobs for specific DID
        ctx.blob_store
            .list_for_user(&did, limit)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        // List all blobs with cursor pagination
        let query = if let Some(cursor) = params.cursor {
            sqlx::query(
                r#"
                SELECT cid, mime_type, size, creator_did, created_at, width, height, alt_text, thumbnail_cid, temp_key
                FROM blob_metadata
                WHERE cid > ?1
                ORDER BY cid ASC
                LIMIT ?2
                "#
            )
            .bind(cursor)
            .bind(limit)
        } else {
            sqlx::query(
                r#"
                SELECT cid, mime_type, size, creator_did, created_at, width, height, alt_text, thumbnail_cid, temp_key
                FROM blob_metadata
                ORDER BY cid ASC
                LIMIT ?1
                "#
            )
            .bind(limit)
        };

        let rows = query
            .fetch_all(&ctx.account_db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let mut blobs = Vec::new();
        for row in rows {
            use sqlx::Row;
            blobs.push(crate::blob_store::BlobMetadata {
                cid: row
                    .try_get("cid")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                mime_type: row
                    .try_get("mime_type")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                size: row
                    .try_get("size")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                creator_did: row
                    .try_get("creator_did")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                created_at: chrono::DateTime::parse_from_rfc3339(
                        &row.try_get::<String, _>("created_at")
                            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                    )
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Invalid timestamp: {}", e)))?,
                width: row
                    .try_get("width")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                height: row
                    .try_get("height")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                alt_text: row
                    .try_get("alt_text")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                thumbnail_cid: row
                    .try_get("thumbnail_cid")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
                // Arc 16b §9.2.3.1: lifecycle discriminator.
                // Admin listing reads the column; helper ships with
                // zero production callers in Arc 16b per §9.2.5.1.
                temp_key: row
                    .try_get("temp_key")
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            });
        }
        blobs
    };

    let next_cursor = blobs.last().map(|b| b.cid.clone());

    Ok(Json(serde_json::json!({
        "blobs": blobs,
        "cursor": next_cursor,
    })))
}

/// Request body for deleteBlob endpoint
#[derive(Deserialize)]
struct DeleteBlobRequest {
    cid: String,
}

/// Delete a specific blob
async fn delete_blob(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Json(req): Json<DeleteBlobRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Check if blob exists
    let metadata = ctx
        .blob_store
        .get_metadata(&req.cid)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if metadata.is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Blob not found: {}", req.cid),
        ));
    }

    // Delete blob
    ctx.blob_store
        .delete(&req.cid)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "cid": req.cid,
        "message": "Blob deleted successfully"
    })))
}

/// Request body for quarantineBlob endpoint
#[derive(Deserialize)]
struct QuarantineBlobRequest {
    cid: String,
    reason: String,
    details: Option<String>,
    legal_reference: Option<String>,
}

/// Quarantine a blob (mark as taken down)
async fn quarantine_blob(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<QuarantineBlobRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::blob_store::quarantine::{BlobQuarantine, QuarantineReason};
    use std::str::FromStr;

    // Parse quarantine reason
    let reason = QuarantineReason::from_str(&req.reason)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Create quarantine manager
    let quarantine = BlobQuarantine::new(ctx.account_db.clone());

    // Quarantine the blob
    let record = quarantine
        .quarantine_blob(
            &req.cid,
            reason,
            req.details.as_deref(),
            &auth.did,
            req.legal_reference.as_deref(),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "cid": record.cid,
        "reason": record.reason,
        "quarantined_by": record.quarantined_by,
        "quarantined_at": record.quarantined_at,
    })))
}

/// Request body for restoreBlob endpoint
#[derive(Deserialize)]
struct RestoreBlobRequest {
    cid: String,
}

/// Restore a quarantined blob
async fn restore_blob(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RestoreBlobRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::blob_store::quarantine::BlobQuarantine;

    // Create quarantine manager
    let quarantine = BlobQuarantine::new(ctx.account_db.clone());

    // Restore the blob
    quarantine
        .restore_blob(&req.cid, &auth.did)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "cid": req.cid,
        "restored_by": auth.did,
        "message": "Blob restored successfully"
    })))
}

/// Request body for runBlobGC endpoint
#[derive(Deserialize)]
struct RunBlobGCRequest {
    #[serde(default = "default_gc_ttl")]
    orphaned_ttl_hours: i64,
    dry_run: Option<bool>,
}

fn default_gc_ttl() -> i64 {
    24
}

/// Run blob garbage collection
async fn run_blob_gc(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Json(req): Json<RunBlobGCRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let dry_run = req.dry_run.unwrap_or(false);

    // List orphaned temp blobs
    let orphaned = ctx
        .blob_store
        .list_orphaned_temp_blobs(req.orphaned_ttl_hours)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut deleted_count = 0;
    let mut errors = Vec::new();

    if !dry_run {
        // Delete each orphaned blob
        for cid in &orphaned {
            match ctx.blob_store.delete_temp_blob(cid).await {
                Ok(_) => {
                    deleted_count += 1;
                    tracing::info!("Deleted orphaned temp blob: {}", cid);
                }
                Err(e) => {
                    errors.push(format!("Failed to delete {}: {}", cid, e));
                    tracing::warn!("Failed to delete orphaned temp blob {}: {}", cid, e);
                }
            }
        }
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "dry_run": dry_run,
        "orphaned_found": orphaned.len(),
        "deleted": deleted_count,
        "errors": errors,
    })))
}

/// Get blob quotas per account
async fn get_blob_quotas(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Get storage usage per user
    let usage = sqlx::query_as::<_, (String, i64, i64)>(
        r#"
        SELECT creator_did, COUNT(*) as blob_count, SUM(size) as total_size
        FROM blob_metadata
        GROUP BY creator_did
        ORDER BY total_size DESC
        LIMIT 100
        "#,
    )
    .fetch_all(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let quotas: Vec<serde_json::Value> = usage
        .iter()
        .map(|(did, count, size)| {
            serde_json::json!({
                "did": did,
                "blob_count": count,
                "total_size_bytes": size,
                "total_size_mb": *size as f64 / 1024.0 / 1024.0,
                // For now, no hard quotas enforced, just reporting usage
                "quota_bytes": null,
                "quota_mb": null,
                "usage_percent": null,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "quotas": quotas,
        "total_users": quotas.len(),
    })))
}

// ============================================================================
// Sequencer Management Endpoints
// ============================================================================

/// Get sequencer status and statistics
async fn get_sequencer_status(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Get current sequence number
    let current_seq = ctx
        .sequencer
        .current_seq()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .unwrap_or(0);

    // Get total event count
    let total_events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM repo_seq WHERE invalidated = 0")
            .fetch_one(&ctx.account_db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Get event counts by type
    let event_counts = sqlx::query_as::<_, (String, i64)>(
        "SELECT event_type, COUNT(*) as count FROM repo_seq WHERE invalidated = 0 GROUP BY event_type"
    )
    .fetch_all(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut events_by_type = serde_json::Map::new();
    for (event_type, count) in event_counts {
        events_by_type.insert(event_type, serde_json::json!(count));
    }

    // Get first and last event timestamps
    let first_event: Option<String> = sqlx::query_scalar(
        "SELECT sequenced_at FROM repo_seq WHERE invalidated = 0 ORDER BY seq ASC LIMIT 1",
    )
    .fetch_optional(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let last_event: Option<String> = sqlx::query_scalar(
        "SELECT sequenced_at FROM repo_seq WHERE invalidated = 0 ORDER BY seq DESC LIMIT 1",
    )
    .fetch_optional(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Check if sequencer is paused (using a config table)
    let is_paused: bool = sqlx::query_scalar(
        "SELECT COALESCE((SELECT value FROM sequencer_config WHERE key = 'paused'), '0') = '1'",
    )
    .fetch_one(&ctx.account_db)
    .await
    .unwrap_or(false);

    Ok(Json(serde_json::json!({
        "status": if is_paused { "paused" } else { "running" },
        "current_seq": current_seq,
        "total_events": total_events,
        "events_by_type": events_by_type,
        "first_event_at": first_event,
        "last_event_at": last_event,
        "paused": is_paused,
    })))
}

/// Query parameters for listRecentEvents endpoint
#[derive(Deserialize)]
struct ListRecentEventsQuery {
    #[serde(default = "default_recent_events_limit")]
    limit: i64,
    cursor: Option<i64>,
    event_type: Option<String>,
}

fn default_recent_events_limit() -> i64 {
    50
}

/// List recent events from the sequencer
async fn list_recent_events(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<ListRecentEventsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let limit = params.limit.min(500); // Cap at 500

    // Build query based on filters
    let mut query = String::from(
        "SELECT seq, did, event_type, sequenced_at FROM repo_seq WHERE invalidated = 0",
    );

    // Add cursor filter
    if let Some(cursor) = params.cursor {
        query.push_str(&format!(" AND seq < {}", cursor));
    }

    // Add event type filter
    if let Some(ref event_type) = params.event_type {
        query.push_str(&format!(" AND event_type = '{}'", event_type));
    }

    query.push_str(&format!(" ORDER BY seq DESC LIMIT {}", limit));

    let rows = sqlx::query(&query)
        .fetch_all(&ctx.account_db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut events = Vec::new();
    for row in rows {
        use sqlx::Row;
        events.push(serde_json::json!({
            "seq": row.try_get::<i64, _>("seq").map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            "did": row.try_get::<String, _>("did").map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            "event_type": row.try_get::<String, _>("event_type").map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
            "sequenced_at": row.try_get::<String, _>("sequenced_at").map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
        }));
    }

    let next_cursor = events
        .last()
        .and_then(|e| e.get("seq"))
        .and_then(|s| s.as_i64());

    Ok(Json(serde_json::json!({
        "events": events,
        "cursor": next_cursor,
    })))
}

/// Pause sequencer event streaming
async fn pause_sequencer(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Set paused flag in database
    sqlx::query("INSERT OR REPLACE INTO sequencer_config (key, value) VALUES ('paused', '1')")
        .execute(&ctx.account_db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    audit_chain::insert_chain_entry_pool(
        &ctx.account_db,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "sequencer.pause",
            subject: None,
            rationale: "operator-initiated sequencer pause",
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "status": "paused",
        "message": "Sequencer event streaming paused"
    })))
}

/// Resume sequencer event streaming
async fn resume_sequencer(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Set paused flag to false in database
    sqlx::query("INSERT OR REPLACE INTO sequencer_config (key, value) VALUES ('paused', '0')")
        .execute(&ctx.account_db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    audit_chain::insert_chain_entry_pool(
        &ctx.account_db,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "sequencer.resume",
            subject: None,
            rationale: "operator-initiated sequencer resume",
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "status": "running",
        "message": "Sequencer event streaming resumed"
    })))
}

/// Request body for resetSequencerCursor endpoint
#[derive(Deserialize)]
struct ResetSequencerCursorRequest {
    #[serde(default)]
    target_seq: Option<i64>,
}

/// Reset sequencer cursor position
async fn reset_sequencer_cursor(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<ResetSequencerCursorRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let target = req.target_seq.unwrap_or(0);

    // Validate target sequence exists if specified
    if target > 0 {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM repo_seq WHERE seq = ?1)")
                .bind(target)
                .fetch_one(&ctx.account_db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        if !exists {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Sequence {} not found", target),
            ));
        }
    }

    // Store cursor position
    sqlx::query(
        "INSERT OR REPLACE INTO sequencer_config (key, value) VALUES ('cursor_position', ?1)",
    )
    .bind(target.to_string())
    .execute(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let rationale = format!("reset sequencer cursor to {}", target);
    audit_chain::insert_chain_entry_pool(
        &ctx.account_db,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "sequencer.reset_cursor",
            subject: None,
            rationale: &rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "cursor_position": target,
        "message": format!("Sequencer cursor reset to {}", target)
    })))
}

/// Request body for rebuildSequencer endpoint
#[derive(Deserialize)]
struct RebuildSequencerRequest {
    #[serde(default)]
    verify_only: bool,
}

/// Rebuild or verify sequencer integrity
async fn rebuild_sequencer(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RebuildSequencerRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Verify sequence integrity
    let gaps = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT seq, seq - LAG(seq, 1, 0) OVER (ORDER BY seq) as gap
        FROM repo_seq
        WHERE invalidated = 0
        HAVING gap > 1
        LIMIT 10
        "#,
    )
    .fetch_all(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let has_gaps = !gaps.is_empty();

    // Check for duplicate sequences
    let duplicates: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT seq FROM repo_seq
        WHERE invalidated = 0
        GROUP BY seq
        HAVING COUNT(*) > 1
        LIMIT 10
        "#,
    )
    .fetch_all(&ctx.account_db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let has_duplicates = !duplicates.is_empty();

    let integrity_ok = !has_gaps && !has_duplicates;

    if req.verify_only {
        let rationale = format!(
            "sequencer integrity check {}",
            if integrity_ok { "passed" } else { "failed" }
        );
        audit_chain::insert_chain_entry_pool(
            &ctx.account_db,
            ctx.config.database.backend,
            AppendEntryParams {
                source: "manual",
                payload: None,
                actor_did: &auth.did,
                action: "sequencer.verify",
                subject: None,
                rationale: &rationale,
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        Ok(Json(serde_json::json!({
            "success": true,
            "verify_only": true,
            "integrity_ok": integrity_ok,
            "has_gaps": has_gaps,
            "has_duplicates": has_duplicates,
            "gaps": gaps.iter().map(|(seq, gap)| serde_json::json!({
                "seq": seq,
                "gap_size": gap
            })).collect::<Vec<_>>(),
            "duplicate_sequences": duplicates,
        })))
    } else {
        // For now, rebuild is just verification
        // In a full implementation, this would:
        // 1. Backup current sequence table
        // 2. Rebuild sequence numbers from scratch
        // 3. Update all references
        // This is a destructive operation and should be done carefully

        audit_chain::insert_chain_entry_pool(
            &ctx.account_db,
            ctx.config.database.backend,
            AppendEntryParams {
                source: "manual",
                payload: None,
                actor_did: &auth.did,
                action: "sequencer.rebuild",
                subject: None,
                rationale: "sequencer rebuild requested (verify-only path)",
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        Ok(Json(serde_json::json!({
            "success": true,
            "verify_only": false,
            "integrity_ok": integrity_ok,
            "message": "Sequencer verification complete. Full rebuild not yet implemented.",
            "has_gaps": has_gaps,
            "has_duplicates": has_duplicates,
        })))
    }
}

// ============================================================================
// Rate Limiting Management Endpoints
// ============================================================================

/// Response for getRateLimitConfig endpoint
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitConfigResponse {
    /// Requests per second for authenticated users
    authenticated_rps: u32,
    /// Requests per second for unauthenticated users
    unauthenticated_rps: u32,
    /// Requests per second for admin users
    admin_rps: u32,
    /// Requests per second for cross-PDS authenticated users
    cross_pds_rps: u32,
    /// Burst size for rate limiting
    burst_size: u32,
    /// Whether proxy headers are trusted for IP extraction
    trust_proxy: bool,
    /// Requests per second for handle resolution
    handle_resolution_rps: u32,
    /// Requests per second for DID resolution
    did_resolution_rps: u32,
    /// Endpoints with custom rate limits
    custom_endpoints: Vec<String>,
}

/// Get current rate limit configuration
///
/// Returns the current rate limiting settings including global limits,
/// per-type limits, and endpoints with custom rate limits.
async fn get_rate_limit_config(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<RateLimitConfigResponse>, (StatusCode, String)> {
    let config = ctx.rate_limiter.get_config();
    let custom_endpoints = ctx.rate_limiter.get_rate_limited_endpoints();

    Ok(Json(RateLimitConfigResponse {
        authenticated_rps: config.authenticated_rps,
        unauthenticated_rps: config.unauthenticated_rps,
        admin_rps: config.admin_rps,
        cross_pds_rps: config.cross_pds_rps,
        burst_size: config.burst_size,
        trust_proxy: config.trust_proxy,
        handle_resolution_rps: config.handle_resolution_rps,
        did_resolution_rps: config.did_resolution_rps,
        custom_endpoints,
    }))
}

/// Rate limit statistics per category
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitCategoryStats {
    category: String,
    recent_requests: u32,
}

/// Response for getRateLimitStatus endpoint
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitStatusResponse {
    /// Total tracked request identifiers
    tracked_identifiers: usize,
    /// Recent request counts by category
    recent_activity: Vec<RateLimitCategoryStats>,
    /// Endpoints with custom rate limits
    rate_limited_endpoints: Vec<String>,
    /// Server uptime information
    status: String,
}

/// Get current rate limiting status
///
/// Returns real-time statistics about rate limiting including
/// current request counts and tracked identifiers.
async fn get_rate_limit_status(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<RateLimitStatusResponse>, (StatusCode, String)> {
    let tracked_identifiers = ctx.rate_limiter.get_tracked_identifiers_count();
    let request_counts = ctx.rate_limiter.get_request_counts();
    let rate_limited_endpoints = ctx.rate_limiter.get_rate_limited_endpoints();

    // Aggregate request counts by category
    let mut category_counts: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    for (key, count) in request_counts {
        // Extract category from key (e.g., "global:authenticated" -> "authenticated")
        let category = if key.contains(':') {
            key.split(':').next_back().unwrap_or(&key).to_string()
        } else {
            key
        };
        *category_counts.entry(category).or_insert(0) += count;
    }

    let recent_activity: Vec<RateLimitCategoryStats> = category_counts
        .into_iter()
        .map(|(category, recent_requests)| RateLimitCategoryStats {
            category,
            recent_requests,
        })
        .collect();

    Ok(Json(RateLimitStatusResponse {
        tracked_identifiers,
        recent_activity,
        rate_limited_endpoints,
        status: "operational".to_string(),
    }))
}

/// Request body for cleanupRateLimitState endpoint
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CleanupRateLimitRequest {
    /// Force cleanup even if not necessary
    #[serde(default)]
    force: bool,
}

/// Cleanup old rate limit tracking state
///
/// Clears expired rate limit tracking entries to free memory.
/// This is normally done automatically but can be triggered manually.
async fn cleanup_rate_limit_state(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<CleanupRateLimitRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let before_count = ctx.rate_limiter.get_tracked_identifiers_count();

    ctx.rate_limiter.cleanup_old_counts();

    let after_count = ctx.rate_limiter.get_tracked_identifiers_count();
    let cleaned_count = before_count.saturating_sub(after_count);

    let rationale = format!(
        "cleaned {} rate-limit entries{}",
        cleaned_count,
        if req.force { " (forced)" } else { "" }
    );
    audit_chain::insert_chain_entry_pool(
        &ctx.account_db,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "rate_limit.cleanup",
            subject: None,
            rationale: &rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tracing::info!(
        "Admin {} triggered rate limit cleanup: {} entries removed",
        auth.did,
        cleaned_count
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "before_count": before_count,
        "after_count": after_count,
        "cleaned_count": cleaned_count,
        "forced": req.force,
    })))
}

// ============================================================================
// Federation and Relay Management Endpoints
// ============================================================================

/// Response for getFederationStatus endpoint
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FederationStatusResponse {
    /// Whether federation is enabled
    enabled: bool,
    /// Service DID for this PDS
    service_did: String,
    /// Number of configured relay servers
    relay_count: usize,
    /// Whether relay client is connected
    relay_connected: bool,
    /// Whether PDS discovery is enabled
    discovery_enabled: bool,
    /// Whether federated search is enabled
    search_enabled: bool,
    /// Number of known PDS instances
    known_instances: usize,
    /// Status message
    status: String,
}

/// Get federation status
///
/// Returns the current federation configuration and connection status.
async fn get_federation_status(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<FederationStatusResponse>, (StatusCode, String)> {
    let relay_connected = ctx.relay_client.is_some();
    let discovery_enabled = ctx.pds_discovery.is_some();
    let search_enabled = ctx.federated_search.is_some();

    // Get count of known instances if discovery is enabled
    let known_instances = if let Some(ref discovery) = ctx.pds_discovery {
        discovery.get_known_instances().await.len()
    } else {
        0
    };

    // Get config info
    let federation_config = &ctx.config.federation;

    let status = if !federation_config.enabled {
        "disabled".to_string()
    } else if relay_connected {
        "connected".to_string()
    } else {
        "enabled_disconnected".to_string()
    };

    Ok(Json(FederationStatusResponse {
        enabled: federation_config.enabled,
        service_did: ctx.config.service.service_did.clone(),
        relay_count: federation_config.relay_urls.len(),
        relay_connected,
        discovery_enabled,
        search_enabled,
        known_instances,
        status,
    }))
}

/// The full deployment federation config (#344) — the env-view the
/// Configuration → Federation policy page renders. Unlike the public describe
/// endpoints, this is the SuperAdmin's *complete* view: it includes the
/// trusted-issuer peer allowlist and the internal auto-stream toggle, which the
/// peer-facing describes omit. Read-only; all fields come straight from
/// `FederationConfig` (env/startup config), so the page is honest that mutation
/// is a restart-time deployment change, not a runtime setting.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FederationPolicyView {
    enabled: bool,
    relay_urls: Vec<String>,
    appview_url: Option<String>,
    firehose_enabled: bool,
    crawl_enabled: bool,
    public_url: Option<String>,
    auto_stream_events: bool,
    peer_pds: Vec<PeerPdsConfigView>,
    /// v0.9 Federation Pattern-1 Phase D (#354 / addendum §A2 M2-4) — the
    /// boot-seed-failure state, so the operator can diagnose the refusal without
    /// grepping the audit log.
    boot_seed_status: BootSeedStatus,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PeerPdsConfigView {
    did: String,
    url: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BootSeedStatus {
    boot_seed_failed: bool,
    failed_keys: Vec<String>,
    seeded_keys: Vec<String>,
    failure_reasons: std::collections::HashMap<String, String>,
}

/// `tools.aurora.ops.getFederationPolicy` (#344) — SuperAdmin read of the full
/// deployment federation config for the Federation policy page. SuperAdmin-only
/// because it surfaces the trusted-issuer peer allowlist (who this PDS trusts)
/// and the auto-stream toggle — the same security-adjacent fields the public
/// `describeServer` / `describePosture` endpoints intentionally exclude.
/// Read-only; no mutation.
async fn get_federation_policy(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<FederationPolicyView>, (StatusCode, String)> {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::SuperAdmin) {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "getFederationPolicy requires SuperAdmin role; have {}",
                auth.role.as_str()
            ),
        ));
    }
    let fc = &ctx.config.federation;
    // v0.9 Federation Pattern-1 (#351 / design §2.2): the describe surface reads
    // the trusted-peer list through the runtime-backed snapshot rather than the
    // static `fc.peer_pds`. Phase A: the runtime key is unset, so the snapshot
    // is the `peer_pds` fallback (identical output to before).
    let peer_snapshot = ctx.trusted_peers.snapshot().await;
    // Phase D (#354 / addendum §A2 H-2): relay_urls reads the runtime store
    // (federation.policy.relay-urls) with a fallback to the static config, so the
    // describe reflects runtime relay switches — paralleling peer_pds.
    let relay_urls = {
        let v = crate::api::aurora_admin::resolve_runtime_setting(
            &ctx,
            crate::api::aurora_admin::FEDERATION_POLICY_RELAY_URLS_KEY,
        )
        .await;
        match serde_json::from_value::<Vec<String>>(v) {
            Ok(urls) if !urls.is_empty() => urls,
            _ => fc.relay_urls.clone(),
        }
    };
    // Phase D (#354 / addendum M2-4): surface the boot-seed-failure state.
    let boot_seed_status = {
        use std::sync::atomic::Ordering;
        let failed = ctx.boot_seed_failed.load(Ordering::Acquire);
        let details = ctx.boot_seed_failure_details.read().await.clone();
        match details {
            Some(d) => BootSeedStatus {
                boot_seed_failed: failed,
                failed_keys: d.failed_keys,
                seeded_keys: d.seeded_keys,
                failure_reasons: d.failure_reasons,
            },
            None => BootSeedStatus {
                boot_seed_failed: failed,
                failed_keys: vec![],
                seeded_keys: vec![],
                failure_reasons: std::collections::HashMap::new(),
            },
        }
    };
    Ok(Json(FederationPolicyView {
        enabled: fc.enabled,
        relay_urls,
        appview_url: fc.appview_url.clone(),
        firehose_enabled: fc.firehose_enabled,
        crawl_enabled: fc.crawl_enabled,
        public_url: fc.public_url.clone(),
        auto_stream_events: fc.auto_stream_events,
        peer_pds: peer_snapshot
            .peers
            .iter()
            .map(|p| PeerPdsConfigView {
                did: p.did.clone(),
                url: p.url.clone(),
            })
            .collect(),
        boot_seed_status,
    }))
}

// v0.9 Federation Pattern-1 Phase B (#352) — peer-allowlist CRUD handlers.
// SuperAdmin-gated; core logic + audit/CAS/recovery handling lives in
// `crate::api::federation_peers`. Mutations round-trip through the
// `getFederationPolicy` describe above (which reads `trusted_peers.snapshot()`).

#[derive(Deserialize)]
struct AddFederationPeerRequest {
    did: String,
    url: String,
}

async fn add_federation_peer(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<AddFederationPeerRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    crate::api::federation_peers::guard_boot_seed(&ctx).map_err(|e| e.into_http())?;
    require_superadmin(&auth)?;
    crate::api::federation_peers::add_federation_peer(&ctx, &auth.did, &req.did, &req.url)
        .await
        .map_err(|e| e.into_http())?;
    Ok(Json(serde_json::json!({ "success": true, "did": req.did })))
}

#[derive(Deserialize)]
struct RemoveFederationPeerRequest {
    did: String,
}

async fn remove_federation_peer(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RemoveFederationPeerRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    crate::api::federation_peers::guard_boot_seed(&ctx).map_err(|e| e.into_http())?;
    require_superadmin(&auth)?;
    crate::api::federation_peers::remove_federation_peer(&ctx, &auth.did, &req.did)
        .await
        .map_err(|e| e.into_http())?;
    Ok(Json(serde_json::json!({ "success": true, "did": req.did })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModifyFederationPeerRequest {
    did: String,
    new_url: String,
}

async fn modify_federation_peer(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<ModifyFederationPeerRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    crate::api::federation_peers::guard_boot_seed(&ctx).map_err(|e| e.into_http())?;
    require_superadmin(&auth)?;
    crate::api::federation_peers::modify_federation_peer(&ctx, &auth.did, &req.did, &req.new_url)
        .await
        .map_err(|e| e.into_http())?;
    Ok(Json(serde_json::json!({ "success": true, "did": req.did })))
}

// v0.9 Federation Pattern-1 Phase C (#353) — discovery-mode + pending-discovery
// dismissal. SuperAdmin-gated; core logic in `crate::api::federation_discovery`.

#[derive(Deserialize)]
struct SetDiscoveryModeRequest {
    mode: String,
}

async fn set_discovery_mode(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<SetDiscoveryModeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    crate::api::federation_peers::guard_boot_seed(&ctx).map_err(|e| e.into_http())?;
    require_superadmin(&auth)?;
    crate::api::federation_discovery::set_discovery_mode(&ctx, &auth.did, &req.mode)
        .await
        .map_err(|e| e.into_http())?;
    Ok(Json(serde_json::json!({ "success": true, "mode": req.mode })))
}

#[derive(Deserialize)]
struct DismissPendingDiscoveryRequest {
    did: String,
}

async fn dismiss_pending_discovery(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<DismissPendingDiscoveryRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    crate::api::federation_peers::guard_boot_seed(&ctx).map_err(|e| e.into_http())?;
    require_superadmin(&auth)?;
    crate::api::federation_discovery::dismiss_pending_discovery(&ctx, &auth.did, &req.did)
        .await
        .map_err(|e| e.into_http())?;
    Ok(Json(serde_json::json!({ "success": true, "did": req.did })))
}

// v0.9 Federation Pattern-1 Phase D (#354) — relay runtime-switch. SuperAdmin-
// gated; boot-seed-failure-flag-gated; core logic + CAS/reconfigure/audit in
// `crate::api::federation_relays`.

#[derive(Deserialize)]
struct AddRelayUrlRequest {
    url: String,
}

async fn add_relay_url(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<AddRelayUrlRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    crate::api::federation_peers::guard_boot_seed(&ctx).map_err(|e| e.into_http())?;
    require_superadmin(&auth)?;
    crate::api::federation_relays::add_relay_url(&ctx, &auth.did, &req.url)
        .await
        .map_err(|e| e.into_http())?;
    Ok(Json(serde_json::json!({ "success": true, "url": req.url })))
}

#[derive(Deserialize)]
struct RemoveRelayUrlRequest {
    url: String,
}

async fn remove_relay_url(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<RemoveRelayUrlRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    crate::api::federation_peers::guard_boot_seed(&ctx).map_err(|e| e.into_http())?;
    require_superadmin(&auth)?;
    crate::api::federation_relays::remove_relay_url(&ctx, &auth.did, &req.url)
        .await
        .map_err(|e| e.into_http())?;
    Ok(Json(serde_json::json!({ "success": true, "url": req.url })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetFederationRelaysRequest {
    relay_urls: Vec<String>,
    #[serde(default = "default_transition_mode")]
    transition_mode: String,
}

fn default_transition_mode() -> String {
    "graceful".to_string()
}

async fn set_federation_relays(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
    Json(req): Json<SetFederationRelaysRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    crate::api::federation_peers::guard_boot_seed(&ctx).map_err(|e| e.into_http())?;
    require_superadmin(&auth)?;
    crate::api::federation_relays::set_federation_relays(
        &ctx,
        &auth.did,
        req.relay_urls.clone(),
        &req.transition_mode,
    )
    .await
    .map_err(|e| e.into_http())?;
    Ok(Json(serde_json::json!({ "success": true, "relayUrls": req.relay_urls })))
}

/// Relay server info
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RelayServerInfo {
    url: String,
    status: String,
}

/// Response for getRelayConfig endpoint
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RelayConfigResponse {
    /// Configured relay servers
    servers: Vec<RelayServerInfo>,
    /// Reconnect interval in seconds
    reconnect_interval: u64,
    /// Buffer size for events
    buffer_size: usize,
    /// Whether compression is enabled
    compression_enabled: bool,
    /// Overall relay status
    status: String,
}

/// Get relay configuration
///
/// Returns the current relay client configuration and server list.
async fn get_relay_config(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<RelayConfigResponse>, (StatusCode, String)> {
    let federation_config = &ctx.config.federation;
    let has_relay = ctx.relay_client.is_some();

    let servers: Vec<RelayServerInfo> = federation_config
        .relay_urls
        .iter()
        .map(|url: &String| RelayServerInfo {
            url: url.clone(),
            status: if has_relay {
                "configured".to_string()
            } else {
                "disabled".to_string()
            },
        })
        .collect();

    let status = if !federation_config.enabled {
        "disabled".to_string()
    } else if servers.is_empty() {
        "no_servers".to_string()
    } else if has_relay {
        "active".to_string()
    } else {
        "inactive".to_string()
    };

    Ok(Json(RelayConfigResponse {
        servers,
        reconnect_interval: 5, // Default from RelayConfig
        buffer_size: 1000,     // Default from RelayConfig
        compression_enabled: true,
        status,
    }))
}

/// Known PDS instance info
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct KnownInstanceInfo {
    did: String,
    url: String,
    name: Option<String>,
    open_registrations: bool,
    user_count: Option<i64>,
    last_seen: Option<i64>,
}

/// Response for listKnownInstances endpoint
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ListKnownInstancesResponse {
    instances: Vec<KnownInstanceInfo>,
    total: usize,
}

/// List known PDS instances
///
/// Returns all PDS instances discovered through relay servers.
async fn list_known_instances(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<ListKnownInstancesResponse>, (StatusCode, String)> {
    let instances: Vec<KnownInstanceInfo> = if let Some(ref discovery) = ctx.pds_discovery {
        discovery
            .get_known_instances()
            .await
            .into_iter()
            .map(|inst| KnownInstanceInfo {
                did: inst.did,
                url: inst.url,
                name: inst.name,
                open_registrations: inst.open_registrations,
                user_count: inst.user_count,
                last_seen: inst.last_seen,
            })
            .collect()
    } else {
        vec![]
    };

    let total = instances.len();
    Ok(Json(ListKnownInstancesResponse { instances, total }))
}

/// Trigger PDS discovery
///
/// Initiates discovery of PDS instances from configured relay servers.
async fn trigger_pds_discovery(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if let Some(ref discovery) = ctx.pds_discovery {
        match discovery.discover_from_relays().await {
            Ok(instances) => {
                let rationale = format!("discovered {} PDS instances", instances.len());
                audit_chain::insert_chain_entry_pool(
                    &ctx.account_db,
                    ctx.config.database.backend,
                    AppendEntryParams {
                        source: "manual",
                        payload: None,
                        actor_did: &auth.did,
                        action: "federation.discover",
                        subject: None,
                        rationale: &rationale,
                        snapshot_id: None,
                        event_id: None,
                        cascade_subjects: &[],
                        cascade_snapshot_ids: &[],
                    },
                )
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

                // v0.9 Federation Pattern-1 Phase C (#353 / Step 7): manual
                // scans bypass the scheduler-level discovery-disabled
                // short-circuit (operator-initiated), but per-peer processing
                // still honors the active mode. No scheduled_discovery_ran audit
                // (manual keeps its own federation.discover above).
                let mode = crate::api::federation_discovery::current_mode(&ctx).await;
                crate::api::federation_discovery::process_scan(&ctx, &instances, mode, false).await;

                tracing::info!(
                    "Admin {} triggered PDS discovery: {} instances found",
                    auth.did,
                    instances.len()
                );

                Ok(Json(serde_json::json!({
                    "success": true,
                    "discovered_count": instances.len(),
                    "message": format!("Discovered {} PDS instances", instances.len()),
                })))
            }
            Err(e) => {
                tracing::warn!("PDS discovery failed: {}", e);
                Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Discovery failed: {}", e),
                ))
            }
        }
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            "Federation discovery is not enabled".to_string(),
        ))
    }
}

/// Get nonce store status (service auth nonces)
///
/// Returns statistics about the service authentication nonce store.
async fn get_nonce_store_status(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let service_auth_enabled = ctx.nonce_store.is_some();
    let dpop_enabled = ctx.dpop_nonce_store.is_some();

    // Get nonce counts if available
    let service_auth_count = if let Some(ref store) = ctx.nonce_store {
        store.count().await
    } else {
        0
    };

    let dpop_count = if let Some(ref store) = ctx.dpop_nonce_store {
        store.count().await
    } else {
        0
    };

    Ok(Json(serde_json::json!({
        "service_auth": {
            "enabled": service_auth_enabled,
            "active_nonces": service_auth_count,
        },
        "dpop": {
            "enabled": dpop_enabled,
            "active_nonces": dpop_count,
        },
        "status": if service_auth_enabled || dpop_enabled { "active" } else { "disabled" },
    })))
}

/// Cleanup nonce stores
///
/// Triggers cleanup of expired nonces in both service auth and DPoP stores.
async fn cleanup_nonce_stores(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let mut cleaned_service_auth = 0;
    let mut cleaned_dpop = 0;

    // Cleanup service auth nonces
    if let Some(ref store) = ctx.nonce_store {
        if let Ok(removed) = store.cleanup_expired().await {
            cleaned_service_auth = removed;
        }
    }

    // Cleanup DPoP nonces
    if let Some(ref store) = ctx.dpop_nonce_store {
        if let Ok(removed) = store.cleanup_expired().await {
            cleaned_dpop = removed;
        }
    }

    let total_cleaned = cleaned_service_auth + cleaned_dpop;

    let rationale = format!(
        "cleaned {} nonces ({} service-auth, {} DPoP)",
        total_cleaned, cleaned_service_auth, cleaned_dpop
    );
    audit_chain::insert_chain_entry_pool(
        &ctx.account_db,
        ctx.config.database.backend,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: &auth.did,
            action: "federation.nonce_cleanup",
            subject: None,
            rationale: &rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tracing::info!(
        "Admin {} triggered nonce cleanup: {} service auth, {} DPoP",
        auth.did,
        cleaned_service_auth,
        cleaned_dpop
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "cleaned": {
            "service_auth": cleaned_service_auth,
            "dpop": cleaned_dpop,
            "total": total_cleaned,
        },
    })))
}

// Arc 2 Step 1 (§6.4.1) — canonical-JSON helper for snapshot
// tests. See the matching declaration in `src/admin/defs.rs` for
// the rationale on top-level placement vs nested-mod placement.
// Twin inline include — see `src/admin/defs.rs` for the v0.6 cleanup note.
#[cfg(test)]
#[path = "../../tests/common/canonical_json.rs"]
#[allow(clippy::duplicate_mod)]
mod canonical_json_helper;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        account::ValidatedSession,
        admin::roles::Role,
        config::{
            AuthConfig, BlobstoreConfig, FederationConfig, IdentityConfig, InviteConfig,
            LoggingConfig, OAuthConfig, RateLimitConfig, ServerConfig, ServiceConfig,
            StorageConfig,
        },
    };
    use std::sync::OnceLock;
    use tempfile::tempdir;

    /// Serialises the filesystem-heavy portion of test setup.
    ///
    /// Without this, `cargo test --lib` would race 25+ parallel `tempdir()`
    /// plus `SqlitePool::connect` plus migration runs against each other and
    /// produce sporadic `SQLITE_CANTOPEN` (code 14) errors on first runs,
    /// especially under WSL2's drvfs where this crate's primary checkout
    /// lives. Holding the lock through `AppContext::new` is cheap and the
    /// test bodies still execute in parallel once setup completes.
    /// Tracked under chainlink #68.
    fn fixture_setup_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    async fn create_test_context() -> AppContext {
        create_test_context_with(|_| {}).await
    }

    // Variant that lets a test tweak the config before the context is built —
    // e.g. flip kryphocron off, since it is ON by default as of v0.9 and the
    // disabled-path tests need to opt the fixture back out.
    async fn create_test_context_with(
        mutate: impl FnOnce(&mut ServerConfig),
    ) -> AppContext {
        let _guard = fixture_setup_lock().lock().await;
        // `into_path()` leaks the TempDir so its Drop doesn't unlink the
        // directory while sqlx connections still hold it open. Under the
        // AnyPool default journal mode (DELETE), SQLite reports
        // SQLITE_READONLY_DBMOVED on the next write once the directory
        // entry is gone — WAL was previously masking this. The OS cleans
        // up /tmp on its own; this is test-only so leaking is fine.
        let dir = tempdir().unwrap().keep();
        let db_path = dir.join("test.db");

        let mut config = ServerConfig {
            service: ServiceConfig {
                hostname: "localhost".to_string(),
                port: 2583,
                service_did: "did:web:localhost".to_string(),
                version: "0.1.0-test".to_string(),
                blob_upload_limit: 5242880,
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
                // Config validation requires JWT secrets >= 32 chars.
                jwt_secret: "test-secret-key-for-admin-tests-32-chars".to_string(),
                // Valid 32-byte hex keys so PlcSigner::from_hex succeeds in
                // tests that exercise PLC code paths.
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
                enabled: true,
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
            validation_mode: crate::validation::ValidationMode::Required,
            distributed_state_mode: Default::default(),
            maintenance_pool: Default::default(),
            gc_sweep: Default::default(),
            bind_audit_orphan_marker: Default::default(),
            blob_metadata: Default::default(),
            entryway: None,
            lexicon: crate::config::LexiconConfig::default(),
            kryphocron: crate::config::KryphocronConfig::default(),
        };

        // The admin module hosts `describe_capabilities`, which
        // Step 3 made registry-driven. Tests in this module
        // construct the context with the populated registry
        // returned by `routes()` so the
        // `describe_capabilities_snapshot` test sees the same
        // wire output a real PDS would emit. Other test
        // fixtures (auth.rs, aurora_*.rs, tests/) keep the empty
        // default — they don't exercise the capability probe.
        let (_router, registry) = super::routes();
        mutate(&mut config);
        AppContext::new(config, registry).await.unwrap()
    }

    #[tokio::test]
    async fn get_federation_policy_superadmin_only_full_view() {
        // #344 — SuperAdmin-only; returns the FULL env config including the
        // peer allowlist + auto-stream toggle (the security-adjacent fields the
        // public describes omit).
        use crate::admin::roles::Role;
        let ctx = create_test_context_with(|c| {
            c.federation.enabled = true;
            c.federation.appview_url = Some("https://api.example".to_string());
            c.federation.auto_stream_events = true;
            c.federation.peer_pds = vec![crate::config::PeerPdsConfig {
                did: "did:plc:peer".to_string(),
                url: "https://peer.example".to_string(),
            }];
        })
        .await;
        let mk = |role: Role| AdminAuthContext {
            did: "did:plc:op".to_string(),
            session: ValidatedSession {
                did: "did:plc:op".to_string(),
                session_id: "s".to_string(),
                is_app_password: false,
            },
            role,
        };
        // Non-SuperAdmin → 403.
        let forbidden = get_federation_policy(State(ctx.clone()), mk(Role::Admin)).await;
        assert_eq!(forbidden.unwrap_err().0, StatusCode::FORBIDDEN);
        // SuperAdmin → full view incl peer_pds + auto_stream_events.
        let view = get_federation_policy(State(ctx.clone()), mk(Role::SuperAdmin))
            .await
            .expect("superadmin gets the policy view")
            .0;
        assert!(view.enabled);
        assert_eq!(view.appview_url.as_deref(), Some("https://api.example"));
        assert!(view.auto_stream_events);
        assert_eq!(view.peer_pds.len(), 1);
        assert_eq!(view.peer_pds[0].did, "did:plc:peer");
        assert_eq!(view.peer_pds[0].url, "https://peer.example");
    }

    // §5.5.4 Phase E — composite-load gating + section composition.
    #[tokio::test]
    async fn phase_e_get_defaults_state_gated_and_composes() {
        let ctx = create_test_context().await;
        let mk = |role: Role| AdminAuthContext {
            did: "did:plc:op".to_string(),
            session: ValidatedSession {
                did: "did:plc:op".to_string(),
                session_id: "s".to_string(),
                is_app_password: false,
            },
            role,
        };
        assert_eq!(
            get_defaults_state(State(ctx.clone()), mk(Role::Admin)).await.unwrap_err().0,
            StatusCode::FORBIDDEN
        );
        let v = get_defaults_state(State(ctx.clone()), mk(Role::SuperAdmin)).await.unwrap().0;
        for sec in ["reportAction", "reviewerAssignment", "autoLabelRules", "escalationRules"] {
            assert_eq!(v[sec]["status"], "ok", "section {} ok", sec);
        }
    }

    // §5.5.4 Phase E — SuperAdmin gating sweep across the §5.5.4 XRPCs.
    #[tokio::test]
    async fn phase_e_superadmin_gating_sweep() {
        let ctx = create_test_context().await;
        let admin = || AdminAuthContext {
            did: "did:plc:op".to_string(),
            session: ValidatedSession {
                did: "did:plc:op".to_string(),
                session_id: "s".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };
        let f = StatusCode::FORBIDDEN;
        assert_eq!(get_defaults_state(State(ctx.clone()), admin()).await.unwrap_err().0, f);
        assert_eq!(
            create_auto_label_rule(State(ctx.clone()), admin(), Json(CreateAutoLabelRuleRequest {
                trigger_type: "report-count".into(),
                trigger_params: serde_json::json!({}),
                label_value: "l".into(),
                subject_scope: "account".into(),
                enabled: true,
                rationale: None,
            })).await.unwrap_err().0,
            f
        );
        assert_eq!(
            create_escalation_rule(State(ctx.clone()), admin(), Json(CreateEscalationRuleRequest {
                trigger_type: "category-match".into(),
                trigger_params: serde_json::json!({}),
                action_type: "mark".into(),
                enabled: true,
                rationale: None,
            })).await.unwrap_err().0,
            f
        );
        assert_eq!(
            clear_escalation(State(ctx.clone()), admin(), Json(ClearEscalationRequest {
                item_id: "1".into(),
                rationale: None,
            })).await.unwrap_err().0,
            f
        );
        assert_eq!(
            assign_reviewer(State(ctx.clone()), admin(), Json(AssignReviewerRequest {
                report_id: 1,
                operator_did: "did:plc:x".into(),
                rationale: None,
            })).await.unwrap_err().0,
            f
        );
    }

    #[tokio::test]
    async fn test_get_system_health() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        let result = get_system_health(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        assert!(json["status"].is_string());
        assert_eq!(json["version"], "0.1.0-test");
        assert!(json["uptime_seconds"].is_number());
        assert!(json["services"].is_object());
        assert!(json["services"]["database"].is_string());
    }

    #[tokio::test]
    async fn test_get_database_status() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        let result = get_database_status(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        assert_eq!(json["status"], "healthy");
        assert!(json["pool"]["size"].is_number());
        assert!(json["pool"]["idle_connections"].is_number());
        assert!(json["pool"]["active_connections"].is_number());
        assert!(json["latency_ms"].is_number());
        assert!(json["statistics"]["total_accounts"].is_number());
    }

    #[tokio::test]
    async fn revoke_operator_sessions_gates_super_requires_rationale_and_audits() {
        let ctx = create_test_context().await;
        let mk_auth = |did: &str, role: Role| AdminAuthContext {
            did: did.to_string(),
            session: ValidatedSession {
                did: did.to_string(),
                session_id: "s".to_string(),
                is_app_password: false,
            },
            role,
        };
        let target = "did:plc:target";
        let req = |rationale: Option<&str>| {
            Json(RevokeOperatorSessionsRequest {
                did: target.to_string(),
                rationale: rationale.map(str::to_string),
            })
        };
        // Two active sessions for the target operator.
        for rid in ["r1", "r2"] {
            ctx.operator_session_store
                .create(target, None, None, rid, chrono::Duration::days(30))
                .await
                .unwrap();
        }

        // Non-SuperAdmin → 403.
        let forbidden = revoke_operator_sessions(
            State(ctx.clone()),
            mk_auth("did:plc:adm", Role::Admin),
            req(Some("x")),
        )
        .await;
        assert_eq!(forbidden.unwrap_err().0, StatusCode::FORBIDDEN);

        // SuperAdmin without rationale → 400.
        let no_rationale = revoke_operator_sessions(
            State(ctx.clone()),
            mk_auth("did:plc:super", Role::SuperAdmin),
            req(None),
        )
        .await;
        assert_eq!(no_rationale.unwrap_err().0, StatusCode::BAD_REQUEST);

        // SuperAdmin happy path → both sessions revoked + an audit entry.
        let ok = revoke_operator_sessions(
            State(ctx.clone()),
            mk_auth("did:plc:super", Role::SuperAdmin),
            req(Some("operator departed")),
        )
        .await
        .expect("bulk revoke succeeds")
        .0;
        assert!(ok.success);
        assert_eq!(ok.revoked, 2);
        assert!(!ok.audit_entry_id.is_empty());

        // The target now has no active sessions.
        let (left, _) = ctx
            .operator_session_store
            .list_by_did(target, 50, None)
            .await
            .unwrap();
        assert!(left.is_empty(), "all target sessions revoked");

        // Idempotent: a second bulk revoke succeeds with a zero count.
        let again = revoke_operator_sessions(
            State(ctx.clone()),
            mk_auth("did:plc:super", Role::SuperAdmin),
            req(Some("re-run")),
        )
        .await
        .expect("idempotent success")
        .0;
        assert_eq!(again.revoked, 0);
    }

    #[tokio::test]
    async fn test_get_resource_usage() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        let result = get_resource_usage(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        assert!(json["memory"].is_object());
        assert!(json["memory"]["resident_bytes"].is_number());
        assert!(json["memory"]["resident_mb"].is_number());
        assert!(json["cpu"].is_object());
        assert!(json["cpu"]["seconds_total"].is_number());
    }

    #[tokio::test]
    async fn test_list_background_jobs() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        let result = list_background_jobs(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        assert!(json["active_jobs"].is_number());
        assert!(json["job_statistics"].is_array());
    }

    #[tokio::test]
    async fn test_run_health_checks() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        let result = run_health_checks(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        assert!(json["overall_status"].is_string());
        assert!(json["checks"].is_array());
        assert!(json["total_duration_ms"].is_number());
        assert!(json["timestamp"].is_string());

        // Verify critical components are checked
        let checks = json["checks"].as_array().unwrap();
        let component_names: Vec<&str> = checks
            .iter()
            .filter_map(|c| c["component"].as_str())
            .collect();

        assert!(component_names.contains(&"database"));
        assert!(component_names.contains(&"blob_storage"));
        assert!(component_names.contains(&"sequencer"));
        assert!(component_names.contains(&"identity_resolver"));
    }

    #[tokio::test]
    async fn test_get_version_info() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        let result = get_version_info(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        assert_eq!(json["version"], "0.1.0-test");
        assert_eq!(json["service_did"], "did:web:localhost");
        assert_eq!(json["hostname"], "localhost");
        assert_eq!(json["port"], 2583);
        assert!(json["build_profile"].is_string());
        assert!(json["features"].is_object());
        assert_eq!(json["features"]["federation"], false);
    }

    #[tokio::test]
    async fn test_get_system_metrics() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        let result = get_system_metrics(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        assert!(json["uptime_seconds"].is_number());
        assert!(json["http"].is_object());
        assert!(json["http"]["requests_total"].is_number());
        assert!(json["http"]["active_requests"].is_number());
        assert!(json["database"].is_object());
        assert!(json["database"]["queries_total"].is_number());
        assert!(json["database"]["pool_size"].is_number());
        assert!(json["cache"].is_object());
        assert!(json["cache"]["hits"].is_number());
        assert!(json["cache"]["misses"].is_number());
        assert!(json["cache"]["hit_rate_percent"].is_number());
        assert!(json["sequencer"].is_object());
        assert!(json["accounts"].is_object());
        assert!(json["background_jobs"].is_object());
    }

    #[tokio::test]
    async fn test_database_status_pool_metrics() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        // Make a query to activate a connection
        let _ = sqlx::query("SELECT 1").fetch_one(&ctx.account_db).await;

        let result = get_database_status(State(ctx.clone()), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        let pool_size = json["pool"]["size"].as_u64().unwrap();
        let idle = json["pool"]["idle_connections"].as_u64().unwrap();
        let active = json["pool"]["active_connections"].as_i64().unwrap();

        // Verify pool metrics are consistent
        assert!(pool_size > 0);
        // idle and active are unsigned, so always >= 0
        assert_eq!(pool_size as i64, idle as i64 + active);
    }

    #[tokio::test]
    async fn test_health_checks_response_times() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        let result = run_health_checks(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        let checks = json["checks"].as_array().unwrap();

        // Verify all checks have response times
        for check in checks {
            assert!(check["response_time_ms"].is_number());
            let response_time = check["response_time_ms"].as_u64().unwrap();
            // Response time should be reasonable (< 1 second)
            assert!(
                response_time < 1000,
                "Response time too high: {}",
                response_time
            );
        }
    }

    #[tokio::test]
    async fn test_system_metrics_cache_hit_rate() {
        let ctx = create_test_context().await;
        let auth = AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        };

        // Record some cache events
        crate::metrics::record_cache_access("test", true);
        crate::metrics::record_cache_access("test", true);
        crate::metrics::record_cache_access("test", false);

        let result = get_system_metrics(State(ctx), auth).await;
        assert!(result.is_ok());

        let json = result.unwrap().0;
        let hits = json["cache"]["hits"].as_i64().unwrap();
        let misses = json["cache"]["misses"].as_i64().unwrap();
        let hit_rate = json["cache"]["hit_rate_percent"].as_f64().unwrap();

        assert!(hits >= 2);
        assert!(misses >= 1);
        assert!((0.0..=100.0).contains(&hit_rate));
    }

    fn admin_test_auth() -> AdminAuthContext {
        AdminAuthContext {
            did: "did:plc:test".to_string(),
            session: ValidatedSession {
                did: "did:plc:test".to_string(),
                session_id: "test_session".to_string(),
                is_app_password: false,
            },
            role: Role::Admin,
        }
    }

    /// Moderator auth fixture — for asserting Admin+ gates reject a Moderator
    /// (v0.9 Arc D #225 kryphocron-ops role-gate tests).
    fn moderator_test_auth() -> AdminAuthContext {
        AdminAuthContext {
            did: "did:plc:moderator".to_string(),
            session: ValidatedSession {
                did: "did:plc:moderator".to_string(),
                session_id: "test_session_moderator".to_string(),
                is_app_password: false,
            },
            role: Role::Moderator,
        }
    }

    /// SuperAdmin auth fixture for tests of tools.aurora.superadmin.*
    /// endpoints (Phase 3.6 / chainlink #103). Same shape as
    /// admin_test_auth, role bumped to SuperAdmin so the handler-level
    /// SuperAdmin gate passes.
    fn superadmin_test_auth() -> AdminAuthContext {
        AdminAuthContext {
            did: "did:plc:superadmin".to_string(),
            session: ValidatedSession {
                did: "did:plc:superadmin".to_string(),
                session_id: "test_session_superadmin".to_string(),
                is_app_password: false,
            },
            role: Role::SuperAdmin,
        }
    }

    async fn read_response_body(resp: axum::response::Response) -> (StatusCode, String) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn test_update_account_signing_key_rejects_non_plc_did() {
        let ctx = create_test_context().await;
        let req = UpdateAccountSigningKeyRequest {
            did: "did:web:example.com".to_string(),
            signing_key: "did:key:z6MkpTHR8VNsBxYAAWHut2Geadd9jSwuBV8xRoAnwWsdvktH".to_string(),
        };

        let resp = update_account_signing_key(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("non-did:plc DID should be rejected");
        let (status, body) = read_response_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("did:plc"));
    }

    #[tokio::test]
    async fn test_update_account_signing_key_rejects_non_did_key_signing_key() {
        let ctx = create_test_context().await;
        let req = UpdateAccountSigningKeyRequest {
            did: "did:plc:abcdefghijklmnop".to_string(),
            signing_key: "z6MkpTHR8VNsBxYAAWHut2Geadd9jSwuBV8xRoAnwWsdvktH".to_string(),
        };

        let resp = update_account_signing_key(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("bare multibase signingKey should be rejected");
        let (status, body) = read_response_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("did:key"));
    }

    #[tokio::test]
    async fn test_update_account_signing_key_rejects_mismatched_signing_key() {
        use crate::crypto::plc::PlcSigner;

        let ctx = create_test_context().await;
        // Sanity-check: derive the operator's did:key so we know what would be
        // accepted, then submit something different.
        let operator_signer =
            PlcSigner::from_hex(&ctx.config.authentication.repo_signing_key).unwrap();
        let operator_did_key = operator_signer.public_key_did_key();
        let mismatching_did_key = "did:key:zQ3shokFTS3brHcDQrn82RUDfCZESWL1ZdCEJwekUDPQiYBme";
        assert_ne!(operator_did_key, mismatching_did_key);

        let req = UpdateAccountSigningKeyRequest {
            did: "did:plc:abcdefghijklmnop".to_string(),
            signing_key: mismatching_did_key.to_string(),
        };

        let resp = update_account_signing_key(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("mismatched signingKey should be rejected by strict-mode");
        let (status, body) = read_response_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("body must be JSON");
        assert_eq!(parsed["error"], "InvalidRequest");
        assert!(parsed["message"]
            .as_str()
            .unwrap()
            .contains("operator's configured signing key"));
    }

    #[tokio::test]
    async fn test_update_account_signing_key_accepts_matching_signing_key() {
        use crate::crypto::plc::PlcSigner;

        let ctx = create_test_context().await;
        let operator_signer =
            PlcSigner::from_hex(&ctx.config.authentication.repo_signing_key).unwrap();
        let operator_did_key = operator_signer.public_key_did_key();

        let req = UpdateAccountSigningKeyRequest {
            did: "did:plc:abcdefghijklmnop".to_string(),
            signing_key: operator_did_key.clone(),
        };

        // A matching signingKey passes strict-mode validation; the handler
        // then proceeds to fetch the PLC document, which fails in the test
        // environment because the configured PLC URL is plc.directory and
        // the DID is fictitious. We assert that the failure is *not* the
        // strict-mode 400 InvalidRequest — i.e., strict-mode let us through.
        let resp = update_account_signing_key(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("PLC document fetch will fail in test env");
        let (status, body) = read_response_body(resp).await;
        if status == StatusCode::BAD_REQUEST {
            let parsed: serde_json::Value =
                serde_json::from_str(&body).expect("BAD_REQUEST body must be JSON in this path");
            assert_ne!(
                parsed["error"], "InvalidRequest",
                "strict-mode incorrectly rejected matching signingKey"
            );
        }
        // Otherwise we hit a downstream failure (network, NOT_FOUND from
        // PLC, etc.) — which is expected and confirms strict-mode passed.
    }

    #[tokio::test]
    async fn test_get_account_info_rejects_non_did_input() {
        let ctx = create_test_context().await;
        let query = GetAccountInfoQuery {
            did: "not-a-did".to_string(),
        };

        let result = get_account_info(State(ctx), admin_test_auth(), Query(query)).await;
        let resp = match result {
            Err(r) => r,
            Ok(_) => panic!("non-DID input should be rejected"),
        };
        let (status, body) = read_response_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("DID"));
    }

    #[tokio::test]
    async fn test_get_account_info_returns_repo_not_found_for_missing_account() {
        let ctx = create_test_context().await;
        let query = GetAccountInfoQuery {
            did: "did:plc:nonexistentaccount0000".to_string(),
        };

        let result = get_account_info(State(ctx), admin_test_auth(), Query(query)).await;
        let resp = match result {
            Err(r) => r,
            Ok(_) => panic!("missing account should return RepoNotFound"),
        };
        let (status, body) = read_response_body(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("body must be JSON");
        assert_eq!(parsed["error"], "RepoNotFound");
        assert!(parsed["message"]
            .as_str()
            .unwrap()
            .contains("did:plc:nonexistentaccount0000"));
    }

    #[tokio::test]
    async fn test_disable_invite_codes_empty_input_is_noop() {
        let ctx = create_test_context().await;
        let req = DisableInviteCodesRequest {
            codes: vec![],
            accounts: vec![],
        };

        let result = disable_invite_codes(State(ctx), admin_test_auth(), Json(req)).await;
        assert_eq!(result.unwrap(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_disable_invite_codes_disables_specific_codes_atomically() {
        let ctx = create_test_context().await;

        // Seed two invite codes via the existing manager API.
        let code_a = ctx
            .invite_manager
            .create_invite("did:plc:creator", 5, None, None, None)
            .await
            .unwrap();
        let code_b = ctx
            .invite_manager
            .create_invite("did:plc:creator", 5, None, None, None)
            .await
            .unwrap();
        assert!(!code_a.disabled && !code_b.disabled);

        let req = DisableInviteCodesRequest {
            codes: vec![code_a.code.clone(), code_b.code.clone()],
            accounts: vec![],
        };
        let result = disable_invite_codes(State(ctx.clone()), admin_test_auth(), Json(req)).await;
        assert_eq!(result.unwrap(), StatusCode::OK);

        // Verify both codes are disabled in the database.
        let a_after = ctx
            .invite_manager
            .get_code(&code_a.code)
            .await
            .unwrap()
            .unwrap();
        let b_after = ctx
            .invite_manager
            .get_code(&code_b.code)
            .await
            .unwrap()
            .unwrap();
        assert!(a_after.disabled);
        assert!(b_after.disabled);
    }

    #[tokio::test]
    async fn test_disable_invite_codes_disables_codes_by_account() {
        let ctx = create_test_context().await;
        let target_did = "did:plc:targetaccount";

        // One code issued *for* the target account, one not.
        let issued_for_target = ctx
            .invite_manager
            .create_invite(
                "did:plc:creator",
                5,
                None,
                None,
                Some(target_did.to_string()),
            )
            .await
            .unwrap();
        let unrelated = ctx
            .invite_manager
            .create_invite("did:plc:creator", 5, None, None, None)
            .await
            .unwrap();

        let req = DisableInviteCodesRequest {
            codes: vec![],
            accounts: vec![target_did.to_string()],
        };
        let result = disable_invite_codes(State(ctx.clone()), admin_test_auth(), Json(req)).await;
        assert_eq!(result.unwrap(), StatusCode::OK);

        let target_after = ctx
            .invite_manager
            .get_code(&issued_for_target.code)
            .await
            .unwrap()
            .unwrap();
        let unrelated_after = ctx
            .invite_manager
            .get_code(&unrelated.code)
            .await
            .unwrap()
            .unwrap();
        assert!(target_after.disabled);
        assert!(!unrelated_after.disabled);
    }

    #[tokio::test]
    async fn test_disable_invite_codes_silently_skips_missing_codes() {
        let ctx = create_test_context().await;

        // Submit codes that don't exist; should succeed (the codes are
        // vacuously disabled). Distinct from the singular endpoint, which
        // returns NotFound for unknown codes.
        let req = DisableInviteCodesRequest {
            codes: vec!["aurora-nonexistent-code-1".to_string()],
            accounts: vec!["did:plc:noaccount".to_string()],
        };
        let result = disable_invite_codes(State(ctx), admin_test_auth(), Json(req)).await;
        assert_eq!(result.unwrap(), StatusCode::OK);
    }

    /// Insert minimal actor+account rows directly into the test database.
    /// Bypasses `account_manager.create_account` which requires PLC
    /// registration over the network. Used only for endpoint tests that
    /// need real DB rows to query against.
    async fn seed_test_account(ctx: &AppContext, did: &str, handle: &str, email: Option<&str>) {
        let now = chrono::Utc::now();
        sqlx::query(
            "INSERT INTO actor (did, handle, created_at, takedown_ref, deactivated_at, delete_after)
             VALUES (?1, ?2, ?3, NULL, NULL, NULL)",
        )
        .bind(did)
        .bind(handle)
        .bind(now.to_rfc3339())
        .execute(&ctx.account_db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO account (did, email, password_hash, email_confirmed_at, invites_disabled)
             VALUES (?1, ?2, 'test-hash', NULL, 0)",
        )
        .bind(did)
        .bind(email)
        .execute(&ctx.account_db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_search_accounts_rejects_out_of_range_limit() {
        let ctx = create_test_context().await;
        let result = search_accounts(
            State(ctx),
            admin_test_auth(),
            Query(SearchAccountsQuery {
                q: None,
                email: None,
                cursor: None,
                limit: Some(101),
            }),
        )
        .await;
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("limit > 100 should be rejected"),
        };
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("limit"));

        let ctx2 = create_test_context().await;
        let result2 = search_accounts(
            State(ctx2),
            admin_test_auth(),
            Query(SearchAccountsQuery {
                q: None,
                email: None,
                cursor: None,
                limit: Some(0),
            }),
        )
        .await;
        let err2 = match result2 {
            Err(e) => e,
            Ok(_) => panic!("limit < 1 should be rejected"),
        };
        assert_eq!(err2.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_search_accounts_empty_db_returns_empty_no_cursor() {
        let ctx = create_test_context().await;
        let resp = search_accounts(
            State(ctx),
            admin_test_auth(),
            Query(SearchAccountsQuery {
                q: None,
                email: None,
                cursor: None,
                limit: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(resp.accounts.is_empty());
        assert!(resp.cursor.is_none());
    }

    #[tokio::test]
    async fn test_search_accounts_filters_by_email_case_insensitive() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:aaaa", "alice.test", Some("Alice@Example.com")).await;
        seed_test_account(&ctx, "did:plc:bbbb", "bob.test", Some("bob@example.com")).await;
        seed_test_account(&ctx, "did:plc:cccc", "carol.test", None).await;

        // Case-insensitive match should find Alice regardless of casing.
        let resp = search_accounts(
            State(ctx.clone()),
            admin_test_auth(),
            Query(SearchAccountsQuery {
                q: None,
                email: Some("alice@example.com".to_string()),
                cursor: None,
                limit: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.accounts.len(), 1);
        assert_eq!(resp.accounts[0].did, "did:plc:aaaa");
        assert!(resp.cursor.is_none());

        // Non-matching email returns empty.
        let resp = search_accounts(
            State(ctx),
            admin_test_auth(),
            Query(SearchAccountsQuery {
                q: None,
                email: Some("nobody@example.com".to_string()),
                cursor: None,
                limit: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(resp.accounts.is_empty());
    }

    // #315 — the free-text `q` param must actually filter (it was silently
    // dropped, returning every account). Covers handle / DID / email substring,
    // the non-matching → 0 case (the bug), and no-q → all (preserved).
    #[tokio::test]
    async fn test_search_accounts_filters_by_q_across_handle_did_email() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:aaaa", "alice.test", Some("alice@example.com")).await;
        seed_test_account(&ctx, "did:plc:bbbb", "bob.test", Some("bob@example.com")).await;
        seed_test_account(&ctx, "did:plc:zzzz", "carol.test", None).await;

        let q_search = |term: Option<&str>| {
            let c = ctx.clone();
            let t = term.map(str::to_string);
            async move {
                search_accounts(
                    State(c),
                    admin_test_auth(),
                    Query(SearchAccountsQuery { q: t, email: None, cursor: None, limit: None }),
                )
                .await
                .unwrap()
                .0
            }
        };

        // Handle substring (case-insensitive).
        let r = q_search(Some("ALICE")).await;
        assert_eq!(r.accounts.len(), 1, "q matches a handle substring");
        assert_eq!(r.accounts[0].did, "did:plc:aaaa");

        // DID substring.
        let r = q_search(Some("zzzz")).await;
        assert_eq!(r.accounts.len(), 1, "q matches a DID substring");
        assert_eq!(r.accounts[0].did, "did:plc:zzzz");

        // Email substring.
        let r = q_search(Some("bob@")).await;
        assert_eq!(r.accounts.len(), 1, "q matches an email substring");
        assert_eq!(r.accounts[0].did, "did:plc:bbbb");

        // Non-matching q → 0 accounts (the bug: previously returned all).
        let r = q_search(Some("did:plc:nonexistent-prefix")).await;
        assert!(r.accounts.is_empty(), "non-matching q must return 0 accounts, not the full list");

        // No q → all accounts (existing behavior preserved).
        let r = q_search(None).await;
        assert_eq!(r.accounts.len(), 3, "absent q returns all accounts");
    }

    // ---- tools.aurora.ops.listAccounts (chainlink #84 / Phase 2.3.7) ----

    fn ops_list_query() -> OpsListAccountsQuery {
        OpsListAccountsQuery {
            signup_date_from: None,
            signup_date_to: None,
            invite_source: None,
            status: None,
            cursor: None,
            limit: None,
        }
    }

    #[tokio::test]
    async fn test_ops_list_accounts_empty_db_returns_empty() {
        let ctx = create_test_context().await;
        let resp = ops_list_accounts(State(ctx), admin_test_auth(), Query(ops_list_query()))
            .await
            .unwrap()
            .0;
        assert!(resp.accounts.is_empty());
        assert!(resp.cursor.is_none());
    }

    #[tokio::test]
    async fn test_ops_list_accounts_returns_all_when_no_filters() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:a1", "a1.test", Some("a@x")).await;
        seed_test_account(&ctx, "did:plc:b2", "b2.test", Some("b@x")).await;
        seed_test_account(&ctx, "did:plc:c3", "c3.test", None).await;

        let resp = ops_list_accounts(State(ctx), admin_test_auth(), Query(ops_list_query()))
            .await
            .unwrap()
            .0;
        assert_eq!(resp.accounts.len(), 3);
    }

    #[tokio::test]
    async fn test_ops_list_accounts_takedown_status_filter() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:active", "ok.test", None).await;
        seed_test_account(&ctx, "did:plc:downed", "down.test", None).await;
        ctx.account_manager
            .takedown_account("did:plc:downed", "ticket-1")
            .await
            .unwrap();

        let mut q = ops_list_query();
        q.status = Some("takedown".to_string());
        let resp = ops_list_accounts(State(ctx.clone()), admin_test_auth(), Query(q))
            .await
            .unwrap()
            .0;
        assert_eq!(resp.accounts.len(), 1);
        assert_eq!(resp.accounts[0].did, "did:plc:downed");

        let mut q = ops_list_query();
        q.status = Some("active".to_string());
        let resp = ops_list_accounts(State(ctx), admin_test_auth(), Query(q))
            .await
            .unwrap()
            .0;
        assert_eq!(resp.accounts.len(), 1);
        assert_eq!(resp.accounts[0].did, "did:plc:active");
    }

    #[tokio::test]
    async fn test_ops_list_accounts_signup_date_range_filters() {
        let ctx = create_test_context().await;
        // Seed accounts and override created_at directly in SQL for the test.
        seed_test_account(&ctx, "did:plc:old", "old.test", None).await;
        seed_test_account(&ctx, "did:plc:mid", "mid.test", None).await;
        seed_test_account(&ctx, "did:plc:new", "new.test", None).await;
        sqlx::query("UPDATE actor SET created_at = ? WHERE did = ?")
            .bind("2024-01-01T00:00:00+00:00")
            .bind("did:plc:old")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        sqlx::query("UPDATE actor SET created_at = ? WHERE did = ?")
            .bind("2025-01-01T00:00:00+00:00")
            .bind("did:plc:mid")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        sqlx::query("UPDATE actor SET created_at = ? WHERE did = ?")
            .bind("2026-01-01T00:00:00+00:00")
            .bind("did:plc:new")
            .execute(&ctx.account_db)
            .await
            .unwrap();

        // Window catches just the middle one.
        let mut q = ops_list_query();
        q.signup_date_from = Some("2024-06-01T00:00:00+00:00".to_string());
        q.signup_date_to = Some("2025-06-01T00:00:00+00:00".to_string());
        let resp = ops_list_accounts(State(ctx), admin_test_auth(), Query(q))
            .await
            .unwrap()
            .0;
        assert_eq!(resp.accounts.len(), 1);
        assert_eq!(resp.accounts[0].did, "did:plc:mid");
    }

    #[tokio::test]
    async fn test_ops_list_accounts_invite_source_filter() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:inviter", "inv.test", None).await;
        seed_test_account(&ctx, "did:plc:invited", "vee.test", None).await;
        seed_test_account(&ctx, "did:plc:other", "other.test", None).await;

        // Create an invite code by inviter and have invited use it.
        let code = ctx
            .invite_manager
            .create_invite("did:plc:inviter", 5, None, None, None)
            .await
            .unwrap();
        ctx.invite_manager
            .use_code(&code.code, "did:plc:invited")
            .await
            .unwrap();

        let mut q = ops_list_query();
        q.invite_source = Some("did:plc:inviter".to_string());
        let resp = ops_list_accounts(State(ctx), admin_test_auth(), Query(q))
            .await
            .unwrap()
            .0;
        assert_eq!(resp.accounts.len(), 1);
        assert_eq!(resp.accounts[0].did, "did:plc:invited");
    }

    #[tokio::test]
    async fn test_ops_list_accounts_paginates_with_cursor() {
        let ctx = create_test_context().await;
        for did in ["did:plc:a", "did:plc:b", "did:plc:c", "did:plc:d"] {
            seed_test_account(&ctx, did, &format!("{}.test", &did[8..]), None).await;
        }

        let mut q = ops_list_query();
        q.limit = Some(2);
        let page1 = ops_list_accounts(State(ctx.clone()), admin_test_auth(), Query(q))
            .await
            .unwrap()
            .0;
        assert_eq!(page1.accounts.len(), 2);
        let cursor = page1.cursor.clone().expect("cursor expected");

        let mut q = ops_list_query();
        q.limit = Some(2);
        q.cursor = Some(cursor);
        let page2 = ops_list_accounts(State(ctx), admin_test_auth(), Query(q))
            .await
            .unwrap()
            .0;
        assert_eq!(page2.accounts.len(), 2);
        // No overlap.
        let p1: Vec<_> = page1.accounts.iter().map(|a| a.did.as_str()).collect();
        let p2: Vec<_> = page2.accounts.iter().map(|a| a.did.as_str()).collect();
        for did in &p2 {
            assert!(!p1.contains(did), "page2 should not overlap page1");
        }
    }

    #[tokio::test]
    async fn test_ops_list_accounts_validation_limit_out_of_range() {
        let ctx = create_test_context().await;
        let mut q = ops_list_query();
        q.limit = Some(0);
        let err = ops_list_accounts(State(ctx.clone()), admin_test_auth(), Query(q))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);

        let mut q = ops_list_query();
        q.limit = Some(101);
        let err = ops_list_accounts(State(ctx), admin_test_auth(), Query(q))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_ops_list_accounts_validation_unknown_status() {
        let ctx = create_test_context().await;
        let mut q = ops_list_query();
        q.status = Some("on-fire".to_string());
        let err = ops_list_accounts(State(ctx), admin_test_auth(), Query(q))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("status"));
    }

    #[tokio::test]
    async fn test_ops_list_accounts_validation_bad_date() {
        let ctx = create_test_context().await;
        let mut q = ops_list_query();
        q.signup_date_from = Some("yesterday".to_string());
        let err = ops_list_accounts(State(ctx), admin_test_auth(), Query(q))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("RFC3339"));
    }

    #[tokio::test]
    async fn test_ops_list_accounts_validation_bad_invite_source() {
        let ctx = create_test_context().await;
        let mut q = ops_list_query();
        q.invite_source = Some("not-a-did".to_string());
        let err = ops_list_accounts(State(ctx), admin_test_auth(), Query(q))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("DID"));
    }

    // ---- tools.aurora.ops.getInstanceMetrics (chainlink #84 / Phase 2.3.8) ----

    #[tokio::test]
    async fn test_ops_get_instance_metrics_empty_instance() {
        let ctx = create_test_context().await;
        let resp = ops_get_instance_metrics(State(ctx), admin_test_auth())
            .await
            .unwrap()
            .0;

        // System health: db is alive, version matches config.
        assert_eq!(resp.system_health.status, "healthy");
        assert_eq!(resp.system_health.version, "0.1.0-test");

        // Account growth: empty DB = zero counts everywhere.
        assert_eq!(resp.account_growth.total_accounts, 0);
        assert_eq!(resp.account_growth.signups_last_24h, 0);
        assert_eq!(resp.account_growth.signups_last_7d, 0);
        assert_eq!(resp.account_growth.signups_last_30d, 0);

        // Federation: disabled in test config.
        assert!(!resp.federation_health.federation_enabled);
        assert!(!resp.federation_health.relay_connected);
        assert_eq!(resp.federation_health.known_instances, 0);
    }

    #[tokio::test]
    async fn test_ops_get_instance_metrics_account_growth_window() {
        let ctx = create_test_context().await;
        // Three accounts, one in each of 24h / 7d-not-24h / older windows.
        seed_test_account(&ctx, "did:plc:fresh", "fresh.test", None).await;
        seed_test_account(&ctx, "did:plc:weekish", "weekish.test", None).await;
        seed_test_account(&ctx, "did:plc:ancient", "ancient.test", None).await;
        // Move "weekish" to ~3 days ago (in 7d window, not 24h).
        sqlx::query("UPDATE actor SET created_at = ? WHERE did = ?")
            .bind(
                (chrono::Utc::now() - chrono::Duration::days(3))
                    .to_rfc3339(),
            )
            .bind("did:plc:weekish")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        // Move "ancient" to ~60 days ago (outside all windows).
        sqlx::query("UPDATE actor SET created_at = ? WHERE did = ?")
            .bind(
                (chrono::Utc::now() - chrono::Duration::days(60))
                    .to_rfc3339(),
            )
            .bind("did:plc:ancient")
            .execute(&ctx.account_db)
            .await
            .unwrap();

        let resp = ops_get_instance_metrics(State(ctx), admin_test_auth())
            .await
            .unwrap()
            .0;

        assert_eq!(resp.account_growth.total_accounts, 3);
        assert_eq!(resp.account_growth.signups_last_24h, 1, "fresh");
        assert_eq!(
            resp.account_growth.signups_last_7d, 2,
            "fresh + weekish"
        );
        assert_eq!(
            resp.account_growth.signups_last_30d, 2,
            "ancient is outside 30d"
        );
    }

    #[tokio::test]
    async fn test_ops_get_instance_metrics_resource_usage_db_pool() {
        let ctx = create_test_context().await;
        let resp = ops_get_instance_metrics(State(ctx), admin_test_auth())
            .await
            .unwrap()
            .0;

        // db_pool_size > 0 because at least one connection is open after the
        // SELECT 1 and the COUNT(*) queries above. Same for idle.
        assert!(resp.resource_usage.db_pool_size >= 1);
    }

    // ---- tools.aurora.describeCapabilities (chainlink #99 / Phase 3.2) ----

    /// Admin route registry completeness — pins the
    /// `tools.aurora.admin.describeCapabilities` wire output AND
    /// asserts structural invariants the registry must satisfy.
    /// Renamed from `describe_capabilities_snapshot` during Arc 8
    /// Step 4 (chainlink #54): the test still snapshots the wire
    /// shape, but its load-bearing purpose post-Arc-8 is to prove
    /// every advertised family and every `WIRE_EXTENSION_ORDER`
    /// entry round-trip through the registry-driven handler.
    ///
    /// What the byte-for-byte literal pins:
    ///
    /// - Top-level field set (`extensions`, `families`,
    ///   `implementation`, `version`) and ordering (alphabetical via
    ///   canonical-JSON).
    /// - All 17 advertised capability extension strings (the Arc 2
    ///   Step 0 recon Q2 set plus the v0.9 Arc B/D additions —
    ///   `themes-v1`, `kryphocron-rotation-v1`, `kryphocron-read-v1`;
    ///   two further §8.15 vocabulary entries — `invite-lineage-v1`
    ///   and `reporter-context-v1` — remain intentionally omitted
    ///   because their endpoints aren't shipped).
    /// - The four namespace keys (`tools.aurora.admin`, `.moderator`,
    ///   `.ops`, `.superadmin`) and every endpoint within each.
    /// - `implementation` literal "aurora-locus" and the pinned
    ///   `version` string from CARGO_PKG_VERSION (bumped in lockstep
    ///   with the cycle).
    ///
    /// What the structural assertions (run after the byte-for-byte
    /// equality) add: inspectable invariant failures (`families`
    /// contains the four namespace keys; `extensions` matches
    /// `WIRE_EXTENSION_ORDER` element-for-element). When the test
    /// fails the byte-for-byte assertion has the canonical
    /// diagnostic, but the structural assertions give a
    /// human-readable second opinion on what specifically drifted.
    ///
    /// Determinism rationale: `serde_json::Map` defaults to a
    /// `BTreeMap` (no `preserve_order` feature) so namespace keys
    /// inside `families` come out alphabetically. Extensions output
    /// comes from `WIRE_EXTENSION_ORDER` filtered by present-set
    /// across registry entries
    /// (`RouteRegistry::advertised_extensions`). Endpoint arrays
    /// inside each family come from
    /// `RouteRegistry::advertised_by_family`, which sorts by
    /// `registration_order` within each family (preserving phase-
    /// introduction order per Step 0 Q5 disposition (a) for
    /// accidental orderings). No `HashMap` iteration anywhere on
    /// the wire path.
    #[tokio::test]
    async fn test_admin_route_registry_completeness() {
        let ctx = create_test_context().await;
        let resp = describe_capabilities(State(ctx), admin_test_auth())
            .await
            .unwrap()
            .0;
        let actual = canonical_json(&resp);
        let expected = concat!(
            r#"{"#,
            // ---- extensions: 18 strings in Vec declaration order ----
            r#""extensions":["#,
            r#"{"name":"subject-context-v1"},"#,
            r#"{"name":"moderator-activity-v1"},"#,
            r#"{"name":"subject-history-v1"},"#,
            r#"{"name":"appeals-v1"},"#,
            r#"{"name":"instance-metrics-v1"},"#,
            r#"{"name":"mod-events-emit-v1"},"#,
            r#"{"name":"batch-takedown-v1"},"#,
            r#"{"name":"trigger-password-reset-v1"},"#,
            r#"{"name":"moderation-metrics-v1"},"#,
            r#"{"name":"queue-stats-v1"},"#,
            r#"{"name":"audit-trail-v1"},"#,
            r#"{"name":"forensic-export-v1"},"#,
            r#"{"name":"mod-events-stream-v1"},"#,
            r#"{"name":"runtime-settings-v1"},"#,
            r#"{"name":"themes-v1"},"#,
            r#"{"name":"kryphocron-rotation-v1"},"#,
            r#"{"name":"kryphocron-read-v1"},"#,
            r#"{"name":"session-management-v1"},"#,
            r#"{"name":"kryphocron-overrides-v1"}"#,
            r#"],"#,
            // ---- families: 4 namespaces, alphabetical keys ----
            r#""families":{"#,
            // tools.aurora.admin (19 endpoints)
            r#""tools.aurora.admin":["#,
            r#""emitEvent","#,
            r#""batchTakedownAccounts","#,
            r#""batchSuspendAccounts","#,
            r#""batchRestoreAccounts","#,
            r#""batchTakedownRecords","#,
            r#""batchApplyLabel","#,
            r#""batchRemoveLabel","#,
            r#""triggerPasswordReset","#,
            r#""getQueueStats","#,
            r#""getModerationMetrics","#,
            r#""getAuditTrail","#,
            r#""getReport","#,
            r#""exportAccountForensic","#,
            r#""subscribeModEvents","#,
            r#""getRuntimeSetting","#,
            r#""setRuntimeSetting","#,
            r#""listInstalled","#,
            r#""listSessions","#,
            r#""revokeSession","#,
            r#""revokeOperatorSessions""#,
            r#"],"#,
            // tools.aurora.moderator (7 endpoints)
            r#""tools.aurora.moderator":["#,
            r#""queryEvents","#,
            r#""getEvent","#,
            r#""queryStatuses","#,
            r#""getSubjectContext","#,
            r#""getSubjectHistory","#,
            r#""listAppeals","#,
            r#""getAppeal""#,
            r#"],"#,
            // tools.aurora.ops (51 endpoints)
            r#""tools.aurora.ops":["#,
            r#""getStats","#,
            r#""listAccounts","#,
            r#""getInstanceMetrics","#,
            r#""getValidationFailures","#,
            r#""getSystemHealth","#,
            r#""getDatabaseStatus","#,
            r#""getResourceUsage","#,
            r#""listBackgroundJobs","#,
            r#""runHealthChecks","#,
            r#""getVersionInfo","#,
            r#""getSystemMetrics","#,
            r#""getNonceStoreStatus","#,
            r#""cleanupNonceStores","#,
            r#""getBlobStatistics","#,
            r#""listBlobs","#,
            r#""deleteBlob","#,
            r#""quarantineBlob","#,
            r#""restoreBlob","#,
            r#""runBlobGC","#,
            r#""getBlobQuotas","#,
            r#""getSequencerStatus","#,
            r#""pauseSequencer","#,
            r#""resumeSequencer","#,
            r#""resetSequencerCursor","#,
            r#""rebuildSequencer","#,
            r#""getRateLimitConfig","#,
            r#""getRateLimitStatus","#,
            r#""cleanupRateLimitState","#,
            r#""getFederationStatus","#,
            r#""getRelayConfig","#,
            r#""listKnownInstances","#,
            r#""triggerPdsDiscovery","#,
            r#""getFederationPolicy","#,
            r#""addFederationPeer","#,
            r#""removeFederationPeer","#,
            r#""modifyFederationPeer","#,
            r#""setDiscoveryMode","#,
            r#""dismissPendingDiscovery","#,
            r#""addRelayUrl","#,
            r#""removeRelayUrl","#,
            r#""setFederationRelays","#,
            r#""triggerRotation","#,
            // v0.9 Arc D (#225) — kryphocron operator read cohort.
            r#""getSubstrateInfo","#,
            r#""getTierStats","#,
            r#""getOracleActivity","#,
            r#""getRotationStatus","#,
            r#""getRotationProgress","#,
            r#""cancelRotation","#,
            r#""listRotations","#,
            r#""getAudienceAggregate","#,
            r#""listAudiences","#,
            r#""getBlockCascadeImpact","#,
            r#""getAccountOverrides","#,
            r#""setAccountOverride""#,
            r#"],"#,
            // tools.aurora.superadmin (33 endpoints)
            r#""tools.aurora.superadmin":["#,
            r#""grantRole","#,
            r#""revokeRole","#,
            r#""assignReviewer","#,
            r#""createAutoLabelRule","#,
            r#""editAutoLabelRule","#,
            r#""deleteAutoLabelRule","#,
            r#""listAutoLabelRules","#,
            r#""createEscalationRule","#,
            r#""editEscalationRule","#,
            r#""deleteEscalationRule","#,
            r#""listEscalationRules","#,
            r#""clearEscalation","#,
            r#""getDefaultsState","#,
            r#""createHook","#,
            r#""editHook","#,
            r#""deleteHook","#,
            r#""listHooks","#,
            r#""getIntegrationHooksState","#,
            r#""uploadBrandingAsset","#,
            r#""preRebuildCheck","#,
            r#""rebuildRepo","#,
            r#""getRebuildProgress","#,
            r#""cancelRebuild","#,
            r#""scanReposForInconsistencies","#,
            r#""getScanProgress","#,
            r#""cancelScan","#,
            r#""getRepoScanResults","#,
            r#""repairRepos","#,
            r#""getBulkRepairProgress","#,
            r#""cancelBulkRepair","#,
            r#""sequencerRecoveryOptions","#,
            r#""runSequencerRecovery","#,
            r#""getSequencerRecoveryProgress","#,
            r#""cancelSequencerRecovery""#,
            r#"]"#,
            r#"},"#,
            // ---- implementation, version (literals) ----
            r#""implementation":"aurora-locus","#,
            r#""version":"0.8.0""#,
            r#"}"#,
        );
        assert_eq!(
            actual, expected,
            "describeCapabilities wire shape changed — \
             update the snapshot AND the §6.3.1 commitment if the \
             change is intentional, otherwise revert"
        );

        // Structural invariants — surface drift with
        // human-readable failures alongside the byte-for-byte
        // diagnostic above. Both pass-together / fail-together
        // in normal operation; a structural-only failure points
        // at a registry/wire-order mismatch the byte-for-byte
        // assertion would also catch but with less direct
        // signal.

        let families_obj = resp
            .families
            .as_object()
            .expect("describeCapabilities.families is a JSON object");
        for namespace in [
            "tools.aurora.admin",
            "tools.aurora.moderator",
            "tools.aurora.ops",
            "tools.aurora.superadmin",
        ] {
            assert!(
                families_obj.contains_key(namespace),
                "families output missing namespace {} — \
                 RouteRegistry::advertised_by_family didn't emit it; \
                 check that admin::routes() registers at least one \
                 route_with_caps() for the family",
                namespace,
            );
        }

        let extension_names: Vec<&str> =
            resp.extensions.iter().map(|e| e.name.as_str()).collect();
        let expected_extensions: Vec<&str> =
            crate::api::registry::WIRE_EXTENSION_ORDER.to_vec();
        assert_eq!(
            extension_names, expected_extensions,
            "extensions output diverges from WIRE_EXTENSION_ORDER — \
             either a registered route is attributing an extension \
             not in the wire-order constant (debug_assert! in \
             RouteRegistry::advertised_extensions catches this in \
             dev/test builds) or the present-set filter is missing \
             an extension that should be advertised"
        );
    }

    // ---- v0.9 Arc D (#225) — kryphocron operator read cohort ----
    //
    // `create_test_context()` builds a kryphocron-disabled deployment (no
    // at-rest hooks / oracle / rewrite job), so these exercise the handlers'
    // entry paths: the `KryphocronDisabled` 400 every endpoint returns when the
    // substrate is absent, and the Admin+ role gate the Laquna-control reads
    // enforce before that. The endpoints' computational core (tier walks,
    // audience tally, cascade parsing, rewrite-job state machine, oracle
    // accessors) is unit-tested in `aurora_kryphocron_ops`, `kryphocron_rewrite`
    // and `kryphocron_rotation`; the dispatchability of all ten is pinned by the
    // describeCapabilities snapshot above.

    /// Status of an `ApiErr`-returning kryphocron-ops handler result.
    fn err_status<T>(r: Result<T, (StatusCode, Json<serde_json::Value>)>) -> StatusCode {
        match r {
            Ok(_) => panic!("expected an error result on a kryphocron-disabled context"),
            Err((status, _)) => status,
        }
    }

    #[tokio::test]
    async fn get_oracle_activity_reports_instrumented_audience_counts() {
        // #335 — with kryphocron enabled (the v0.9 default), the endpoint
        // surfaces the audience-oracle consultation tally as instrumented
        // aggregate counts (not the old instrumented:false stub).
        use crate::api::aurora_kryphocron_ops as k;
        use crate::kryphocron_oracle_activity::OracleConsultation;
        let ctx = create_test_context().await;
        ctx.audience_oracle_activity.record(OracleConsultation::WriteAllowed);
        ctx.audience_oracle_activity.record(OracleConsultation::WriteDenied);
        ctx.audience_oracle_activity.record(OracleConsultation::WriteDeferred);
        ctx.audience_oracle_activity.record(OracleConsultation::ReadAuthorized);
        ctx.audience_oracle_activity.record(OracleConsultation::ReadAuthorized);

        let body = k::get_oracle_activity(State(ctx.clone()), admin_test_auth())
            .await
            .expect("enabled substrate returns activity")
            .0;
        assert_eq!(body["instrumented"], true);
        assert_eq!(body["oracle"], "audience");
        assert!(body["since"].is_string());
        assert_eq!(body["consultations"]["total"], 5);
        assert_eq!(body["consultations"]["write"]["allowed"], 1);
        assert_eq!(body["consultations"]["write"]["denied"], 1);
        assert_eq!(body["consultations"]["write"]["deferred"], 1);
        assert_eq!(body["consultations"]["read"]["authorized"], 2);
        assert_eq!(body["consultations"]["read"]["denied"], 0);
    }

    #[tokio::test]
    async fn test_kryphocron_ops_disabled_returns_400() {
        use crate::api::aurora_kryphocron_ops as k;
        // Kryphocron is on by default (v0.9); this test exercises the
        // explicitly-disabled deployment, so opt the fixture back out.
        let ctx = create_test_context_with(|c| c.kryphocron.enabled = false).await;
        let acct = || {
            axum::extract::Query(k::AccountFilter {
                account: "did:plc:test".to_string(),
            })
        };

        // Moderator+ reads: disabled substrate ⇒ 400 (no role gate).
        assert_eq!(
            err_status(k::get_substrate_info(State(ctx.clone()), admin_test_auth()).await),
            StatusCode::BAD_REQUEST,
        );
        assert_eq!(
            err_status(k::get_tier_stats(State(ctx.clone()), admin_test_auth()).await),
            StatusCode::BAD_REQUEST,
        );
        assert_eq!(
            err_status(k::get_oracle_activity(State(ctx.clone()), admin_test_auth()).await),
            StatusCode::BAD_REQUEST,
        );
        assert_eq!(
            err_status(k::get_rotation_status(State(ctx.clone()), admin_test_auth()).await),
            StatusCode::BAD_REQUEST,
        );
        assert_eq!(
            err_status(k::get_audience_aggregate(State(ctx.clone()), admin_test_auth()).await),
            StatusCode::BAD_REQUEST,
        );
        assert_eq!(
            err_status(k::list_audiences(State(ctx.clone()), admin_test_auth(), acct()).await),
            StatusCode::BAD_REQUEST,
        );
        assert_eq!(
            err_status(
                k::get_block_cascade_impact(State(ctx.clone()), admin_test_auth(), acct()).await
            ),
            StatusCode::BAD_REQUEST,
        );

        // Admin+ reads with an Admin caller: role gate passes, disabled ⇒ 400.
        assert_eq!(
            err_status(k::get_rotation_progress(State(ctx.clone()), admin_test_auth()).await),
            StatusCode::BAD_REQUEST,
        );
        assert_eq!(
            err_status(k::cancel_rotation(State(ctx.clone()), admin_test_auth()).await),
            StatusCode::BAD_REQUEST,
        );
        assert_eq!(
            err_status(k::list_rotations(State(ctx), admin_test_auth()).await),
            StatusCode::BAD_REQUEST,
        );
    }

    #[tokio::test]
    async fn test_kryphocron_ops_admin_gate_denies_moderator() {
        use crate::api::aurora_kryphocron_ops as k;
        // The Admin-gate (403) checks fire before the disabled (400) check, but
        // the final assertion reaches the disabled check — so opt the fixture
        // out of the now-default-on kryphocron to keep exercising that 400.
        let ctx = create_test_context_with(|c| c.kryphocron.enabled = false).await;

        // The three Laquna-control reads gate at Admin+ (§6.4.2 / §6.4.2.1):
        // a Moderator is rejected with 403 BEFORE the disabled check.
        assert_eq!(
            err_status(k::get_rotation_progress(State(ctx.clone()), moderator_test_auth()).await),
            StatusCode::FORBIDDEN,
        );
        assert_eq!(
            err_status(k::cancel_rotation(State(ctx.clone()), moderator_test_auth()).await),
            StatusCode::FORBIDDEN,
        );
        assert_eq!(
            err_status(k::list_rotations(State(ctx.clone()), moderator_test_auth()).await),
            StatusCode::FORBIDDEN,
        );

        // A Moderator+ observability read is NOT Admin-gated: it reaches the
        // disabled check and returns 400, not 403.
        assert_eq!(
            err_status(k::get_substrate_info(State(ctx), moderator_test_auth()).await),
            StatusCode::BAD_REQUEST,
        );
    }

    #[tokio::test]
    async fn test_describe_capabilities_returns_expected_shape() {
        let ctx = create_test_context().await;
        let resp = describe_capabilities(State(ctx), admin_test_auth())
            .await
            .unwrap()
            .0;

        assert_eq!(resp.implementation, "aurora-locus");
        // version comes from CARGO_PKG_VERSION; pinned to the
        // current cycle release per CR-4 / chainlink #117. Bump in
        // lockstep with Cargo.toml when the cycle increments.
        assert_eq!(resp.version, "0.8.0");

        // Families object must include the four Aurora namespaces, each
        // a JSON array (possibly empty for namespaces that haven't
        // shipped endpoints yet).
        let families = resp.families.as_object().expect("families is object");
        for ns in [
            "tools.aurora.ops",
            "tools.aurora.moderator",
            "tools.aurora.admin",
            "tools.aurora.superadmin",
        ] {
            assert!(
                families.get(ns).map(|v| v.is_array()).unwrap_or(false),
                "missing or non-array family: {}",
                ns
            );
        }
    }

    #[tokio::test]
    async fn test_describe_capabilities_lists_phase_2_3_ops_endpoints() {
        // Sanity-check the static list against what's actually shipped
        // — every endpoint named here was registered in Phase 2.3
        // (chainlink #84). Future sub-phases extend the list; this
        // test guards against the static list silently drifting from
        // the route registrations.
        let ctx = create_test_context().await;
        let resp = describe_capabilities(State(ctx), admin_test_auth())
            .await
            .unwrap()
            .0;

        let ops = resp
            .families
            .get("tools.aurora.ops")
            .and_then(|v| v.as_array())
            .expect("ops family present");
        let ops_names: Vec<&str> =
            ops.iter().map(|v| v.as_str().unwrap()).collect();

        // Spot-check several Phase 2.3 endpoints across categories.
        for expected in [
            "getStats",
            "listAccounts",
            "getInstanceMetrics",
            "pauseSequencer",
            "getFederationStatus",
        ] {
            assert!(
                ops_names.contains(&expected),
                "Phase 2.3 endpoint {} missing from capability list",
                expected
            );
        }
    }

    // ---- tools.aurora.superadmin.{grant,revoke}Role (chainlink #103 / Phase 3.6) ----

    #[tokio::test]
    async fn test_grant_role_rejects_non_superadmin() {
        // Admin is not enough — role management is SuperAdmin-only post
        // Phase 3.6. Verifies the handler-level gate fires before any
        // role mutation reaches the database.
        let ctx = create_test_context().await;
        let req = GrantRoleRequest {
            did: "did:plc:nominee".to_string(),
            role: "moderator".to_string(),
            rationale: Some("test grant".to_string()),
        };
        let (status, body) = grant_role(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("Admin must not be allowed to grant roles");
        assert_eq!(status, StatusCode::FORBIDDEN);
        // G1.1 structured-error shape: `{error: "Forbidden", message: "..."}`.
        assert_eq!(body.0["error"], "Forbidden");
        let message = body.0["message"].as_str().expect("message must be a string");
        assert!(
            message.contains("SuperAdmin"),
            "error message should reference SuperAdmin requirement, got: {}",
            message
        );
    }

    #[tokio::test]
    async fn test_revoke_role_rejects_non_superadmin() {
        let ctx = create_test_context().await;
        let req = RevokeRoleRequest {
            did: "did:plc:victim".to_string(),
            rationale: Some("test revoke".to_string()),
        };
        let (status, body) = revoke_role(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("Admin must not be allowed to revoke roles");
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body.0["error"], "Forbidden");
        let message = body.0["message"].as_str().expect("message must be a string");
        assert!(message.contains("SuperAdmin"));
    }

    #[tokio::test]
    async fn test_grant_role_allowed_for_superadmin() {
        // Happy path: SuperAdmin grants a Moderator role; succeeds.
        let ctx = create_test_context().await;
        let req = GrantRoleRequest {
            did: "did:plc:newmod".to_string(),
            role: "moderator".to_string(),
            rationale: Some("trial period".to_string()),
        };
        let resp = grant_role(State(ctx.clone()), superadmin_test_auth(), Json(req))
            .await
            .expect("SuperAdmin should be allowed to grant roles");
        let output = resp.0;
        assert_eq!(output.did, "did:plc:newmod");
        assert_eq!(output.role, "moderator");
        assert!(
            !output.audit_entry_id.is_empty(),
            "grantRole must return the chain entry id"
        );
        // Wire-form check: serialize to JSON and confirm the field
        // surfaces as `auditEntryId` (camelCase) per the Arc 2
        // action-ID contract — the pre-Arc-2 wire form
        // `audit_entry_id` (snake_case) was renamed in Step 2.
        let wire = serde_json::to_value(&output).unwrap();
        assert!(
            wire.get("auditEntryId").is_some(),
            "grantRole wire output must use camelCase `auditEntryId`; full payload: {}",
            wire,
        );
        assert!(
            wire.get("audit_entry_id").is_none(),
            "grantRole must not emit the pre-Arc-2 snake_case `audit_entry_id`",
        );
    }

    #[tokio::test]
    async fn test_grant_role_rejects_missing_rationale() {
        let ctx = create_test_context().await;
        let req = GrantRoleRequest {
            did: "did:plc:newmod".to_string(),
            role: "moderator".to_string(),
            rationale: None,
        };
        let (status, body) = grant_role(State(ctx), superadmin_test_auth(), Json(req))
            .await
            .expect_err("missing rationale must be rejected");
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body.0["error"], "InvalidRequest");
        assert_eq!(body.0["message"], "rationale-required");
    }

    #[tokio::test]
    async fn test_grant_role_writes_to_audit_chain() {
        let ctx = create_test_context().await;
        let req = GrantRoleRequest {
            did: "did:plc:newmod".to_string(),
            role: "moderator".to_string(),
            rationale: Some("on-call rotation".to_string()),
        };
        let _ = grant_role(State(ctx.clone()), superadmin_test_auth(), Json(req))
            .await
            .expect("SuperAdmin grant succeeds");
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_chain_entry \
             WHERE action = $1 AND subject_did = $2",
        )
        .bind("role.grant")
        .bind("did:plc:newmod")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(count, 1, "grant_role must write exactly one chain entry");
    }

    #[tokio::test]
    async fn test_revoke_role_rejects_missing_rationale() {
        let ctx = create_test_context().await;
        let req = RevokeRoleRequest {
            did: "did:plc:nobody".to_string(),
            rationale: None,
        };
        let (status, body) = revoke_role(State(ctx), superadmin_test_auth(), Json(req))
            .await
            .expect_err("missing rationale must be rejected");
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body.0["error"], "InvalidRequest");
        assert_eq!(body.0["message"], "rationale-required");
    }

    #[tokio::test]
    async fn test_describe_capabilities_advertises_superadmin_endpoints() {
        // Phase 3.6 adds grantRole + revokeRole to the superadmin
        // family. Catches accidental removal from the static list.
        let ctx = create_test_context().await;
        let resp = describe_capabilities(State(ctx), admin_test_auth())
            .await
            .unwrap()
            .0;
        let superadmin = resp
            .families
            .get("tools.aurora.superadmin")
            .and_then(|v| v.as_array())
            .expect("superadmin family present");
        let names: Vec<&str> =
            superadmin.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(names.contains(&"grantRole"), "grantRole missing");
        assert!(names.contains(&"revokeRole"), "revokeRole missing");
        assert!(names.contains(&"preRebuildCheck"), "preRebuildCheck missing");
        assert!(names.contains(&"rebuildRepo"), "rebuildRepo missing");
        assert!(names.contains(&"getRebuildProgress"), "getRebuildProgress missing");
        assert!(names.contains(&"cancelRebuild"), "cancelRebuild missing");
        assert!(names.contains(&"scanReposForInconsistencies"), "scanReposForInconsistencies missing");
        assert!(names.contains(&"getScanProgress"), "getScanProgress missing");
        assert!(names.contains(&"cancelScan"), "cancelScan missing");
        assert!(names.contains(&"getRepoScanResults"), "getRepoScanResults missing");
        assert!(names.contains(&"repairRepos"), "repairRepos missing");
        assert!(names.contains(&"getBulkRepairProgress"), "getBulkRepairProgress missing");
        assert!(names.contains(&"cancelBulkRepair"), "cancelBulkRepair missing");
        assert!(names.contains(&"sequencerRecoveryOptions"), "sequencerRecoveryOptions missing");
        assert!(names.contains(&"runSequencerRecovery"), "runSequencerRecovery missing");
        assert!(names.contains(&"getSequencerRecoveryProgress"), "getSequencerRecoveryProgress missing");
        assert!(names.contains(&"cancelSequencerRecovery"), "cancelSequencerRecovery missing");
    }

    #[tokio::test]
    async fn test_describe_capabilities_advertises_shipped_canonical_set() {
        // Phase 3.5–3.10 each shipped capability families; this test
        // pins the §8.15 canonical vocabulary the server advertises
        // so the admin UI's Server → Capabilities page renders the
        // correct ✓/✗ matrix. Capabilities whose endpoints have not
        // shipped (`invite-lineage-v1`, `reporter-context-v1`) are
        // intentionally absent — gating affordances on absent
        // capabilities is the entire point of the probe.
        let ctx = create_test_context().await;
        let resp = describe_capabilities(State(ctx), admin_test_auth())
            .await
            .unwrap()
            .0;
        let names: Vec<&str> = resp.extensions.iter().map(|e| e.name.as_str()).collect();
        let expected = [
            "subject-context-v1",
            "moderator-activity-v1",
            "subject-history-v1",
            "appeals-v1",
            "instance-metrics-v1",
            "mod-events-emit-v1",
            "batch-takedown-v1",
            "trigger-password-reset-v1",
            "moderation-metrics-v1",
            "queue-stats-v1",
            "audit-trail-v1",
            "forensic-export-v1",
            "mod-events-stream-v1",
            "runtime-settings-v1",
        ];
        for cap in expected {
            assert!(
                names.contains(&cap),
                "missing capability {}; advertised set = {:?}",
                cap,
                names
            );
        }
        // Guard against accidentally advertising capabilities whose
        // endpoints aren't shipped yet.
        for forbidden in ["invite-lineage-v1", "reporter-context-v1"] {
            assert!(
                !names.contains(&forbidden),
                "{} must not be advertised — its endpoints aren't shipped",
                forbidden
            );
        }
    }

    // ---- Phase 1.7: account/did deprecation-alias rollout ---------------

    #[tokio::test]
    async fn test_resolve_helper_rejects_both_fields() {
        let ctx = create_test_context().await;
        let result = resolve_account_or_did(
            &ctx,
            Some("did:plc:foo"),
            Some("did:plc:foo"),
        )
        .await;
        let err = result.expect_err("both fields should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("exactly one"));
    }

    #[tokio::test]
    async fn test_resolve_helper_rejects_neither_field() {
        let ctx = create_test_context().await;
        let result = resolve_account_or_did(&ctx, None, None).await;
        let err = result.expect_err("missing both should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("Missing"));
    }

    #[tokio::test]
    async fn test_resolve_helper_did_form_account_returns_as_is() {
        let ctx = create_test_context().await;
        let did = resolve_account_or_did(&ctx, Some("did:plc:abcd"), None)
            .await
            .unwrap();
        assert_eq!(did, "did:plc:abcd");
    }

    #[tokio::test]
    async fn test_resolve_helper_handle_form_account_resolves_via_db() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:alice", "alice.test", Some("alice@x")).await;

        let did = resolve_account_or_did(&ctx, Some("alice.test"), None)
            .await
            .unwrap();
        assert_eq!(did, "did:plc:alice");
    }

    #[tokio::test]
    async fn test_resolve_helper_legacy_did_field_works() {
        let ctx = create_test_context().await;
        let did = resolve_account_or_did(&ctx, None, Some("did:plc:legacy"))
            .await
            .unwrap();
        assert_eq!(did, "did:plc:legacy");
    }

    #[tokio::test]
    async fn test_resolve_helper_legacy_did_field_rejects_handle_form() {
        let ctx = create_test_context().await;
        let result = resolve_account_or_did(&ctx, None, Some("not-a-did")).await;
        let err = result.expect_err("legacy did field should reject non-DID");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    /// Helper: returns whether `account.invites_disabled` is set for a DID.
    async fn account_invites_disabled(ctx: &AppContext, did: &str) -> bool {
        use sqlx::Row;
        let row: i64 = sqlx::query("SELECT invites_disabled FROM account WHERE did = ?")
            .bind(did)
            .fetch_one(&ctx.account_db)
            .await
            .unwrap()
            .get(0);
        row != 0
    }

    #[tokio::test]
    async fn test_disable_account_invites_with_account_field_did_form() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:foo", "foo.test", Some("f@x")).await;

        let req = AccountInvitesRequest {
            account: Some("did:plc:foo".to_string()),
            did: None,
            note: None,
        };
        let status = disable_account_invites(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        // Side-effect verification: the canonical DID's row had its flag
        // flipped (this also verifies the resolver pointed at the right row).
        assert!(account_invites_disabled(&ctx, "did:plc:foo").await);
    }

    #[tokio::test]
    async fn test_disable_account_invites_with_account_field_handle_form() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:bar", "bar.test", Some("b@x")).await;

        let req = AccountInvitesRequest {
            account: Some("bar.test".to_string()),
            did: None,
            note: None,
        };
        let status = disable_account_invites(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        // Verifies the resolver mapped handle "bar.test" to "did:plc:bar".
        assert!(account_invites_disabled(&ctx, "did:plc:bar").await);
    }

    #[tokio::test]
    async fn test_disable_account_invites_with_legacy_did_field() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:baz", "baz.test", None).await;

        let req = AccountInvitesRequest {
            account: None,
            did: Some("did:plc:baz".to_string()),
            note: None,
        };
        let status = disable_account_invites(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert!(account_invites_disabled(&ctx, "did:plc:baz").await);
    }

    #[tokio::test]
    async fn test_disable_account_invites_rejects_both_fields() {
        let ctx = create_test_context().await;
        let req = AccountInvitesRequest {
            account: Some("did:plc:x".to_string()),
            did: Some("did:plc:x".to_string()),
            note: None,
        };
        let err = disable_account_invites(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("both fields should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_disable_account_invites_rejects_neither_field() {
        let ctx = create_test_context().await;
        let req = AccountInvitesRequest {
            account: None,
            did: None,
            note: None,
        };
        let err = disable_account_invites(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("missing both fields should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_disable_account_invites_propagates_note_to_audit_chain() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:noted", "noted.test", None).await;

        let req = AccountInvitesRequest {
            account: Some("did:plc:noted".to_string()),
            did: None,
            note: Some("Spam ring cleanup 2026-Q2".to_string()),
        };
        let _ = disable_account_invites(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap();

        // Verify the chain row carries the note in the rationale column.
        // Migrated from the legacy admin_audit_log shape (chainlink #109);
        // the chain is the system of record for all administrative
        // decisions per docs/AURORA_ADMIN_UI_DESIGN.md §3.4.
        let row: (String, Option<String>, String) = sqlx::query_as(
            "SELECT action, subject_did, rationale FROM audit_chain_entry
             WHERE action = 'account.invites.disable' AND subject_did = ?",
        )
        .bind("did:plc:noted")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(row.0, "account.invites.disable");
        assert_eq!(row.1.as_deref(), Some("did:plc:noted"));
        assert_eq!(row.2, "Spam ring cleanup 2026-Q2");
    }

    #[tokio::test]
    async fn test_enable_account_invites_happy_path_uses_same_pattern() {
        // enableAccountInvites and disableAccountInvites share AccountInvitesRequest;
        // exercising one happy path here (in addition to the disable suite above)
        // confirms the symmetric handler registers and routes correctly.
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:enabled", "enabled.test", None).await;
        // Start in the disabled state so re-enabling is a real change.
        ctx.account_manager
            .disable_account_invites("did:plc:enabled")
            .await
            .unwrap();

        let req = AccountInvitesRequest {
            account: Some("enabled.test".to_string()),
            did: None,
            note: Some("Reinstated after appeal".to_string()),
        };
        let status = enable_account_invites(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        // Resolver mapped handle correctly and the underlying DB op flipped.
        assert!(!account_invites_disabled(&ctx, "did:plc:enabled").await);
    }

    /// Helper: count chain rows matching `(action, subject_did)`. The
    /// audit-chain coverage tests below all share this shape — every
    /// successful administrative call writes exactly one row, so the
    /// assertion is "row count is 1".
    async fn count_chain_rows(
        ctx: &AppContext,
        action: &str,
        subject_did: Option<&str>,
    ) -> i64 {
        match subject_did {
            Some(did) => sqlx::query_scalar(
                "SELECT COUNT(*) FROM audit_chain_entry \
                 WHERE action = ? AND subject_did = ?",
            )
            .bind(action)
            .bind(did)
            .fetch_one(&ctx.account_db)
            .await
            .unwrap(),
            None => sqlx::query_scalar(
                "SELECT COUNT(*) FROM audit_chain_entry \
                 WHERE action = ? AND subject_did IS NULL",
            )
            .bind(action)
            .fetch_one(&ctx.account_db)
            .await
            .unwrap(),
        }
    }

    // ---------------------------------------------------------------
    // L-1 chain coverage — happy-path tests that pin "exactly one chain
    // row per administrative decision" for one representative site in
    // each disposition category. Combined with the existing
    // category-specific behavior tests, this gives the chain
    // surface-level coverage every category needs without duplicating
    // every per-site behavior test (chainlink #109).
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn test_takedown_account_writes_chain_entry() {
        // Category A: account-subject. The chain entry's subject is the
        // takedown target's DID, action is "account.takedown", rationale
        // is the operator-supplied reason.
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:victimA", "victima.test", None).await;
        let req = TakedownAccountRequest {
            did: "did:plc:victimA".to_string(),
            reason: "spam-ring".to_string(),
            notes: None,
        };
        let _ = takedown_account(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .expect("takedown succeeds");
        assert_eq!(
            count_chain_rows(&ctx, "account.takedown", Some("did:plc:victimA")).await,
            1
        );
    }

    // LB-1 Session 12 / chainlink #129: handler-level atomicity
    // regression for takedown_account. The handler runs the
    // moderation row INSERT, the actor.takedown_ref UPDATE, and the
    // chain append all in one tx; rolling back the tx must leave
    // none of them in the database. This test exercises the
    // contract via the in_tx primitives the handler uses.
    #[tokio::test]
    async fn takedown_account_atomicity_rollback_pins_invariant() {
        use crate::admin::moderation::{ApplyActionParams, ModerationAction, ModerationManager};
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:victimT", "vt.test", None).await;
        {
            let _guard = audit_chain::AppendChainGuard::acquire().await;
            let mut tx = ctx.account_db.begin().await.unwrap();
            ModerationManager::apply_action_in_tx(
                &mut tx,
                ApplyActionParams {
                    did: "did:plc:victimT",
                    action: ModerationAction::Takedown,
                    reason: "would be rolled back",
                    moderated_by: "did:plc:admin",
                    expires_in: None,
                    report_id: None,
                    notes: None,
                },
            )
            .await
            .unwrap();
            audit_chain::insert_chain_entry(
                &mut tx,
                ctx.config.database.backend,
                AppendEntryParams {
                    source: "manual",
                    payload: None,
                    actor_did: "did:plc:admin",
                    action: "account.takedown",
                    subject: Some(&Subject::Repo {
                        did: "did:plc:victimT".to_string(),
                    }),
                    rationale: "would be rolled back",
                    snapshot_id: None,
                    event_id: None,
                    cascade_subjects: &[],
                    cascade_snapshot_ids: &[],
                },
            )
            .await
            .unwrap();
            tx.rollback().await.unwrap();
        }

        let mod_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM account_moderation WHERE did = 'did:plc:victimT'",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(mod_count, 0);
        let takedown: Option<String> = sqlx::query_scalar(
            "SELECT takedown_ref FROM actor WHERE did = 'did:plc:victimT'",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert!(takedown.is_none());
        assert_eq!(
            count_chain_rows(&ctx, "account.takedown", Some("did:plc:victimT")).await,
            0
        );
    }

    // LB-1 Session 12 / chainlink #129: handler-level atomicity
    // regression for update_account_handle. Pre-fix the handle
    // UPDATE and chain entry committed in separate transactions;
    // post-fix they share one. Rolling back the wrapping tx must
    // leave the handle and chain entry both unchanged.
    #[tokio::test]
    async fn update_account_handle_atomicity_rollback_pins_invariant() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:hh", "hh.test", None).await;
        {
            let _guard = audit_chain::AppendChainGuard::acquire().await;
            let mut tx = ctx.account_db.begin().await.unwrap();
            crate::account::AccountManager::update_handle_in_tx(
                &mut tx,
                "did:plc:hh",
                "renamed.test",
            )
            .await
            .unwrap();
            audit_chain::insert_chain_entry(
                &mut tx,
                ctx.config.database.backend,
                AppendEntryParams {
                    source: "manual",
                    payload: None,
                    actor_did: "did:plc:admin",
                    action: "account.update_handle",
                    subject: Some(&Subject::Repo {
                        did: "did:plc:hh".to_string(),
                    }),
                    rationale: "would be rolled back",
                    snapshot_id: None,
                    event_id: None,
                    cascade_subjects: &[],
                    cascade_snapshot_ids: &[],
                },
            )
            .await
            .unwrap();
            tx.rollback().await.unwrap();
        }

        let handle: Option<String> =
            sqlx::query_scalar("SELECT handle FROM actor WHERE did = 'did:plc:hh'")
                .fetch_one(&ctx.account_db)
                .await
                .unwrap();
        assert_eq!(handle.as_deref(), Some("hh.test"));
        assert_eq!(
            count_chain_rows(&ctx, "account.update_handle", Some("did:plc:hh")).await,
            0
        );
    }

    #[tokio::test]
    async fn test_apply_label_writes_chain_entry() {
        // Category B: record-subject. apply_label routes to a Record
        // subject built from req.uri/req.cid; the chain row's
        // subject_uri is set, subject_did is NULL.
        let ctx = create_test_context().await;
        let req = ApplyLabelRequest {
            uri: "at://did:plc:author/app.bsky.feed.post/abc".to_string(),
            cid: Some("bafyreitestcid".to_string()),
            val: "spam".to_string(),
            expires_days: None,
        };
        let _ = apply_label(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .expect("apply_label succeeds");
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_chain_entry \
             WHERE action = 'label.apply' AND subject_uri = ?",
        )
        .bind("at://did:plc:author/app.bsky.feed.post/abc")
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(count, 1, "apply_label must write one chain entry");
    }

    #[tokio::test]
    async fn test_create_invite_code_writes_chain_entry() {
        // Category E: server-level (None subject), action "invite.create".
        let ctx = create_test_context().await;
        let req = CreateInviteCodeRequest {
            uses: Some(1),
            expires_days: None,
            note: None,
            for_account: None,
        };
        let _ = create_invite_code(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .expect("invite create succeeds");
        assert_eq!(count_chain_rows(&ctx, "invite.create", None).await, 1);
    }

    #[tokio::test]
    async fn test_pause_sequencer_writes_chain_entry() {
        // Category F: server-level no-subject. action "sequencer.pause"
        // with None subject — chain row's subject_did/uri/cid are all NULL.
        let ctx = create_test_context().await;
        let _ = pause_sequencer(State(ctx.clone()), admin_test_auth())
            .await
            .expect("pause sequencer succeeds");
        assert_eq!(count_chain_rows(&ctx, "sequencer.pause", None).await, 1);
    }

    #[tokio::test]
    async fn test_update_account_email_writes_chain_entry() {
        // Category G: previously audit-blind. Now writes one chain row
        // with action "account.update_email" and the new email in the
        // rationale.
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:emailchain", "ec.test", Some("a@b.com"))
            .await;
        let req = UpdateAccountEmailRequest {
            account: None,
            did: Some("did:plc:emailchain".to_string()),
            email: "new@example.org".to_string(),
        };
        let status = update_account_email(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            count_chain_rows(&ctx, "account.update_email", Some("did:plc:emailchain"))
                .await,
            1
        );
    }

    /// v0.8 arc 3 (#184) — Gate 3 of the email `:`-reject (admin
    /// update-email). A `did:`-leading email is rejected with the
    /// charset-specific message before the existing `@`/length check
    /// (M-5 message uniformity), matching Gate 1 (`validate_email`) and
    /// Gate 2 (updateEmail handler).
    #[tokio::test]
    async fn update_account_email_rejects_colon_email() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:colontest", "ct.test", Some("a@b.com")).await;
        let req = UpdateAccountEmailRequest {
            account: None,
            did: Some("did:plc:colontest".to_string()),
            email: "did:foo@example.com".to_string(),
        };
        let err = update_account_email(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert_eq!(err.1, "Email address must not contain ':'");
    }

    // LB-1 / chainlink #128: pin the atomicity invariant for
    // update_account_email. The handler runs the email UPDATE and
    // the chain append in one transaction; if anything inside the
    // tx rolls back, both writes must roll back together. This
    // test exercises that contract directly via the in_tx
    // primitives the handler uses.
    #[tokio::test]
    async fn update_account_email_in_tx_rolls_back_atomically() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:atomictest", "at.test", Some("old@x.com"))
            .await;

        // Build an append-chain guard + tx + run the same _in_tx
        // calls the handler does, then deliberately rollback.
        // Neither row should land.
        {
            let _guard = audit_chain::AppendChainGuard::acquire().await;
            let mut tx = ctx.account_db.begin().await.unwrap();
            crate::account::AccountManager::update_email_in_tx(
                &mut tx,
                "did:plc:atomictest",
                "new@example.com",
            )
            .await
            .unwrap();
            audit_chain::insert_chain_entry(
                &mut tx,
                ctx.config.database.backend,
                AppendEntryParams {
                    source: "manual",
                    payload: None,
                    actor_did: "did:plc:admin",
                    action: "account.update_email",
                    subject: Some(&Subject::Repo {
                        did: "did:plc:atomictest".to_string(),
                    }),
                    rationale: "would be rolled back",
                    snapshot_id: None,
                    event_id: None,
                    cascade_subjects: &[],
                    cascade_snapshot_ids: &[],
                },
            )
            .await
            .unwrap();
            tx.rollback().await.unwrap();
        }

        // Email unchanged.
        let email = account_email(&ctx, "did:plc:atomictest").await;
        assert_eq!(
            email.as_deref(),
            Some("old@x.com"),
            "rolled-back tx must not land email update"
        );
        // No chain entry for this DID.
        let chain_count =
            count_chain_rows(&ctx, "account.update_email", Some("did:plc:atomictest"))
                .await;
        assert_eq!(
            chain_count, 0,
            "rolled-back tx must not land chain entry"
        );
    }

    #[tokio::test]
    async fn test_update_subject_status_writes_one_chain_entry_per_call() {
        // Category D consolidation: even when both takedown and
        // deactivated patches are supplied, the handler emits ONE chain
        // entry with action "subject.update_status", per §3.4
        // "one decision = one chain entry".
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:bothpatches", "bp.test", None).await;

        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoRef {
                did: "did:plc:bothpatches".to_string(),
            },
            takedown: Some(StatusAttr {
                applied: true,
                ref_field: Some("ticket-X".to_string()),
            }),
            deactivated: Some(StatusAttr {
                applied: true,
                ref_field: None,
            }),
            legacy_record_uri_used: false,
        };
        let _ = update_subject_status(State(ctx.clone()), admin_test_auth(), crate::api::extractors::AuroraJson(req))
            .await
            .expect("update_subject_status succeeds");

        assert_eq!(
            count_chain_rows(
                &ctx,
                "subject.update_status",
                Some("did:plc:bothpatches"),
            )
            .await,
            1,
            "two patches in one call must produce exactly one chain row"
        );
    }

    /// Helper: read the current email column for a DID.
    async fn account_email(ctx: &AppContext, did: &str) -> Option<String> {
        use sqlx::Row;
        sqlx::query("SELECT email FROM account WHERE did = ?")
            .bind(did)
            .fetch_one(&ctx.account_db)
            .await
            .unwrap()
            .get(0)
    }

    #[tokio::test]
    async fn test_update_account_email_with_account_handle_form() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:emailtest", "email.test", Some("old@x.com")).await;

        let req = UpdateAccountEmailRequest {
            account: Some("email.test".to_string()),
            did: None,
            email: "new@example.com".to_string(),
        };
        let status = update_account_email(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            account_email(&ctx, "did:plc:emailtest").await.as_deref(),
            Some("new@example.com")
        );
    }

    #[tokio::test]
    async fn test_update_account_email_with_legacy_did() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:legacyemail", "legacyemail.test", None).await;

        let req = UpdateAccountEmailRequest {
            account: None,
            did: Some("did:plc:legacyemail".to_string()),
            email: "back@compat.com".to_string(),
        };
        let status = update_account_email(State(ctx.clone()), admin_test_auth(), Json(req))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            account_email(&ctx, "did:plc:legacyemail").await.as_deref(),
            Some("back@compat.com")
        );
    }

    // ---- Phase 1.8: sendEmail required-field flips ----------------------

    #[test]
    fn test_send_email_request_subject_is_optional() {
        // Spec says `subject` is optional. Aurora used to require it; verify
        // the deserializer now accepts a payload that omits subject.
        let json = serde_json::json!({
            "recipientDid": "did:plc:r",
            "content": "hello",
            "senderDid": "did:plc:s",
        });
        let req: SendEmailRequest = serde_json::from_value(json).unwrap();
        assert!(req.subject.is_none());
        assert_eq!(req.recipient_did, "did:plc:r");
        assert_eq!(req.sender_did.as_deref(), Some("did:plc:s"));
    }

    #[test]
    fn test_send_email_request_sender_did_remains_optional_aurora_extension() {
        // Spec says `senderDid` is required, but Aurora retains the
        // permissive extension allowing omission (defaults to authenticated
        // admin DID at handler time).
        let json = serde_json::json!({
            "recipientDid": "did:plc:r",
            "content": "hello",
        });
        let req: SendEmailRequest = serde_json::from_value(json).unwrap();
        assert!(req.subject.is_none());
        assert!(req.sender_did.is_none());
    }

    #[tokio::test]
    async fn test_send_email_subject_omitted_reaches_handler() {
        // With no subject and a missing recipient, we hit the account-lookup
        // error path. Reaching that path proves the request deserialized
        // (subject correctly optional) and the handler ran past the entry.
        let ctx = create_test_context().await;
        let req = SendEmailRequest {
            recipient_did: "did:plc:doesnotexist".to_string(),
            content: "ping".to_string(),
            subject: None,
            sender_did: Some("did:plc:admin".to_string()),
            comment: None,
        };
        let err = send_email(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("missing recipient should 404 — proves we reached the handler");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_send_email_sender_did_omitted_defaults_to_admin() {
        // Same shape as above with senderDid omitted — should also reach
        // the handler (account-not-found 404), proving the Aurora-permissive
        // extension still deserializes.
        let ctx = create_test_context().await;
        let req = SendEmailRequest {
            recipient_did: "did:plc:doesnotexist".to_string(),
            content: "ping".to_string(),
            subject: Some("urgent".to_string()),
            sender_did: None,
            comment: None,
        };
        let err = send_email(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("missing recipient should 404");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_send_email_subject_provided_back_compat() {
        // Existing callers still passing a subject get the same behavior.
        let ctx = create_test_context().await;
        let req = SendEmailRequest {
            recipient_did: "did:plc:doesnotexist".to_string(),
            content: "back compat".to_string(),
            subject: Some("Important".to_string()),
            sender_did: Some("did:plc:s".to_string()),
            comment: Some("ticket-1234".to_string()),
        };
        let err = send_email(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("missing recipient should 404");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    // ---- Phase 1.6: updateSubjectStatus polymorphism ---------------------

    #[test]
    fn test_subject_union_repo_ref_round_trip() {
        let json = serde_json::json!({
            "$type": "com.atproto.admin.defs#repoRef",
            "did": "did:plc:abc"
        });
        let parsed: SubjectUnion = serde_json::from_value(json.clone()).unwrap();
        match &parsed {
            SubjectUnion::RepoRef { did } => assert_eq!(did, "did:plc:abc"),
            _ => panic!("expected RepoRef"),
        }
        let serialized = serde_json::to_value(&parsed).unwrap();
        assert_eq!(serialized, json);
    }

    #[test]
    fn test_subject_union_strong_ref_round_trip() {
        let json = serde_json::json!({
            "$type": "com.atproto.repo.strongRef",
            "uri": "at://did:plc:abc/app.bsky.feed.post/xyz",
            "cid": "bafyabc"
        });
        let parsed: SubjectUnion = serde_json::from_value(json.clone()).unwrap();
        match &parsed {
            SubjectUnion::StrongRef { uri, cid } => {
                assert_eq!(uri, "at://did:plc:abc/app.bsky.feed.post/xyz");
                assert_eq!(cid, "bafyabc");
            }
            _ => panic!("expected StrongRef"),
        }
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);
    }

    #[test]
    fn test_subject_union_repo_blob_ref_round_trip() {
        // §6.4.0.5: wire field is `record_uri` (snake_case), matching
        // Subject::Blob in src/admin/defs.rs.
        let json = serde_json::json!({
            "$type": "com.atproto.admin.defs#repoBlobRef",
            "did": "did:plc:abc",
            "cid": "bafyblob",
            "record_uri": "at://did:plc:abc/app.bsky.feed.post/xyz"
        });
        let parsed: SubjectUnion = serde_json::from_value(json.clone()).unwrap();
        match &parsed {
            SubjectUnion::RepoBlobRef {
                did,
                cid,
                record_uri,
            } => {
                assert_eq!(did, "did:plc:abc");
                assert_eq!(cid, "bafyblob");
                assert_eq!(record_uri.as_deref(), Some("at://did:plc:abc/app.bsky.feed.post/xyz"));
            }
            _ => panic!("expected RepoBlobRef"),
        }
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);
    }

    #[test]
    fn test_subject_union_rejects_missing_type_discriminator() {
        let json = serde_json::json!({"did": "did:plc:abc"});
        let result: Result<SubjectUnion, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }

    /// Arc 2 Step 0.5 (§6.4.0.5) — within-surface byte-equality
    /// regression guard for the canonical Aurora Subject contract.
    /// `Subject` (in `crate::admin::defs`) is the type Aurora-namespace
    /// handlers serialize on getAuditTrail / subscribeModEvents /
    /// batch-label / etc.; `SubjectUnion` (private to admin.rs) is the
    /// parsing dual on updateSubjectStatus and friends. The two MUST
    /// produce byte-identical canonical-JSON output for every shared
    /// variant — otherwise round-tripping a payload through
    /// updateSubjectStatus → getAuditTrail would surface a re-keyed
    /// shape and break clients.
    ///
    /// The byte-drift between these types on `Blob`/`RepoBlobRef` —
    /// `record_uri` vs `recordUri` — was caught by Arc 2 Step 0 recon
    /// and fixed in this step by dropping `rename_all = "camelCase"`
    /// from `RepoBlobRef`. This test pins all three shared variants
    /// (Repo, Record, Blob) byte-equal across the two types so the
    /// drift cannot regress.
    ///
    /// Uses the formal canonical-JSON helper at
    /// `tests/common/canonical_json.rs`, included via `#[path]` at
    /// the bottom of this test module so unit tests can reach it
    /// across the unit-vs-integration boundary.
    #[test]
    fn subject_blob_and_subject_union_repoblobref_serialize_byte_equal() {
        use crate::admin::defs::Subject;

        // ---- Repo / RepoRef ----
        let s_repo = Subject::Repo {
            did: "did:plc:test1234567890abcdef".to_string(),
        };
        let u_repo = SubjectUnion::RepoRef {
            did: "did:plc:test1234567890abcdef".to_string(),
        };
        assert_eq!(
            canonical_json(&s_repo),
            canonical_json(&u_repo),
            "Repo / RepoRef must serialize byte-equal"
        );

        // ---- Record / StrongRef ----
        let s_record = Subject::Record {
            uri: "at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc".to_string(),
            cid: "bafyreidemo123".to_string(),
        };
        let u_strong = SubjectUnion::StrongRef {
            uri: "at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc".to_string(),
            cid: "bafyreidemo123".to_string(),
        };
        assert_eq!(
            canonical_json(&s_record),
            canonical_json(&u_strong),
            "Record / StrongRef must serialize byte-equal"
        );

        // ---- Blob / RepoBlobRef (the case Step 0.5 reconciled) ----
        let s_blob = Subject::Blob {
            did: "did:plc:test1234567890abcdef".to_string(),
            cid: "bafyreidemoblob456".to_string(),
            record_uri: Some(
                "at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc".to_string(),
            ),
        };
        let u_blob = SubjectUnion::RepoBlobRef {
            did: "did:plc:test1234567890abcdef".to_string(),
            cid: "bafyreidemoblob456".to_string(),
            record_uri: Some(
                "at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc".to_string(),
            ),
        };
        let s_blob_json = canonical_json(&s_blob);
        let u_blob_json = canonical_json(&u_blob);
        assert_eq!(
            s_blob_json, u_blob_json,
            "Blob / RepoBlobRef must serialize byte-equal — \
             record_uri rename direction is the load-bearing fix"
        );
        // Pin the absolute wire shape so a later refactor that
        // re-introduces `rename_all = \"camelCase\"` (or otherwise
        // re-keys to `recordUri`) is caught at this assertion, not
        // just at the cross-type comparison above.
        assert_eq!(
            s_blob_json,
            r#"{"$type":"com.atproto.admin.defs#repoBlobRef","cid":"bafyreidemoblob456","did":"did:plc:test1234567890abcdef","record_uri":"at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc"}"#,
            "wire shape must include snake_case `record_uri`"
        );

        // Blob with record_uri = None (skip_serializing_if path).
        let s_blob_none = Subject::Blob {
            did: "did:plc:test1234567890abcdef".to_string(),
            cid: "bafyreidemoblob456".to_string(),
            record_uri: None,
        };
        let u_blob_none = SubjectUnion::RepoBlobRef {
            did: "did:plc:test1234567890abcdef".to_string(),
            cid: "bafyreidemoblob456".to_string(),
            record_uri: None,
        };
        assert_eq!(
            canonical_json(&s_blob_none),
            canonical_json(&u_blob_none),
            "Blob / RepoBlobRef byte-equal when record_uri omitted"
        );
    }

    use super::canonical_json_helper::canonical_json;

    #[test]
    fn test_status_attr_round_trip() {
        let json = serde_json::json!({"applied": true, "ref": "ticket-1234"});
        let parsed: StatusAttr = serde_json::from_value(json).unwrap();
        assert!(parsed.applied);
        assert_eq!(parsed.ref_field.as_deref(), Some("ticket-1234"));

        let json_no_ref = serde_json::json!({"applied": false});
        let parsed: StatusAttr = serde_json::from_value(json_no_ref).unwrap();
        assert!(!parsed.applied);
        assert!(parsed.ref_field.is_none());
    }

    /// Helper: read takedown_ref off the actor table.
    async fn account_takedown_ref(ctx: &AppContext, did: &str) -> Option<String> {
        use sqlx::Row;
        sqlx::query("SELECT takedown_ref FROM actor WHERE did = ?")
            .bind(did)
            .fetch_one(&ctx.account_db)
            .await
            .unwrap()
            .get(0)
    }

    /// Helper: read deactivated_at off the actor table.
    async fn account_deactivated(ctx: &AppContext, did: &str) -> bool {
        use sqlx::Row;
        let row: Option<String> =
            sqlx::query("SELECT deactivated_at FROM actor WHERE did = ?")
                .bind(did)
                .fetch_one(&ctx.account_db)
                .await
                .unwrap()
                .get(0);
        row.is_some()
    }

    #[tokio::test]
    async fn test_update_subject_status_takedown_account() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:victim", "victim.test", None).await;

        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoRef {
                did: "did:plc:victim".to_string(),
            },
            takedown: Some(StatusAttr {
                applied: true,
                ref_field: Some("ticket-99".to_string()),
            }),
            deactivated: None,
            legacy_record_uri_used: false,
        };
        let resp = update_subject_status(State(ctx.clone()), admin_test_auth(), crate::api::extractors::AuroraJson(req))
            .await
            .unwrap()
            .0;
        // Echoes back the subject and the post-patch takedown state.
        match resp.subject {
            SubjectUnion::RepoRef { did } => assert_eq!(did, "did:plc:victim"),
            _ => panic!("expected RepoRef"),
        }
        let td = resp.takedown.expect("takedown should be echoed");
        assert!(td.applied);
        assert_eq!(td.ref_field.as_deref(), Some("ticket-99"));
        assert_eq!(
            account_takedown_ref(&ctx, "did:plc:victim").await.as_deref(),
            Some("ticket-99")
        );
    }

    #[tokio::test]
    async fn test_update_subject_status_restores_account_via_applied_false() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:revived", "revived.test", None).await;
        // Pre-takedown the account.
        ctx.account_manager
            .takedown_account("did:plc:revived", "ticket-old")
            .await
            .unwrap();
        assert!(account_takedown_ref(&ctx, "did:plc:revived").await.is_some());

        // Patch with applied=false -> implicit restore per spec.
        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoRef {
                did: "did:plc:revived".to_string(),
            },
            takedown: Some(StatusAttr {
                applied: false,
                ref_field: None,
            }),
            deactivated: None,
            legacy_record_uri_used: false,
        };
        let resp = update_subject_status(State(ctx.clone()), admin_test_auth(), crate::api::extractors::AuroraJson(req))
            .await
            .unwrap()
            .0;
        let td = resp.takedown.expect("takedown should be echoed");
        assert!(!td.applied);
        assert!(td.ref_field.is_none());
        assert!(account_takedown_ref(&ctx, "did:plc:revived").await.is_none());
    }

    /// chainlink #179: the reverse-takedown path of updateSubjectStatus
    /// (takedown.applied = false) must emit an `#account` event so downstream
    /// subscribers see the restore, symmetrizing with the takedown-apply emit.
    /// Previously the §8.1.2 v0.5 deferral left no emit on restore.
    #[tokio::test]
    async fn test_update_subject_status_reverse_takedown_emits_account_event() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:revrestore", "revrestore.test", None).await;

        // Take the account down via updateSubjectStatus (emits one #account).
        let takedown_req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoRef {
                did: "did:plc:revrestore".to_string(),
            },
            takedown: Some(StatusAttr {
                applied: true,
                ref_field: Some("t-1".to_string()),
            }),
            deactivated: None,
            legacy_record_uri_used: false,
        };
        let _ = update_subject_status(
            State(ctx.clone()),
            admin_test_auth(),
            crate::api::extractors::AuroraJson(takedown_req),
        )
        .await
        .unwrap();

        let before: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM repo_seq WHERE event_type = 'account' AND invalidated = 0",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();

        // Reverse-takedown via updateSubjectStatus (applied = false).
        let restore_req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoRef {
                did: "did:plc:revrestore".to_string(),
            },
            takedown: Some(StatusAttr {
                applied: false,
                ref_field: None,
            }),
            deactivated: None,
            legacy_record_uri_used: false,
        };
        let _ = update_subject_status(
            State(ctx.clone()),
            admin_test_auth(),
            crate::api::extractors::AuroraJson(restore_req),
        )
        .await
        .unwrap();

        let after: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM repo_seq WHERE event_type = 'account' AND invalidated = 0",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();

        assert_eq!(
            after,
            before + 1,
            "reverse-takedown must emit exactly one #account event (chainlink #179)"
        );
    }

    #[tokio::test]
    async fn test_update_subject_status_deactivates_account() {
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:dorm", "dorm.test", None).await;

        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoRef {
                did: "did:plc:dorm".to_string(),
            },
            takedown: None,
            deactivated: Some(StatusAttr {
                applied: true,
                ref_field: None,
            }),
            legacy_record_uri_used: false,
        };
        let _ = update_subject_status(State(ctx.clone()), admin_test_auth(), crate::api::extractors::AuroraJson(req))
            .await
            .unwrap();
        assert!(account_deactivated(&ctx, "did:plc:dorm").await);
    }

    #[tokio::test]
    async fn test_update_subject_status_record_returns_501() {
        let ctx = create_test_context().await;
        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::StrongRef {
                uri: "at://did:plc:foo/app.bsky.feed.post/xyz".to_string(),
                cid: "bafyabc".to_string(),
            },
            takedown: Some(StatusAttr {
                applied: true,
                ref_field: None,
            }),
            deactivated: None,
            legacy_record_uri_used: false,
        };
        let resp = update_subject_status(State(ctx), admin_test_auth(), crate::api::extractors::AuroraJson(req))
            .await
            .expect_err("strongRef should return 501 until record-level setter exists");
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert!(body.contains("Record-level"));
    }

    /// Seed a blob row directly into `blob_metadata` so the existence
    /// check in `apply_blob_status` (`BlobStore::get_metadata`) finds it.
    /// Bypasses the upload path.
    async fn seed_test_blob(ctx: &AppContext, cid: &str, did: &str) {
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at)
             VALUES (?, 'application/octet-stream', 0, ?, ?)",
        )
        .bind(cid)
        .bind(did)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&ctx.account_db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_update_subject_status_takedown_blob() {
        let ctx = create_test_context().await;
        seed_test_blob(&ctx, "bafyblob01", "did:plc:owner").await;

        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoBlobRef {
                did: "did:plc:owner".to_string(),
                cid: "bafyblob01".to_string(),
                record_uri: None,
            },
            takedown: Some(StatusAttr {
                applied: true,
                ref_field: Some("legal-1".to_string()),
            }),
            deactivated: None,
            legacy_record_uri_used: false,
        };
        let resp = update_subject_status(State(ctx.clone()), admin_test_auth(), crate::api::extractors::AuroraJson(req))
            .await
            .unwrap()
            .0;
        let td = resp.takedown.expect("takedown should be echoed");
        assert!(td.applied);

        // Verify via the quarantine system directly.
        use crate::blob_store::quarantine::BlobQuarantine;
        let quarantine = BlobQuarantine::new(ctx.account_db.clone());
        assert!(quarantine.is_quarantined("bafyblob01").await.unwrap());
    }

    /// Helper: read body bytes and parse JSON for a Response error.
    async fn read_xrpc_error(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).expect("error body must be JSON");
        (status, json)
    }

    #[tokio::test]
    async fn test_update_subject_status_blob_deactivated_returns_400() {
        let ctx = create_test_context().await;
        seed_test_blob(&ctx, "bafyblob02", "did:plc:owner").await;

        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoBlobRef {
                did: "did:plc:owner".to_string(),
                cid: "bafyblob02".to_string(),
                record_uri: None,
            },
            takedown: None,
            deactivated: Some(StatusAttr {
                applied: true,
                ref_field: None,
            }),
            legacy_record_uri_used: false,
        };
        let resp = update_subject_status(State(ctx), admin_test_auth(), crate::api::extractors::AuroraJson(req))
            .await
            .expect_err("blob + deactivated should reject");
        let (status, body) = read_xrpc_error(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "InvalidRequest");
        assert!(body["message"]
            .as_str()
            .unwrap()
            .contains("not applicable to blob"));
    }

    #[tokio::test]
    async fn test_update_subject_status_record_deactivated_returns_400() {
        let ctx = create_test_context().await;
        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::StrongRef {
                uri: "at://did:plc:foo/app.bsky.feed.post/xyz".to_string(),
                cid: "bafyabc".to_string(),
            },
            takedown: None,
            deactivated: Some(StatusAttr {
                applied: true,
                ref_field: None,
            }),
            legacy_record_uri_used: false,
        };
        let resp = update_subject_status(State(ctx), admin_test_auth(), crate::api::extractors::AuroraJson(req))
            .await
            .expect_err("record + deactivated should reject");
        let (status, body) = read_xrpc_error(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "InvalidRequest");
        assert!(body["message"]
            .as_str()
            .unwrap()
            .contains("not applicable to record"));
    }

    #[tokio::test]
    async fn test_update_subject_status_blob_not_found_returns_404() {
        let ctx = create_test_context().await;
        // Do not seed; the blob doesn't exist.

        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoBlobRef {
                did: "did:plc:owner".to_string(),
                cid: "bafynonexistent".to_string(),
                record_uri: None,
            },
            takedown: Some(StatusAttr {
                applied: true,
                ref_field: None,
            }),
            deactivated: None,
            legacy_record_uri_used: false,
        };
        let resp = update_subject_status(State(ctx), admin_test_auth(), crate::api::extractors::AuroraJson(req))
            .await
            .expect_err("non-existent blob should 404");
        let (status, body) = read_xrpc_error(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "BlobNotFound");
        assert!(body["message"]
            .as_str()
            .unwrap()
            .contains("bafynonexistent"));
    }

    #[tokio::test]
    async fn test_update_subject_status_restore_non_quarantined_blob_idempotent() {
        let ctx = create_test_context().await;
        seed_test_blob(&ctx, "bafyblob03", "did:plc:owner").await;
        // Blob exists but is NOT quarantined; restore should succeed
        // (idempotent — desired post-state already obtains).

        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoBlobRef {
                did: "did:plc:owner".to_string(),
                cid: "bafyblob03".to_string(),
                record_uri: None,
            },
            takedown: Some(StatusAttr {
                applied: false,
                ref_field: None,
            }),
            deactivated: None,
            legacy_record_uri_used: false,
        };
        let resp = update_subject_status(State(ctx), admin_test_auth(), crate::api::extractors::AuroraJson(req))
            .await
            .unwrap()
            .0;
        let td = resp.takedown.expect("takedown should be echoed");
        assert!(!td.applied);
    }

    #[tokio::test]
    async fn test_update_subject_status_takedown_already_quarantined_idempotent() {
        let ctx = create_test_context().await;
        seed_test_blob(&ctx, "bafyblob04", "did:plc:owner").await;
        // Pre-quarantine.
        use crate::blob_store::quarantine::{BlobQuarantine, QuarantineReason};
        let quarantine = BlobQuarantine::new(ctx.account_db.clone());
        quarantine
            .quarantine_blob(
                "bafyblob04",
                QuarantineReason::Other,
                Some("first-takedown"),
                "did:plc:admin1",
                None,
            )
            .await
            .unwrap();

        // Repeat takedown — should succeed despite already-quarantined.
        let req = UpdateSubjectStatusRequest {
            subject: SubjectUnion::RepoBlobRef {
                did: "did:plc:owner".to_string(),
                cid: "bafyblob04".to_string(),
                record_uri: None,
            },
            takedown: Some(StatusAttr {
                applied: true,
                ref_field: Some("second-takedown".to_string()),
            }),
            deactivated: None,
            legacy_record_uri_used: false,
        };
        let resp = update_subject_status(State(ctx), admin_test_auth(), crate::api::extractors::AuroraJson(req))
            .await
            .unwrap()
            .0;
        let td = resp.takedown.expect("takedown should be echoed");
        assert!(td.applied);
    }

    #[tokio::test]
    async fn test_update_subject_status_rejects_malformed_subject() {
        // Subject without $type discriminator → serde rejects deserialization.
        let json = serde_json::json!({
            "subject": {"did": "did:plc:abc"},
            "takedown": {"applied": true}
        });
        let result: Result<UpdateSubjectStatusRequest, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_update_subject_status_rejects_status_attr_without_applied() {
        let json = serde_json::json!({
            "subject": {
                "$type": "com.atproto.admin.defs#repoRef",
                "did": "did:plc:abc"
            },
            "takedown": {"ref": "missing-applied-field"}
        });
        let result: Result<UpdateSubjectStatusRequest, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_update_subject_status_response_shape_matches_lexicon() {
        // Lexicon output: subject (required) + takedown (optional). No
        // deactivated. Verify the serialised JSON matches.
        let resp = UpdateSubjectStatusResponse {
            subject: SubjectUnion::RepoRef {
                did: "did:plc:abc".to_string(),
            },
            takedown: Some(StatusAttr {
                applied: true,
                ref_field: Some("ticket-1".to_string()),
            }),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["subject"]["$type"], "com.atproto.admin.defs#repoRef");
        assert_eq!(json["subject"]["did"], "did:plc:abc");
        assert_eq!(json["takedown"]["applied"], true);
        assert_eq!(json["takedown"]["ref"], "ticket-1");
        // No deactivated field per the lexicon's output schema.
        assert!(json.get("deactivated").is_none());

        // takedown can be omitted entirely when None.
        let resp_no_td = UpdateSubjectStatusResponse {
            subject: SubjectUnion::RepoRef {
                did: "did:plc:xyz".to_string(),
            },
            takedown: None,
        };
        let json = serde_json::to_value(&resp_no_td).unwrap();
        assert!(json.get("takedown").is_none());
    }

    // ---- Phase 1.10: invite-code pagination -------------------------------

    /// Helper that creates `n` invite codes with a small delay between each
    /// so they have distinct `created_at` timestamps. Returns codes in
    /// creation order (oldest first).
    async fn seed_invite_codes(ctx: &AppContext, n: usize) -> Vec<crate::admin::InviteCode> {
        let mut codes = Vec::with_capacity(n);
        for i in 0..n {
            let c = ctx
                .invite_manager
                .create_invite("did:plc:creator", 5, None, Some(format!("seed {i}")), None)
                .await
                .unwrap();
            // #258: create_invite stamps created_at with Utc::now(); under a
            // coarse OS clock (e.g. WSL2 ~15ms) plus parallel-test load, the
            // rapid inserts can collide same-millisecond, making the `recent`
            // (created_at DESC) ordering + cursor boundary ambiguous. Stamp a
            // deterministic, strictly-increasing created_at (one minute apart,
            // same RFC3339 form Utc::now().to_rfc3339() produces) so the order
            // is timing-independent: codes[i] is always older than codes[i+1].
            let created_at = format!("2020-01-01T00:{:02}:00+00:00", i);
            sqlx::query("UPDATE invite_code SET created_at = $1 WHERE code = $2")
                .bind(&created_at)
                .bind(&c.code)
                .execute(&ctx.account_db)
                .await
                .unwrap();
            codes.push(c);
        }
        codes
    }

    #[tokio::test]
    async fn test_get_invite_codes_sort_recent_returns_newest_first() {
        let ctx = create_test_context().await;
        let seeded = seed_invite_codes(&ctx, 3).await;

        let resp = get_invite_codes(
            State(ctx),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: Some("recent".to_string()),
                limit: None,
                cursor: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.codes.len(), 3);
        // Newest-first: reverse of seed order.
        assert_eq!(resp.codes[0].code, seeded[2].code);
        assert_eq!(resp.codes[1].code, seeded[1].code);
        assert_eq!(resp.codes[2].code, seeded[0].code);
        assert!(resp.cursor.is_none()); // 3 codes, no more pages
    }

    #[tokio::test]
    async fn test_get_invite_codes_sort_usage_orders_by_use_count() {
        let ctx = create_test_context().await;
        let seeded = seed_invite_codes(&ctx, 3).await;
        // Record uses: seeded[0] used twice, seeded[1] used once, seeded[2] zero.
        for (idx, count) in [2u32, 1, 0].iter().enumerate() {
            for _ in 0..*count {
                sqlx::query(
                    "INSERT INTO invite_code_use (code, used_by, used_at) VALUES (?, ?, ?)",
                )
                .bind(&seeded[idx].code)
                .bind("did:plc:user")
                .bind(chrono::Utc::now().to_rfc3339())
                .execute(&ctx.account_db)
                .await
                .unwrap();
            }
        }

        let resp = get_invite_codes(
            State(ctx),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: Some("usage".to_string()),
                limit: None,
                cursor: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.codes.len(), 3);
        // Most-used first.
        assert_eq!(resp.codes[0].code, seeded[0].code); // 2 uses
        assert_eq!(resp.codes[1].code, seeded[1].code); // 1 use
        assert_eq!(resp.codes[2].code, seeded[2].code); // 0 uses
    }

    #[tokio::test]
    async fn test_get_invite_codes_paginates_with_cursor_recent() {
        let ctx = create_test_context().await;
        let seeded = seed_invite_codes(&ctx, 5).await;

        // Page 1 of 2 with a cursor.
        let page1 = get_invite_codes(
            State(ctx.clone()),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: Some("recent".to_string()),
                limit: Some(2),
                cursor: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page1.codes.len(), 2);
        assert_eq!(page1.codes[0].code, seeded[4].code); // newest
        assert_eq!(page1.codes[1].code, seeded[3].code);
        let cursor1 = page1.cursor.expect("more results, cursor expected");

        // Page 2.
        let page2 = get_invite_codes(
            State(ctx.clone()),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: Some("recent".to_string()),
                limit: Some(2),
                cursor: Some(cursor1),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page2.codes.len(), 2);
        assert_eq!(page2.codes[0].code, seeded[2].code);
        assert_eq!(page2.codes[1].code, seeded[1].code);
        let cursor2 = page2.cursor.expect("one more page expected");

        // Page 3 finishes the set.
        let page3 = get_invite_codes(
            State(ctx),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: Some("recent".to_string()),
                limit: Some(2),
                cursor: Some(cursor2),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page3.codes.len(), 1);
        assert_eq!(page3.codes[0].code, seeded[0].code); // oldest
        assert!(page3.cursor.is_none());
    }

    #[tokio::test]
    async fn test_get_invite_codes_rejects_out_of_range_limit() {
        let ctx = create_test_context().await;
        let result = get_invite_codes(
            State(ctx.clone()),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: None,
                limit: Some(0),
                cursor: None,
            }),
        )
        .await;
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("limit=0 should be rejected"),
        };
        assert_eq!(err.0, StatusCode::BAD_REQUEST);

        let result = get_invite_codes(
            State(ctx),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: None,
                limit: Some(501),
                cursor: None,
            }),
        )
        .await;
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("limit=501 should be rejected"),
        };
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_get_invite_codes_rejects_invalid_sort() {
        let ctx = create_test_context().await;
        let result = get_invite_codes(
            State(ctx),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: Some("alphabetical".to_string()),
                limit: None,
                cursor: None,
            }),
        )
        .await;
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("invalid sort should be rejected"),
        };
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("recent") && err.1.contains("usage"));
    }

    #[tokio::test]
    async fn test_get_invite_codes_rejects_cursor_with_mismatched_sort() {
        let ctx = create_test_context().await;
        let _ = seed_invite_codes(&ctx, 3).await;

        // Get a cursor for sort=recent.
        let page1 = get_invite_codes(
            State(ctx.clone()),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: Some("recent".to_string()),
                limit: Some(1),
                cursor: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let recent_cursor = page1.cursor.unwrap();

        // Replay the same cursor on sort=usage; should be rejected.
        let result = get_invite_codes(
            State(ctx),
            admin_test_auth(),
            Query(GetInviteCodesQuery {
                sort: Some("usage".to_string()),
                limit: Some(1),
                cursor: Some(recent_cursor),
            }),
        )
        .await;
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("mismatched sort+cursor should be rejected"),
        };
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_list_invite_codes_paginates_too() {
        let ctx = create_test_context().await;
        let _ = seed_invite_codes(&ctx, 3).await;

        let page1 = list_invite_codes(
            State(ctx.clone()),
            admin_test_auth(),
            Query(ListInviteCodesQuery {
                sort: None,
                limit: Some(2),
                cursor: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page1.codes.len(), 2);
        let cursor = page1.cursor.expect("more results expected");

        let page2 = list_invite_codes(
            State(ctx),
            admin_test_auth(),
            Query(ListInviteCodesQuery {
                sort: None,
                limit: Some(2),
                cursor: Some(cursor),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page2.codes.len(), 1);
        assert!(page2.cursor.is_none());
    }

    // ---- Phase 1.9: getAccountInfos param encoding + handle field --------

    #[tokio::test]
    async fn test_get_account_infos_repeated_query_params_returns_both() {
        use axum_extra::extract::Query as ExtraQuery;

        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:one", "one.test", Some("one@x")).await;
        seed_test_account(&ctx, "did:plc:two", "two.test", Some("two@x")).await;

        // Simulate axum-extra's parsing of `?dids=did:plc:one&dids=did:plc:two`.
        let query = GetAccountInfosQuery {
            dids: vec!["did:plc:one".to_string(), "did:plc:two".to_string()],
        };
        let resp = get_account_infos(State(ctx), admin_test_auth(), ExtraQuery(query))
            .await
            .unwrap()
            .0;
        assert_eq!(resp.infos.len(), 2);
        // Order matches input order; handle is now required (String, not Option).
        assert_eq!(resp.infos[0].did, "did:plc:one");
        assert_eq!(resp.infos[0].handle, "one.test");
        assert_eq!(resp.infos[1].did, "did:plc:two");
        assert_eq!(resp.infos[1].handle, "two.test");
    }

    #[tokio::test]
    async fn test_get_account_infos_silently_skips_missing_dids() {
        use axum_extra::extract::Query as ExtraQuery;

        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:exists", "exists.test", Some("e@x")).await;

        let query = GetAccountInfosQuery {
            dids: vec![
                "did:plc:exists".to_string(),
                "did:plc:doesnotexist".to_string(),
                "not-a-did-at-all".to_string(),
            ],
        };
        let resp = get_account_infos(State(ctx), admin_test_auth(), ExtraQuery(query))
            .await
            .unwrap()
            .0;
        // Existing skip-on-error behavior preserved: only the present account
        // appears, and the malformed entry is filtered before lookup.
        assert_eq!(resp.infos.len(), 1);
        assert_eq!(resp.infos[0].did, "did:plc:exists");
    }

    #[tokio::test]
    async fn test_get_account_infos_empty_array_400() {
        use axum_extra::extract::Query as ExtraQuery;

        let ctx = create_test_context().await;
        let query = GetAccountInfosQuery { dids: vec![] };
        let result = get_account_infos(State(ctx), admin_test_auth(), ExtraQuery(query)).await;
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("empty dids array should return 400"),
        };
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_account_info_handle_is_required_in_response_shape() {
        // Verify the serialized accountView has handle as a string, not as
        // null or missing. Spec marks handle as required.
        let info = AccountInfo {
            did: "did:plc:foo".to_string(),
            handle: "foo.test".to_string(),
            email: None,
            indexed_at: "2026-01-01T00:00:00Z".to_string(),
            email_confirmed_at: None,
            invited_by: None,
            invites: vec![],
            invites_disabled: false,
            invite_note: None,
            deactivated_at: None,
            threat_signatures: vec![],
        };
        let json = serde_json::to_value(&info).unwrap();
        // Must be a string, not null and not missing.
        assert!(json.get("handle").unwrap().is_string());
        assert_eq!(json["handle"], "foo.test");
    }

    #[tokio::test]
    async fn test_get_account_info_singular_returns_required_handle() {
        // The shared AccountInfo struct propagates to the singular endpoint
        // too — handle is required there as well.
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:single", "single.test", Some("s@x")).await;

        let resp = get_account_info(
            State(ctx),
            admin_test_auth(),
            Query(GetAccountInfoQuery {
                did: "did:plc:single".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.handle, "single.test");
    }

    #[tokio::test]
    async fn test_search_accounts_returns_required_handle() {
        // Same propagation check for the searchAccounts endpoint.
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:srch", "srch.test", Some("s@x")).await;

        let resp = search_accounts(
            State(ctx),
            admin_test_auth(),
            Query(SearchAccountsQuery {
                q: None,
                email: Some("s@x".to_string()),
                cursor: None,
                limit: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(resp.accounts.len(), 1);
        assert_eq!(resp.accounts[0].handle, "srch.test");
    }

    #[tokio::test]
    async fn test_send_email_account_without_email_400() {
        // Seed a recipient with no email; verify the handler reaches the
        // mailer step and rejects with 400 once it discovers the account
        // has no address. Confirms subject defaulting doesn't blow up in
        // the path before the email-presence check.
        let ctx = create_test_context().await;
        seed_test_account(&ctx, "did:plc:noemail", "noemail.test", None).await;

        let req = SendEmailRequest {
            recipient_did: "did:plc:noemail".to_string(),
            content: "hi".to_string(),
            subject: None, // exercises DEFAULT_EMPTY_SUBJECT path
            sender_did: Some("did:plc:s".to_string()),
            comment: None,
        };
        let err = send_email(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("account without email should 400");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("no email"));
    }

    #[tokio::test]
    async fn test_update_account_email_rejects_both_fields() {
        let ctx = create_test_context().await;
        let req = UpdateAccountEmailRequest {
            account: Some("did:plc:x".to_string()),
            did: Some("did:plc:x".to_string()),
            email: "x@y.com".to_string(),
        };
        let err = update_account_email(State(ctx), admin_test_auth(), Json(req))
            .await
            .expect_err("both fields should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_search_accounts_paginates_with_cursor() {
        let ctx = create_test_context().await;
        // Three accounts, ordered did:plc:a < did:plc:b < did:plc:c.
        seed_test_account(&ctx, "did:plc:a", "a.test", Some("a@x")).await;
        seed_test_account(&ctx, "did:plc:b", "b.test", Some("b@x")).await;
        seed_test_account(&ctx, "did:plc:c", "c.test", Some("c@x")).await;

        // Page size 2 → first page returns a, b with cursor = "did:plc:b".
        let page1 = search_accounts(
            State(ctx.clone()),
            admin_test_auth(),
            Query(SearchAccountsQuery {
                q: None,
                email: None,
                cursor: None,
                limit: Some(2),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page1.accounts.len(), 2);
        assert_eq!(page1.accounts[0].did, "did:plc:a");
        assert_eq!(page1.accounts[1].did, "did:plc:b");
        assert_eq!(page1.cursor.as_deref(), Some("did:plc:b"));

        // Second page picks up after the cursor; returns c, no further cursor.
        let page2 = search_accounts(
            State(ctx),
            admin_test_auth(),
            Query(SearchAccountsQuery {
                q: None,
                email: None,
                cursor: page1.cursor,
                limit: Some(2),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(page2.accounts.len(), 1);
        assert_eq!(page2.accounts[0].did, "did:plc:c");
        assert!(page2.cursor.is_none());
    }

    // ---------- Arc 6 Step 7: updateSubjectStatus dual-shape ----------
    //
    // Per V04_DESIGN §5.3.6 + Step 0 Q9. RepoBlobRef subjects accept
    // both canonical `record_uri` (snake_case) and legacy `recordUri`
    // (camelCase). The custom Deserialize on UpdateSubjectStatusRequest
    // peeks at the raw JSON to flag which form was used; the handler
    // reads the flag to record a legacy-wire-shape counter increment.

    #[test]
    fn update_subject_status_parses_canonical_record_uri_shape() {
        let json = r#"{
            "subject": {
                "$type": "com.atproto.admin.defs#repoBlobRef",
                "did": "did:plc:owner",
                "cid": "bafyblob",
                "record_uri": "at://did:plc:owner/app.bsky.feed.post/x"
            },
            "takedown": {"applied": true}
        }"#;
        let req: UpdateSubjectStatusRequest = serde_json::from_str(json).unwrap();
        assert!(
            !req.legacy_record_uri_used,
            "canonical 'record_uri' must not flag legacy"
        );
        if let SubjectUnion::RepoBlobRef { record_uri, .. } = &req.subject {
            assert_eq!(
                record_uri.as_deref(),
                Some("at://did:plc:owner/app.bsky.feed.post/x")
            );
        } else {
            panic!("expected RepoBlobRef variant");
        }
    }

    #[test]
    fn update_subject_status_parses_legacy_camelcase_shape_and_flags_it() {
        let json = r#"{
            "subject": {
                "$type": "com.atproto.admin.defs#repoBlobRef",
                "did": "did:plc:owner",
                "cid": "bafyblob",
                "recordUri": "at://did:plc:owner/app.bsky.feed.post/x"
            },
            "takedown": {"applied": true}
        }"#;
        let req: UpdateSubjectStatusRequest = serde_json::from_str(json).unwrap();
        assert!(
            req.legacy_record_uri_used,
            "legacy 'recordUri' must flag for handler-side observability"
        );
        if let SubjectUnion::RepoBlobRef { record_uri, .. } = &req.subject {
            assert_eq!(
                record_uri.as_deref(),
                Some("at://did:plc:owner/app.bsky.feed.post/x"),
                "alias must normalize recordUri → record_uri"
            );
        } else {
            panic!("expected RepoBlobRef variant");
        }
    }

    #[test]
    fn update_subject_status_rejects_both_record_uri_shapes_simultaneously() {
        let json = r#"{
            "subject": {
                "$type": "com.atproto.admin.defs#repoBlobRef",
                "did": "did:plc:owner",
                "cid": "bafyblob",
                "record_uri": "at://x/y/1",
                "recordUri": "at://x/y/2"
            },
            "takedown": {"applied": true}
        }"#;
        let err = serde_json::from_str::<UpdateSubjectStatusRequest>(json).unwrap_err();
        assert!(
            err.to_string().contains("not both"),
            "error must point at the both-shapes-present case; got: {}",
            err
        );
    }

    #[test]
    fn update_subject_status_non_blob_subjects_dont_flag_legacy() {
        // RepoRef and StrongRef subjects don't have record_uri at all,
        // so the legacy flag must remain false regardless.
        let repo_ref_json = r#"{
            "subject": {
                "$type": "com.atproto.admin.defs#repoRef",
                "did": "did:plc:owner"
            },
            "takedown": {"applied": true}
        }"#;
        let req: UpdateSubjectStatusRequest = serde_json::from_str(repo_ref_json).unwrap();
        assert!(!req.legacy_record_uri_used);

        let strong_ref_json = r#"{
            "subject": {
                "$type": "com.atproto.repo.strongRef",
                "uri": "at://did:plc:foo/app.bsky.feed.post/x",
                "cid": "bafyabc"
            },
            "takedown": {"applied": true}
        }"#;
        let req: UpdateSubjectStatusRequest = serde_json::from_str(strong_ref_json).unwrap();
        assert!(!req.legacy_record_uri_used);
    }

    // ---------- §8.1.7 / #273 — listSessions + revokeSession ----------

    fn op_auth(did: &str, role: Role, sid: &str) -> AdminAuthContext {
        AdminAuthContext {
            did: did.to_string(),
            session: ValidatedSession {
                did: did.to_string(),
                session_id: sid.to_string(),
                is_app_password: false,
            },
            role,
        }
    }

    fn list_query(did: Option<&str>) -> axum::extract::Query<ListSessionsQuery> {
        axum::extract::Query(ListSessionsQuery {
            did: did.map(|s| s.to_string()),
            pagination: PaginationParams::default(),
        })
    }

    #[tokio::test]
    async fn list_sessions_self_service_returns_own_with_current_flag() {
        let ctx = create_test_context().await;
        let did = "did:plc:selfop";
        let sid1 = ctx
            .operator_session_store
            .create(did, Some("203.0.113.1"), None, "r1", chrono::Duration::days(30))
            .await
            .unwrap();
        let _sid2 = ctx
            .operator_session_store
            .create(did, None, None, "r1", chrono::Duration::days(30))
            .await
            .unwrap();

        let resp = list_sessions(
            State(ctx.clone()),
            op_auth(did, Role::Moderator, &sid1),
            list_query(None),
        )
        .await
        .expect("list ok")
        .0;
        let sessions = resp["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 2, "both of the operator's sessions");
        let current: Vec<_> = sessions.iter().filter(|s| s["isCurrent"] == true).collect();
        assert_eq!(current.len(), 1, "exactly the caller's own session is current");
        assert_eq!(current[0]["sid"], sid1);
    }

    #[tokio::test]
    async fn list_sessions_superadmin_lists_all_operators() {
        let ctx = create_test_context().await;
        ctx.operator_session_store
            .create("did:plc:opA", None, None, "r1", chrono::Duration::days(30))
            .await
            .unwrap();
        ctx.operator_session_store
            .create("did:plc:opB", None, None, "r1", chrono::Duration::days(30))
            .await
            .unwrap();

        let resp = list_sessions(
            State(ctx.clone()),
            op_auth("did:plc:super", Role::SuperAdmin, "super-sid"),
            list_query(None),
        )
        .await
        .expect("list ok")
        .0;
        let dids: Vec<&str> = resp["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["did"].as_str().unwrap())
            .collect();
        assert!(dids.contains(&"did:plc:opA") && dids.contains(&"did:plc:opB"));
    }

    #[tokio::test]
    async fn list_sessions_non_superadmin_foreign_did_forbidden() {
        let ctx = create_test_context().await;
        let err = list_sessions(
            State(ctx),
            op_auth("did:plc:me", Role::Admin, "my-sid"),
            list_query(Some("did:plc:someone-else")),
        )
        .await
        .expect_err("foreign did without SuperAdmin must be forbidden");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn revoke_session_self_service_fires_the_gate() {
        let ctx = create_test_context().await;
        let did = "did:plc:selfrevoke";
        let sid = ctx
            .operator_session_store
            .create(did, None, None, "r1", chrono::Duration::days(30))
            .await
            .unwrap();
        // Self-service revoke needs no rationale.
        let out = revoke_session(
            State(ctx.clone()),
            op_auth(did, Role::Moderator, &sid),
            axum::Json(RevokeSessionRequest {
                sid: sid.clone(),
                rationale: None,
            }),
        )
        .await
        .expect("self-service revoke succeeds")
        .0;
        assert!(out.success);
        assert!(
            !ctx.operator_session_store
                .validate_and_touch(&sid)
                .await
                .unwrap(),
            "self-revoked session fails the per-request gate"
        );
    }

    #[tokio::test]
    async fn revoke_session_superadmin_force_logout_gate_and_rationale() {
        let ctx = create_test_context().await;
        let victim = "did:plc:victim";
        let sid = ctx
            .operator_session_store
            .create(victim, None, None, "r1", chrono::Duration::days(30))
            .await
            .unwrap();
        let super_auth = || op_auth("did:plc:super", Role::SuperAdmin, "super-sid");

        // Cross-operator force-logout requires a rationale.
        let err = revoke_session(
            State(ctx.clone()),
            super_auth(),
            axum::Json(RevokeSessionRequest {
                sid: sid.clone(),
                rationale: None,
            }),
        )
        .await
        .expect_err("cross-operator revoke without rationale is rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);

        // With a rationale it succeeds and the gate fires.
        let out = revoke_session(
            State(ctx.clone()),
            super_auth(),
            axum::Json(RevokeSessionRequest {
                sid: sid.clone(),
                rationale: Some("suspected credential compromise".to_string()),
            }),
        )
        .await
        .expect("superadmin force-logout succeeds")
        .0;
        assert!(out.success);
        assert!(
            !ctx.operator_session_store
                .validate_and_touch(&sid)
                .await
                .unwrap(),
            "force-logged-out session fails the per-request gate"
        );
    }

    #[tokio::test]
    async fn revoke_session_moderator_cannot_revoke_other() {
        let ctx = create_test_context().await;
        let sid = ctx
            .operator_session_store
            .create("did:plc:other", None, None, "r1", chrono::Duration::days(30))
            .await
            .unwrap();
        let err = revoke_session(
            State(ctx),
            op_auth("did:plc:mod", Role::Moderator, "mod-sid"),
            axum::Json(RevokeSessionRequest {
                sid,
                rationale: Some("nope".to_string()),
            }),
        )
        .await
        .expect_err("non-SuperAdmin revoking another's session is forbidden");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    /// The full §8.1.7 verification gate, end to end through both surfaces:
    /// SuperAdmin force-logs-out a specific operator session via the XRPC,
    /// and that operator's next request (a real admin token bearing the
    /// session's `sid`) reauthenticates — admin_auth_from_token now rejects.
    #[tokio::test]
    async fn force_logout_makes_operators_next_request_reauthenticate() {
        let ctx = create_test_context().await;
        let victim = "did:plc:gatevictim";
        ctx.admin_role_manager
            .grant_role(victim, Role::Admin, "did:plc:bootstrap", None)
            .await
            .unwrap();
        let sid = ctx
            .operator_session_store
            .create(victim, None, None, "r1", chrono::Duration::days(30))
            .await
            .unwrap();

        // A real admin token bearing the session sid.
        let secret = &ctx.config.authentication.jwt_secret;
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &serde_json::json!({
                "sub": victim,
                "scope": "admin",
                "exp": chrono::Utc::now().timestamp() + 3600,
                "sid": sid,
            }),
            &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();

        // Live before revoke.
        crate::auth::admin_auth_from_token(&ctx, &token)
            .await
            .expect("token authenticates before force-logout");

        // SuperAdmin force-logs-out the session.
        let _ = revoke_session(
            State(ctx.clone()),
            op_auth("did:plc:super", Role::SuperAdmin, "super-sid"),
            axum::Json(RevokeSessionRequest {
                sid: sid.clone(),
                rationale: Some("compromised laptop".to_string()),
            }),
        )
        .await
        .expect("force-logout succeeds");

        // Next request reauthenticates: the same token is now rejected.
        match crate::auth::admin_auth_from_token(&ctx, &token).await {
            Err(PdsError::Authentication(_)) => {}
            other => panic!("expected Authentication (401) after force-logout, got {:?}", other),
        }
    }

    // ---------- §7.4.1 / #286 preRebuildCheck ----------

    #[tokio::test]
    async fn pre_rebuild_check_requires_superadmin() {
        let ctx = create_test_context().await;
        let err = pre_rebuild_check(
            State(ctx),
            op_auth("did:plc:admin", Role::Admin, "sid"),
            axum::extract::Query(PreRebuildCheckParams {
                did: "did:plc:target".to_string(),
                deep: false,
            }),
        )
        .await
        .expect_err("Admin (not SuperAdmin) must be forbidden");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn pre_rebuild_check_404_when_no_history() {
        let ctx = create_test_context().await;
        // A fresh context's sequencer has no commit events for this DID, so the
        // preflight returns None → 404 (nothing to rebuild). Exercises the
        // handler's None path + SuperAdmin gate; the aggregation itself is
        // covered by the sequencer-level rebuild_preflight tests.
        let err = pre_rebuild_check(
            State(ctx),
            op_auth("did:plc:super", Role::SuperAdmin, "sid"),
            axum::extract::Query(PreRebuildCheckParams {
                did: "did:plc:no-history".to_string(),
                deep: false,
            }),
        )
        .await
        .expect_err("no history → NotFound");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn pre_rebuild_check_deep_surfaces_unverifiable_history() {
        use crate::sequencer::events::CommitEvent;
        use proto_blue::lex_cbor::cid_for_lex;
        use proto_blue::lex_data::LexValue;
        use proto_blue::repo::{blocks_to_car, BlockMap};

        let ctx = create_test_context().await;
        let did = "did:plc:deep-broken";
        // Seed one commit whose head block is absent from its (empty) CAR. The
        // metadata preflight still succeeds (commit exists), but deep
        // reconstruction can't resolve the repo → verify_repo errors.
        let absent = cid_for_lex(&LexValue::String("absent".to_string())).unwrap();
        ctx.sequencer
            .sequence_commit(CommitEvent::new(
                did.to_string(),
                absent.to_string(),
                "3jzfcijpj2z2a".to_string(),
                None,
                None,
                blocks_to_car(None, &BlockMap::new()).unwrap(),
                vec![],
            ))
            .await
            .unwrap();

        let resp = pre_rebuild_check(
            State(ctx),
            op_auth("did:plc:super", Role::SuperAdmin, "sid"),
            axum::extract::Query(PreRebuildCheckParams {
                did: did.to_string(),
                deep: true,
            }),
        )
        .await
        .expect("unverifiable history is a 200 diagnostic, not a 500");
        // Wiring assertion: deep=true actually ran reconstruction and reported it.
        assert_eq!(resp.0["deepVerified"], serde_json::Value::Bool(false));
        assert!(
            resp.0["deepError"].is_string(),
            "the verification failure must be surfaced as a diagnostic"
        );
    }

    // ---------- §7.4.1 / #290 destructive rebuild XRPCs ----------

    #[tokio::test]
    async fn rebuild_repo_rejects_non_superadmin() {
        let ctx = create_test_context().await;
        let err = rebuild_repo(
            State(ctx),
            op_auth("did:plc:admin", Role::Admin, "sid"),
            Json(RebuildRepoRequest {
                did: "did:plc:target".to_string(),
                rationale: Some("fixing it".to_string()),
            }),
        )
        .await
        .expect_err("Admin (not SuperAdmin) must be forbidden");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn rebuild_repo_requires_rationale() {
        let ctx = create_test_context().await;
        // Empty/whitespace rationale is rejected (high-impact destructive action).
        let err = rebuild_repo(
            State(ctx),
            op_auth("did:plc:super", Role::SuperAdmin, "sid"),
            Json(RebuildRepoRequest {
                did: "did:plc:target".to_string(),
                rationale: Some("   ".to_string()),
            }),
        )
        .await
        .expect_err("missing rationale → 400");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_rebuild_progress_unknown_job_404() {
        let ctx = create_test_context().await;
        let err = get_rebuild_progress(
            State(ctx),
            op_auth("did:plc:super", Role::SuperAdmin, "sid"),
            axum::extract::Query(GetRebuildProgressParams {
                job_id: "no-such-job".to_string(),
            }),
        )
        .await
        .expect_err("unknown job → NotFound");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn cancel_rebuild_unknown_job_404() {
        let ctx = create_test_context().await;
        let err = cancel_rebuild(
            State(ctx),
            op_auth("did:plc:super", Role::SuperAdmin, "sid"),
            Json(CancelRebuildRequest {
                job_id: "no-such-job".to_string(),
            }),
        )
        .await
        .expect_err("unknown job → NotFound");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }

    /// End-to-end job lifecycle through the handlers: starting a rebuild for an
    /// account with no resolvable signing key spawns a job that fails fast, and
    /// `getRebuildProgress` reports the terminal `failed` phase with a
    /// diagnostic — exercising start → spawn → run → finish → progress, and
    /// proving the original repo is never touched (shadow-then-swap).
    #[tokio::test]
    async fn rebuild_repo_lifecycle_reports_terminal_failure() {
        let ctx = create_test_context().await;
        let started = rebuild_repo(
            State(ctx.clone()),
            op_auth("did:plc:super", Role::SuperAdmin, "sid"),
            Json(RebuildRepoRequest {
                did: "did:plc:no-account".to_string(),
                rationale: Some("investigating".to_string()),
            }),
        )
        .await
        .expect("a rebuild is started")
        .0;
        let job_id = started["jobId"].as_str().expect("jobId returned").to_string();
        assert_eq!(started["status"], "started");

        // Poll until the background job reaches a terminal phase.
        let mut phase = String::new();
        let mut error_present = false;
        for _ in 0..200 {
            let p = get_rebuild_progress(
                State(ctx.clone()),
                op_auth("did:plc:super", Role::SuperAdmin, "sid"),
                axum::extract::Query(GetRebuildProgressParams {
                    job_id: job_id.clone(),
                }),
            )
            .await
            .expect("progress for a known job")
            .0;
            phase = p["phase"].as_str().unwrap().to_string();
            error_present = p["error"].is_string();
            if phase == "failed" || phase == "completed" || phase == "cancelled" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(phase, "failed", "no-account rebuild must terminate as failed");
        assert!(error_present, "a failed job surfaces a diagnostic");
    }

    // ---------- §7.4.3 / #291 bulk-repair scan XRPCs ----------

    #[tokio::test]
    async fn scan_rejects_non_superadmin() {
        let ctx = create_test_context().await;
        let err = scan_repos_for_inconsistencies(
            State(ctx),
            op_auth("did:plc:admin", Role::Admin, "sid"),
        )
        .await
        .expect_err("Admin (not SuperAdmin) must be forbidden");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn get_repo_scan_results_rejects_non_superadmin() {
        let ctx = create_test_context().await;
        let err = get_repo_scan_results(
            State(ctx),
            op_auth("did:plc:admin", Role::Admin, "sid"),
            axum::extract::Query(GetRepoScanResultsParams { severity: None, limit: None, cursor: None }),
        )
        .await
        .expect_err("Admin must be forbidden");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn cancel_scan_with_none_running_is_409() {
        let ctx = create_test_context().await;
        let err = cancel_scan(State(ctx), op_auth("did:plc:super", Role::SuperAdmin, "sid"))
            .await
            .expect_err("no scan in progress → Conflict");
        assert_eq!(err.0, StatusCode::CONFLICT);
    }

    /// End-to-end scan lifecycle through the handlers: a fresh test context has
    /// no accounts, so the scan completes with zero findings — exercising
    /// start → run → finish (+ ScanCompleted audit) → getScanProgress →
    /// getRepoScanResults.
    #[tokio::test]
    async fn scan_lifecycle_empty_completes_with_no_findings() {
        let ctx = create_test_context().await;
        let started = scan_repos_for_inconsistencies(
            State(ctx.clone()),
            op_auth("did:plc:super", Role::SuperAdmin, "sid"),
        )
        .await
        .expect("scan starts")
        .0;
        assert_eq!(started["status"], "started");
        assert!(started["scanId"].is_string());

        // Poll until the scan reports not-running.
        let mut last_outcome = serde_json::Value::Null;
        for _ in 0..200 {
            let p = get_scan_progress(
                State(ctx.clone()),
                op_auth("did:plc:super", Role::SuperAdmin, "sid"),
            )
            .await
            .expect("progress")
            .0;
            if p["running"] == serde_json::Value::Bool(false) && p["lastOutcome"].is_string() {
                last_outcome = p["lastOutcome"].clone();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(last_outcome, "completed", "empty scan completes");

        // Results: no findings, zero counts.
        let results = get_repo_scan_results(
            State(ctx),
            op_auth("did:plc:super", Role::SuperAdmin, "sid"),
            axum::extract::Query(GetRepoScanResultsParams { severity: None, limit: None, cursor: None }),
        )
        .await
        .expect("results")
        .0;
        assert_eq!(results["findings"].as_array().unwrap().len(), 0);
        assert_eq!(results["counts"]["total"], 0);
    }

    // ---------- §7.4.3 / #292 bulk-repair XRPCs ----------

    #[tokio::test]
    async fn repair_repos_rejects_non_superadmin() {
        let ctx = create_test_context().await;
        let err = repair_repos(
            State(ctx),
            op_auth("did:plc:admin", Role::Admin, "sid"),
            Json(RepairReposRequest { dids: vec!["did:plc:x".into()], all: false, rationale: Some("fix".into()) }),
        )
        .await
        .expect_err("Admin must be forbidden");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn repair_repos_requires_rationale() {
        let ctx = create_test_context().await;
        let err = repair_repos(
            State(ctx),
            op_auth("did:plc:super", Role::SuperAdmin, "sid"),
            Json(RepairReposRequest { dids: vec!["did:plc:x".into()], all: false, rationale: None }),
        )
        .await
        .expect_err("missing rationale → 400");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn repair_repos_empty_targets_400() {
        let ctx = create_test_context().await;
        // all=true but no findings → empty target set → 400.
        let err = repair_repos(
            State(ctx),
            op_auth("did:plc:super", Role::SuperAdmin, "sid"),
            Json(RepairReposRequest { dids: vec![], all: true, rationale: Some("fix".into()) }),
        )
        .await
        .expect_err("no targets → 400");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn cancel_bulk_repair_with_none_running_is_409() {
        let ctx = create_test_context().await;
        let err = cancel_bulk_repair(State(ctx), op_auth("did:plc:super", Role::SuperAdmin, "sid"))
            .await
            .expect_err("no bulk repair → Conflict");
        assert_eq!(err.0, StatusCode::CONFLICT);
    }

    /// End-to-end bulk-repair lifecycle through the handlers: a bogus target
    /// DID (no account → its per-account rebuild fails) drives the loop to a
    /// completed batch with failed=1 — exercising start → run_one per target →
    /// tally → finish (+ BulkRepairInitiated envelope) → progress.
    #[tokio::test]
    async fn bulk_repair_lifecycle_tallies_per_account_failure() {
        let ctx = create_test_context().await;
        let started = repair_repos(
            State(ctx.clone()),
            op_auth("did:plc:super", Role::SuperAdmin, "sid"),
            Json(RepairReposRequest {
                dids: vec!["did:plc:no-account".into()],
                all: false,
                rationale: Some("investigating".into()),
            }),
        )
        .await
        .expect("bulk repair starts")
        .0;
        assert_eq!(started["status"], "started");
        assert_eq!(started["targetCount"], 1);

        let mut done = false;
        let mut progress = serde_json::Value::Null;
        for _ in 0..200 {
            progress = get_bulk_repair_progress(
                State(ctx.clone()),
                op_auth("did:plc:super", Role::SuperAdmin, "sid"),
            )
            .await
            .expect("progress")
            .0;
            if progress["running"] == serde_json::Value::Bool(false)
                && progress["lastOutcome"].is_string()
            {
                done = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(done, "bulk repair reached a terminal state");
        assert_eq!(progress["lastOutcome"], "completed");
        assert_eq!(progress["processed"], 1);
        assert_eq!(progress["failed"], 1, "the no-account target fails its rebuild");
        assert_eq!(progress["repaired"], 0);
    }

    // ---------- §7.4.2 / #294 sequencer-recovery XRPCs ----------

    #[tokio::test]
    async fn sequencer_recovery_options_rejects_non_superadmin() {
        let ctx = create_test_context().await;
        let err = sequencer_recovery_options(State(ctx), op_auth("did:plc:admin", Role::Admin, "sid"))
            .await
            .expect_err("Admin must be forbidden");
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn sequencer_recovery_options_lists_validate() {
        let ctx = create_test_context().await;
        let out = sequencer_recovery_options(State(ctx), op_auth("did:plc:super", Role::SuperAdmin, "sid"))
            .await
            .expect("options")
            .0;
        let ops = out["operations"].as_array().unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0]["id"], "validate");
        assert_eq!(ops[0]["destructive"], false);
        // Empty test sequencer → zero rows, no head.
        assert_eq!(out["state"]["totalRows"], 0);
        assert_eq!(out["state"]["invalidatedRows"], 0);
    }

    #[tokio::test]
    async fn run_sequencer_recovery_unknown_op_400() {
        let ctx = create_test_context().await;
        let err = run_sequencer_recovery(
            State(ctx),
            op_auth("did:plc:super", Role::SuperAdmin, "sid"),
            Json(RunSequencerRecoveryRequest { operation: "reSequence".into(), rationale: None }),
        )
        .await
        .expect_err("unknown operation → 400");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn cancel_sequencer_recovery_with_none_running_is_409() {
        let ctx = create_test_context().await;
        let err = cancel_sequencer_recovery(State(ctx), op_auth("did:plc:super", Role::SuperAdmin, "sid"))
            .await
            .expect_err("nothing running → Conflict");
        assert_eq!(err.0, StatusCode::CONFLICT);
    }

    /// End-to-end validate lifecycle through the handlers: a fresh (empty)
    /// sequencer validates clean — exercising runSequencerRecovery → run →
    /// finish (+ SequencerValidated audit) → getSequencerRecoveryProgress with
    /// the report attached.
    #[tokio::test]
    async fn sequencer_recovery_validate_lifecycle_clean() {
        let ctx = create_test_context().await;
        let started = run_sequencer_recovery(
            State(ctx.clone()),
            op_auth("did:plc:super", Role::SuperAdmin, "sid"),
            Json(RunSequencerRecoveryRequest { operation: "validate".into(), rationale: None }),
        )
        .await
        .expect("validate starts")
        .0;
        assert_eq!(started["status"], "started");
        assert_eq!(started["operation"], "validate");

        let mut progress = serde_json::Value::Null;
        let mut done = false;
        for _ in 0..200 {
            progress = get_sequencer_recovery_progress(
                State(ctx.clone()),
                op_auth("did:plc:super", Role::SuperAdmin, "sid"),
            )
            .await
            .expect("progress")
            .0;
            if progress["running"] == serde_json::Value::Bool(false)
                && progress["lastOutcome"].is_string()
            {
                done = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(done, "validate reached a terminal state");
        assert_eq!(progress["lastOutcome"], "completed");
        assert_eq!(progress["report"]["malformedCount"], 0);
        assert_eq!(progress["report"]["nonMonotonicCount"], 0);
    }
}
