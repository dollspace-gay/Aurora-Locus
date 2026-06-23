//! v0.9 Federation Pattern-1 Phase C (#353) — discovery modes + pending-discovery.
//!
//! Discovery becomes runtime-controlled: a 3-mode taxonomy
//! (`allowlist-only` | `auto-accept` | `discovery-disabled`), a bounded
//! pending-discovery surface (dedup-by-DID + last-seen LRU, max 100), and a
//! dismissal CRUD. The scheduler reads the mode once per scan-start and
//! dispatches each discovered peer accordingly. After this phase discovery is
//! runtime-mutable; relay set stays static (Phase D).
//!
//! Design body: `docs/internal/design/v09_federation_pattern1_substrate_design.md`
//! (§3.1–§3.4, §5.1, §6.3, R4-locked).
//!
//! **Memory-#18 translations (recorded for review):**
//! - **`process_discovered_peer` runs at the scheduler/handler level, not inside
//!   `PdsDiscovery`.** `PdsDiscovery` holds no `AppContext`, so it cannot read
//!   the runtime mode or `trusted_peers`. The scheduler job (`jobs/mod.rs`) and
//!   the manual `triggerPdsDiscovery` handler — both of which hold `ctx` —
//!   iterate the discovered `PdsInstance`s through `process_scan` here. The
//!   design's mode-at-scan-start / per-peer-dispatch / disabled-short-circuit
//!   contract is preserved; only the call site moves.
//! - **Two new `*_seeded` audit names** (`discovery_mode_seeded`,
//!   `pending_discoveries_seeded`) round out the seed-audit family §5.1
//!   enumerated only partially.
//! - **`source` NOT NULL** (inherited): auto-accept additions use
//!   `source="discovery"`; scheduler/abort/seed entries `system_diagnostic`;
//!   operator XRPCs `manual`.

use crate::api::aurora_admin::{
    cas_runtime_setting, read_runtime_row_value, resolve_runtime_setting,
    FEDERATION_POLICY_DISCOVERY_MODE_KEY, FEDERATION_POLICY_PENDING_DISCOVERIES_KEY,
};
use crate::api::federation_peers::{emit, guard_recovery, FedPeerError};
use crate::api::moderation_defaults::SYSTEM_DID;
use crate::context::AppContext;
use crate::federation::discovery::PdsInstance;

/// Bounded CAS retry budget (design §2.3 / §3.4).
const MAX_CAS_RETRIES: usize = 3;
/// Pending-discovery surface cap (design §3.4).
const PENDING_MAX: usize = 100;

const SOURCE_MANUAL: &str = "manual";
const SOURCE_DIAGNOSTIC: &str = "system_diagnostic";

// Audit action names (§5.1; free-form strings, no registry — Phase B recon R6).
const ACTION_MODE_SEEDED: &str = "federation.discovery_mode_seeded";
const ACTION_PENDING_SEEDED: &str = "federation.pending_discoveries_seeded";
const ACTION_SCHEDULED_RAN: &str = "federation.scheduled_discovery_ran";
const ACTION_MODE_CHANGED: &str = "federation.discovery_mode_changed";
const ACTION_MODE_CHANGE_ABORTED: &str = "federation.discovery_mode_change_aborted";
const ACTION_PENDING_DISMISSED: &str = "federation.pending_discovery_dismissed";
const ACTION_PENDING_DISMISS_ABORTED: &str = "federation.pending_discovery_dismiss_aborted";
const ACTION_PENDING_EVICTED: &str = "federation.pending_discovery_evicted";
const ACTION_SURFACING_ABORTED: &str = "federation.pending_discovery_surfacing_aborted";

const MODE_ALLOWLIST_ONLY: &str = "allowlist-only";
const MODE_AUTO_ACCEPT: &str = "auto-accept";
const MODE_DISCOVERY_DISABLED: &str = "discovery-disabled";

/// The 3-mode discovery taxonomy (design §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryMode {
    /// Discovered peers surface to the pending list for operator review (default).
    AllowlistOnly,
    /// Discovered peers are auto-added to the trusted set (relay-trust delegation).
    AutoAccept,
    /// Scheduler scans are skipped entirely; manual scans no-op per-peer.
    DiscoveryDisabled,
}

impl DiscoveryMode {
    pub fn as_str(self) -> &'static str {
        match self {
            DiscoveryMode::AllowlistOnly => MODE_ALLOWLIST_ONLY,
            DiscoveryMode::AutoAccept => MODE_AUTO_ACCEPT,
            DiscoveryMode::DiscoveryDisabled => MODE_DISCOVERY_DISABLED,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            MODE_ALLOWLIST_ONLY => Some(DiscoveryMode::AllowlistOnly),
            MODE_AUTO_ACCEPT => Some(DiscoveryMode::AutoAccept),
            MODE_DISCOVERY_DISABLED => Some(DiscoveryMode::DiscoveryDisabled),
            _ => None,
        }
    }
}

/// One pending-discovery entry (design §3.4 / R3 M-4 shape).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingEntry {
    pub did: String,
    pub url: String,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub first_scan_id: String,
    pub last_seen_scan_id: String,
}

/// Result of an `upsert_pending_discovery` call — lets the scheduler distinguish
/// outcomes for audit-emission purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    Added,
    Updated,
    EvictedAndAdded,
    CasExhausted,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn new_scan_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Read the live discovery mode (3-tier resolve; default `allowlist-only`).
pub async fn current_mode(ctx: &AppContext) -> DiscoveryMode {
    let v = resolve_runtime_setting(ctx, FEDERATION_POLICY_DISCOVERY_MODE_KEY).await;
    v.as_str()
        .and_then(DiscoveryMode::parse)
        .unwrap_or(DiscoveryMode::AllowlistOnly)
}

fn parse_pending(raw: &Option<String>) -> Vec<PendingEntry> {
    raw.as_deref()
        .and_then(|s| serde_json::from_str::<Vec<PendingEntry>>(s).ok())
        .unwrap_or_default()
}

/// CAS the pending-discoveries value (UPDATE when the row exists, insert-if-absent
/// on first write so a concurrent writer loses and retries).
async fn write_pending(
    ctx: &AppContext,
    expected: Option<&str>,
    new: &str,
    actor: &str,
) -> Result<bool, FedPeerError> {
    match expected {
        Some(exp) => {
            cas_runtime_setting(ctx, FEDERATION_POLICY_PENDING_DISCOVERIES_KEY, exp, new, actor)
                .await
                .map_err(|e| FedPeerError::Internal(e.to_string()))
        }
        None => {
            let res = sqlx::query(
                "INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) \
                 SELECT $1, $2, $3, $4 \
                 WHERE NOT EXISTS (SELECT 1 FROM runtime_settings WHERE key = $1)",
            )
            .bind(FEDERATION_POLICY_PENDING_DISCOVERIES_KEY)
            .bind(new)
            .bind(now())
            .bind(actor)
            .execute(&ctx.account_db)
            .await
            .map_err(|e| FedPeerError::Internal(e.to_string()))?;
            Ok(res.rows_affected() >= 1)
        }
    }
}

/// Dedup-by-DID + last-seen-update with bounded LRU eviction (design §3.4).
/// Returns the outcome; `CasExhausted` is an outcome (not an error) so the
/// caller emits the contextual abort audit.
pub async fn upsert_pending_discovery(
    ctx: &AppContext,
    inst: &PdsInstance,
    scan_id: &str,
) -> Result<UpsertOutcome, FedPeerError> {
    for _ in 0..MAX_CAS_RETRIES {
        let raw = read_runtime_row_value(ctx, FEDERATION_POLICY_PENDING_DISCOVERIES_KEY).await;
        let mut pending = parse_pending(&raw);
        let stamp = now();

        // Existing entry: refresh last-seen, preserve first-seen.
        if let Some(e) = pending.iter_mut().find(|e| e.did == inst.did) {
            e.last_seen_at = stamp.clone();
            e.last_seen_scan_id = scan_id.to_string();
            let new = serde_json::to_string(&pending)
                .map_err(|e| FedPeerError::Internal(e.to_string()))?;
            if write_pending(ctx, raw.as_deref(), &new, SYSTEM_DID).await? {
                return Ok(UpsertOutcome::Updated);
            }
            continue;
        }

        // New entry. Evict the oldest-last-seen if the surface is full.
        let mut evicted: Option<PendingEntry> = None;
        if pending.len() >= PENDING_MAX {
            if let Some((idx, _)) = pending
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.last_seen_at.cmp(&b.1.last_seen_at))
            {
                evicted = Some(pending.remove(idx));
            }
        }
        pending.push(PendingEntry {
            did: inst.did.clone(),
            url: inst.url.clone(),
            first_seen_at: stamp.clone(),
            last_seen_at: stamp.clone(),
            first_scan_id: scan_id.to_string(),
            last_seen_scan_id: scan_id.to_string(),
        });
        let new = serde_json::to_string(&pending)
            .map_err(|e| FedPeerError::Internal(e.to_string()))?;
        if write_pending(ctx, raw.as_deref(), &new, SYSTEM_DID).await? {
            if let Some(ev) = evicted {
                emit(
                    ctx,
                    SYSTEM_DID,
                    ACTION_PENDING_EVICTED,
                    SOURCE_DIAGNOSTIC,
                    serde_json::json!({ "did": ev.did, "url": ev.url, "eviction_reason": "list_full_lru" }),
                    "pending discovery evicted (list full, LRU)",
                )
                .await?;
                return Ok(UpsertOutcome::EvictedAndAdded);
            }
            return Ok(UpsertOutcome::Added);
        }
    }
    Ok(UpsertOutcome::CasExhausted)
}

/// Per-peer mode dispatch (design §3.2 / R3 H-1). Never propagates per-peer
/// failures — a scan processes many peers, so each is logged and the loop
/// continues. Skips peers already trusted.
async fn process_discovered_peer(
    ctx: &AppContext,
    inst: &PdsInstance,
    mode: DiscoveryMode,
    scan_id: &str,
) {
    if ctx.trusted_peers.is_trusted(&inst.did).await {
        return;
    }
    match mode {
        DiscoveryMode::AllowlistOnly => {
            match upsert_pending_discovery(ctx, inst, scan_id).await {
                Ok(UpsertOutcome::CasExhausted) => {
                    let _ = emit(
                        ctx,
                        SYSTEM_DID,
                        ACTION_SURFACING_ABORTED,
                        SOURCE_DIAGNOSTIC,
                        serde_json::json!({ "did": inst.did, "url": inst.url, "scan_id": scan_id, "reason": "cas_exhausted" }),
                        "pending discovery surfacing aborted: CAS exhausted",
                    )
                    .await;
                }
                Ok(_) => {}
                Err(e) => tracing::error!(error = ?e, did = %inst.did, "pending-discovery upsert failed"),
            }
        }
        DiscoveryMode::AutoAccept => {
            // add_discovered_peer emits its own peer_added / peer_add_aborted.
            if let Err(e) = crate::api::federation_peers::add_discovered_peer(
                ctx, &inst.did, &inst.url, scan_id,
            )
            .await
            {
                match e {
                    // Raced with another add, or already self-emitted abort.
                    FedPeerError::DuplicateDid(_) | FedPeerError::CasExhausted => {}
                    other => {
                        tracing::error!(error = ?other, did = %inst.did, "auto-accept add failed")
                    }
                }
            }
        }
        // Manual scans reach here under disabled mode (scheduler short-circuits
        // before iterating); per-peer no-op.
        DiscoveryMode::DiscoveryDisabled => {}
    }
}

/// Process a scan's discovered instances under `mode`. When `emit_scan_audit`
/// (scheduler path), generates a scan-id and emits `scheduled_discovery_ran`;
/// the manual path passes `false` (it keeps its own `federation.discover`).
/// Returns the scan_id used.
pub async fn process_scan(
    ctx: &AppContext,
    instances: &[PdsInstance],
    mode: DiscoveryMode,
    emit_scan_audit: bool,
) -> String {
    let scan_id = new_scan_id();
    for inst in instances {
        process_discovered_peer(ctx, inst, mode, &scan_id).await;
    }
    if emit_scan_audit {
        let _ = emit(
            ctx,
            SYSTEM_DID,
            ACTION_SCHEDULED_RAN,
            SOURCE_DIAGNOSTIC,
            serde_json::json!({
                "scan_id": scan_id,
                "relays": ctx.config.federation.relay_urls,
                "mode": mode.as_str(),
                "discovered_count": instances.len(),
            }),
            "scheduled discovery scan ran",
        )
        .await;
    }
    scan_id
}

/// Boot-seed `federation.policy.discovery-mode` to `allowlist-only` when unset
/// (design §2.4 / commit 9). Idempotent; emits `discovery_mode_seeded`.
pub async fn seed_discovery_mode(ctx: &AppContext) {
    seed_scalar_if_absent(
        ctx,
        FEDERATION_POLICY_DISCOVERY_MODE_KEY,
        serde_json::json!(MODE_ALLOWLIST_ONLY),
        ACTION_MODE_SEEDED,
        serde_json::json!({ "mode": MODE_ALLOWLIST_ONLY }),
        "discovery mode seeded at boot",
    )
    .await;
}

/// Boot-seed `federation.policy.pending-discoveries` to `[]` when unset.
/// Idempotent; emits `pending_discoveries_seeded`.
pub async fn seed_pending_discoveries(ctx: &AppContext) {
    seed_scalar_if_absent(
        ctx,
        FEDERATION_POLICY_PENDING_DISCOVERIES_KEY,
        serde_json::json!([]),
        ACTION_PENDING_SEEDED,
        serde_json::json!({ "initial_count": 0 }),
        "pending-discoveries surface seeded at boot",
    )
    .await;
}

/// Seed-if-absent for a single runtime key + a one-shot seed audit. Best-effort;
/// never blocks boot.
async fn seed_scalar_if_absent(
    ctx: &AppContext,
    key: &str,
    value: serde_json::Value,
    audit_action: &str,
    audit_payload: serde_json::Value,
    rationale: &str,
) {
    let exists = sqlx::query("SELECT 1 FROM runtime_settings WHERE key = $1")
        .bind(key)
        .fetch_optional(&ctx.account_db)
        .await;
    match exists {
        Ok(Some(_)) => return,
        Ok(None) => {}
        Err(e) => {
            tracing::error!(error = %e, key, "discovery seed: existence probe failed");
            return;
        }
    }
    let encoded = match serde_json::to_string(&value) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, key, "discovery seed: encode failed");
            return;
        }
    };
    let inserted = sqlx::query(
        "INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) \
         SELECT $1, $2, $3, $4 \
         WHERE NOT EXISTS (SELECT 1 FROM runtime_settings WHERE key = $1)",
    )
    .bind(key)
    .bind(&encoded)
    .bind(now())
    .bind(SYSTEM_DID)
    .execute(&ctx.account_db)
    .await;
    match inserted {
        Ok(res) if res.rows_affected() >= 1 => {
            if let Err(e) = emit(
                ctx,
                SYSTEM_DID,
                audit_action,
                SOURCE_DIAGNOSTIC,
                audit_payload,
                rationale,
            )
            .await
            {
                tracing::error!(error = ?e, key, "discovery seed: audit emit failed");
            }
        }
        Ok(_) => {}
        Err(e) => tracing::error!(error = %e, key, "discovery seed: insert failed"),
    }
}

/// `setDiscoveryMode` core (design §3.3). SuperAdmin gating is at the handler;
/// recovery lockout + validation + CAS + audit here.
pub async fn set_discovery_mode(
    ctx: &AppContext,
    operator_did: &str,
    mode_str: &str,
) -> Result<(), FedPeerError> {
    guard_recovery()?;
    let mode = DiscoveryMode::parse(mode_str)
        .ok_or_else(|| FedPeerError::InvalidMode(mode_str.to_string()))?;

    for _ in 0..MAX_CAS_RETRIES {
        let raw = read_runtime_row_value(ctx, FEDERATION_POLICY_DISCOVERY_MODE_KEY).await;
        let current = raw
            .as_deref()
            .and_then(|s| serde_json::from_str::<String>(s).ok())
            .unwrap_or_else(|| MODE_ALLOWLIST_ONLY.to_string());
        if current == mode.as_str() {
            return Ok(()); // No-op: same mode, no audit (Phase B no-change parity).
        }
        let new = serde_json::to_string(mode.as_str())
            .map_err(|e| FedPeerError::Internal(e.to_string()))?;
        let wrote = match raw.as_deref() {
            Some(exp) => cas_runtime_setting(
                ctx,
                FEDERATION_POLICY_DISCOVERY_MODE_KEY,
                exp,
                &new,
                operator_did,
            )
            .await
            .map_err(|e| FedPeerError::Internal(e.to_string()))?,
            None => {
                // Unseeded (defensive): insert directly.
                let res = sqlx::query(
                    "INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) \
                     SELECT $1, $2, $3, $4 \
                     WHERE NOT EXISTS (SELECT 1 FROM runtime_settings WHERE key = $1)",
                )
                .bind(FEDERATION_POLICY_DISCOVERY_MODE_KEY)
                .bind(&new)
                .bind(now())
                .bind(operator_did)
                .execute(&ctx.account_db)
                .await
                .map_err(|e| FedPeerError::Internal(e.to_string()))?;
                res.rows_affected() >= 1
            }
        };
        if wrote {
            emit(
                ctx,
                operator_did,
                ACTION_MODE_CHANGED,
                SOURCE_MANUAL,
                serde_json::json!({ "before": current, "after": mode.as_str() }),
                "discovery mode changed",
            )
            .await?;
            return Ok(());
        }
    }
    emit(
        ctx,
        operator_did,
        ACTION_MODE_CHANGE_ABORTED,
        SOURCE_DIAGNOSTIC,
        serde_json::json!({ "attempted_after": mode.as_str(), "reason": "cas_exhausted" }),
        "discovery mode change aborted: CAS exhausted",
    )
    .await?;
    Err(FedPeerError::CasExhausted)
}

/// `dismissPendingDiscovery` core (design §3.4). Removes a pending entry by DID.
pub async fn dismiss_pending_discovery(
    ctx: &AppContext,
    operator_did: &str,
    did: &str,
) -> Result<(), FedPeerError> {
    guard_recovery()?;

    for _ in 0..MAX_CAS_RETRIES {
        let raw = read_runtime_row_value(ctx, FEDERATION_POLICY_PENDING_DISCOVERIES_KEY).await;
        let pending = parse_pending(&raw);
        let Some(target) = pending.iter().find(|e| e.did == did).cloned() else {
            return Err(FedPeerError::NotPresent(did.to_string()));
        };
        let remaining: Vec<PendingEntry> =
            pending.into_iter().filter(|e| e.did != did).collect();
        let new = serde_json::to_string(&remaining)
            .map_err(|e| FedPeerError::Internal(e.to_string()))?;
        if write_pending(ctx, raw.as_deref(), &new, operator_did).await? {
            emit(
                ctx,
                operator_did,
                ACTION_PENDING_DISMISSED,
                SOURCE_MANUAL,
                serde_json::json!({ "did": target.did, "url": target.url, "dismissed_reason": "manual" }),
                "pending discovery dismissed",
            )
            .await?;
            return Ok(());
        }
    }
    emit(
        ctx,
        operator_did,
        ACTION_PENDING_DISMISS_ABORTED,
        SOURCE_DIAGNOSTIC,
        serde_json::json!({ "did": did, "reason": "cas_exhausted" }),
        "pending discovery dismiss aborted: CAS exhausted",
    )
    .await?;
    Err(FedPeerError::CasExhausted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst(did: &str, url: &str) -> PdsInstance {
        PdsInstance {
            did: did.to_string(),
            url: url.to_string(),
            name: None,
            open_registrations: false,
            user_count: None,
            last_seen: None,
            features: Vec::new(),
        }
    }

    async fn audit_count(ctx: &AppContext, action: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_chain_entry WHERE action = $1")
            .bind(action)
            .fetch_one(&ctx.account_db)
            .await
            .unwrap()
    }

    async fn read_pending(ctx: &AppContext) -> Vec<PendingEntry> {
        parse_pending(&read_runtime_row_value(ctx, FEDERATION_POLICY_PENDING_DISCOVERIES_KEY).await)
    }

    #[tokio::test]
    async fn mode_parse_roundtrip() {
        for m in [
            DiscoveryMode::AllowlistOnly,
            DiscoveryMode::AutoAccept,
            DiscoveryMode::DiscoveryDisabled,
        ] {
            assert_eq!(DiscoveryMode::parse(m.as_str()), Some(m));
        }
        assert_eq!(DiscoveryMode::parse("nonsense"), None);
    }

    #[tokio::test]
    async fn seed_then_mode_is_allowlist_only_and_pending_empty() {
        let _g = crate::api::federation_peers::test_support::serial().lock().await;
        let ctx = crate::api::federation_peers::test_support::ctx_with_peers(&[]).await;
        seed_discovery_mode(&ctx).await;
        seed_pending_discoveries(&ctx).await;
        assert_eq!(current_mode(&ctx).await, DiscoveryMode::AllowlistOnly);
        assert_eq!(read_pending(&ctx).await.len(), 0);
        assert_eq!(audit_count(&ctx, ACTION_MODE_SEEDED).await, 1);
        assert_eq!(audit_count(&ctx, ACTION_PENDING_SEEDED).await, 1);
        // Idempotent.
        seed_discovery_mode(&ctx).await;
        assert_eq!(audit_count(&ctx, ACTION_MODE_SEEDED).await, 1);
    }

    #[tokio::test]
    async fn set_mode_changes_value_and_audits() {
        let _g = crate::api::federation_peers::test_support::serial().lock().await;
        let ctx = crate::api::federation_peers::test_support::ctx_with_peers(&[]).await;
        seed_discovery_mode(&ctx).await;
        set_discovery_mode(&ctx, "did:plc:op", "auto-accept").await.unwrap();
        assert_eq!(current_mode(&ctx).await, DiscoveryMode::AutoAccept);
        assert_eq!(audit_count(&ctx, ACTION_MODE_CHANGED).await, 1);
        // Same-mode is a no-op (no audit).
        set_discovery_mode(&ctx, "did:plc:op", "auto-accept").await.unwrap();
        assert_eq!(audit_count(&ctx, ACTION_MODE_CHANGED).await, 1);
    }

    #[tokio::test]
    async fn set_mode_rejects_invalid() {
        let _g = crate::api::federation_peers::test_support::serial().lock().await;
        let ctx = crate::api::federation_peers::test_support::ctx_with_peers(&[]).await;
        assert!(matches!(
            set_discovery_mode(&ctx, "did:plc:op", "bogus").await,
            Err(FedPeerError::InvalidMode(_))
        ));
    }

    #[tokio::test]
    async fn allowlist_only_scan_surfaces_to_pending() {
        let _g = crate::api::federation_peers::test_support::serial().lock().await;
        let ctx = crate::api::federation_peers::test_support::ctx_with_peers(&[]).await;
        seed_pending_discoveries(&ctx).await;
        let instances = [inst("did:plc:a", "https://a.example")];
        process_scan(&ctx, &instances, DiscoveryMode::AllowlistOnly, true).await;
        let pending = read_pending(&ctx).await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].did, "did:plc:a");
        assert_eq!(audit_count(&ctx, ACTION_SCHEDULED_RAN).await, 1);
        // Not auto-added to the trusted set.
        assert!(!ctx.trusted_peers.is_trusted("did:plc:a").await);
    }

    #[tokio::test]
    async fn auto_accept_scan_adds_to_trusted_with_discovery_source() {
        let _g = crate::api::federation_peers::test_support::serial().lock().await;
        let ctx = crate::api::federation_peers::test_support::ctx_with_peers(&[]).await;
        let instances = [inst("did:plc:a", "https://a.example")];
        process_scan(&ctx, &instances, DiscoveryMode::AutoAccept, true).await;
        assert!(ctx.trusted_peers.is_trusted("did:plc:a").await);
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_chain_entry WHERE action = 'federation.peer_added' AND source = 'discovery'",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn rediscovery_updates_last_seen_no_duplicate() {
        let _g = crate::api::federation_peers::test_support::serial().lock().await;
        let ctx = crate::api::federation_peers::test_support::ctx_with_peers(&[]).await;
        seed_pending_discoveries(&ctx).await;
        let instances = [inst("did:plc:a", "https://a.example")];
        let o1 = upsert_pending_discovery(&ctx, &instances[0], "scan1").await.unwrap();
        let o2 = upsert_pending_discovery(&ctx, &instances[0], "scan2").await.unwrap();
        assert_eq!(o1, UpsertOutcome::Added);
        assert_eq!(o2, UpsertOutcome::Updated);
        let pending = read_pending(&ctx).await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].first_scan_id, "scan1");
        assert_eq!(pending[0].last_seen_scan_id, "scan2");
    }

    #[tokio::test]
    async fn lru_eviction_at_capacity() {
        let _g = crate::api::federation_peers::test_support::serial().lock().await;
        let ctx = crate::api::federation_peers::test_support::ctx_with_peers(&[]).await;
        // Pre-seed a full surface with STRICTLY-ORDERED last_seen_at (craft the
        // timestamps directly so the LRU victim is deterministic — relying on
        // real-clock ordering across a tight loop collides at sub-ms speed).
        // did:plc:0 is oldest (…000000Z) … did:plc:99 newest (…000099Z).
        let entries: Vec<PendingEntry> = (0..PENDING_MAX)
            .map(|i| {
                let ts = format!("2026-01-01T00:00:00.{i:06}Z");
                PendingEntry {
                    did: format!("did:plc:{i}"),
                    url: "https://x.example".to_string(),
                    first_seen_at: ts.clone(),
                    last_seen_at: ts.clone(),
                    first_scan_id: "seed".to_string(),
                    last_seen_scan_id: "seed".to_string(),
                }
            })
            .collect();
        sqlx::query("INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) VALUES ($1, $2, 'now', 'test')")
            .bind(FEDERATION_POLICY_PENDING_DISCOVERIES_KEY)
            .bind(serde_json::to_string(&entries).unwrap())
            .execute(&ctx.account_db)
            .await
            .unwrap();

        // Refresh did:plc:0 → real now() (2026-06+) is newer than every crafted
        // 2026-01 stamp, so it is no longer the LRU victim.
        let out0 = upsert_pending_discovery(&ctx, &inst("did:plc:0", "https://x.example"), "refresh")
            .await
            .unwrap();
        assert_eq!(out0, UpsertOutcome::Updated);

        // The 101st distinct DID evicts the oldest-remaining last_seen (did:plc:1).
        let out = upsert_pending_discovery(&ctx, &inst("did:plc:new", "https://n.example"), "overflow")
            .await
            .unwrap();
        assert_eq!(out, UpsertOutcome::EvictedAndAdded);
        let pending = read_pending(&ctx).await;
        assert_eq!(pending.len(), PENDING_MAX);
        assert!(pending.iter().any(|e| e.did == "did:plc:0"), "refreshed entry survived");
        assert!(pending.iter().any(|e| e.did == "did:plc:new"), "new entry added");
        assert!(!pending.iter().any(|e| e.did == "did:plc:1"), "oldest evicted");
        assert_eq!(audit_count(&ctx, ACTION_PENDING_EVICTED).await, 1);
    }

    #[tokio::test]
    async fn dismiss_removes_and_audits() {
        let _g = crate::api::federation_peers::test_support::serial().lock().await;
        let ctx = crate::api::federation_peers::test_support::ctx_with_peers(&[]).await;
        seed_pending_discoveries(&ctx).await;
        upsert_pending_discovery(&ctx, &inst("did:plc:a", "https://a.example"), "s").await.unwrap();
        dismiss_pending_discovery(&ctx, "did:plc:op", "did:plc:a").await.unwrap();
        assert_eq!(read_pending(&ctx).await.len(), 0);
        assert_eq!(audit_count(&ctx, ACTION_PENDING_DISMISSED).await, 1);
        // Absent DID → NotPresent.
        assert!(matches!(
            dismiss_pending_discovery(&ctx, "did:plc:op", "did:plc:absent").await,
            Err(FedPeerError::NotPresent(_))
        ));
    }

    #[tokio::test]
    async fn disabled_mode_per_peer_noop() {
        let _g = crate::api::federation_peers::test_support::serial().lock().await;
        let ctx = crate::api::federation_peers::test_support::ctx_with_peers(&[]).await;
        seed_pending_discoveries(&ctx).await;
        process_scan(&ctx, &[inst("did:plc:a", "https://a.example")], DiscoveryMode::DiscoveryDisabled, false).await;
        assert_eq!(read_pending(&ctx).await.len(), 0);
        assert!(!ctx.trusted_peers.is_trusted("did:plc:a").await);
    }

    #[tokio::test]
    async fn recovery_mode_blocks_both_discovery_xrpcs() {
        let _g = crate::api::federation_peers::test_support::serial().lock().await;
        let ctx = crate::api::federation_peers::test_support::ctx_with_peers(&[]).await;
        seed_discovery_mode(&ctx).await;
        seed_pending_discoveries(&ctx).await;
        upsert_pending_discovery(&ctx, &inst("did:plc:a", "https://a.example"), "s").await.unwrap();
        std::env::set_var(
            crate::api::aurora_admin::RECOVERY_MODE_ENV,
            "true",
        );
        let m = set_discovery_mode(&ctx, "did:plc:op", "auto-accept").await;
        let d = dismiss_pending_discovery(&ctx, "did:plc:op", "did:plc:a").await;
        std::env::remove_var(crate::api::aurora_admin::RECOVERY_MODE_ENV);
        assert!(matches!(m, Err(FedPeerError::RecoveryMode)));
        assert!(matches!(d, Err(FedPeerError::RecoveryMode)));
        // Rejected at entry: mode unchanged, pending entry still present.
        assert_eq!(current_mode(&ctx).await, DiscoveryMode::AllowlistOnly);
        assert_eq!(read_pending(&ctx).await.len(), 1);
    }

    #[tokio::test]
    async fn mode_change_does_not_retroactively_accept_pending() {
        let _g = crate::api::federation_peers::test_support::serial().lock().await;
        let ctx = crate::api::federation_peers::test_support::ctx_with_peers(&[]).await;
        seed_discovery_mode(&ctx).await;
        seed_pending_discoveries(&ctx).await;
        // allowlist-only surfaces a peer to pending.
        process_scan(&ctx, &[inst("did:plc:a", "https://a.example")], DiscoveryMode::AllowlistOnly, false).await;
        // Switch to auto-accept; the existing pending entry is NOT retroactively trusted.
        set_discovery_mode(&ctx, "did:plc:op", "auto-accept").await.unwrap();
        assert!(!ctx.trusted_peers.is_trusted("did:plc:a").await);
        assert_eq!(read_pending(&ctx).await.len(), 1, "pending persists across mode change");
    }
}
