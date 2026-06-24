//! v0.9 Federation Pattern-1 Phase B (#352) — peer-allowlist CRUD.
//!
//! The first runtime-mutable federation surface. Three SuperAdmin operations
//! (add / remove / modify) mutate `federation.policy.peer-allowlist` via the
//! §5.5.4 value-CAS-with-bounded-retry primitive, each emitting an audit-chain
//! entry; a boot-time seed populates the key from `FederationConfig.peer_pds`
//! when unset. After this phase `TrustedPeerSet` reads a runtime-mutable list.
//!
//! Design body: `docs/internal/design/v09_federation_pattern1_substrate_design.md`
//! (§2.3, §2.4, §5.1, §6.1–§6.3, R4-locked).
//!
//! **Memory-#18 translations (substrate ≠ design body; recorded for review):**
//! - **No lexicon JSON.** AL admin verbs are NSID route-registry endpoints
//!   (`tools.aurora.ops.*` via `route_with_caps`), not lexicon files. The 3
//!   mutations register as `tools.aurora.ops.{add,remove,modify}FederationPeer`.
//! - **`source` is NOT NULL.** Design §5.2 specifies `source=NULL` for
//!   operator-XRPC audits; AL's `AppendEntryParams.source: &str` is NOT NULL, so
//!   operator mutations use `"manual"` and seed/abort entries use
//!   `"system_diagnostic"` (the §6.1 taxonomy).
//! - **Seed-with-audit is federation-specific.** Phase A's generic
//!   `seed_federation_policy` emits no audits; `seed_peer_allowlist` here does
//!   seed-if-absent + per-peer `federation.peer_seeded`. Only the peer-allowlist
//!   key is seeded in Phase B (discovery-mode / relay-urls / pending-discoveries
//!   stay unset for Phases C/D).
//! - **Recovery-mode lockout is a new guard.** No generic mutation-lockout
//!   helper exists; `RECOVERY_MODE_ENV` is read at handler entry → 503.

use crate::api::aurora_admin::{
    cas_runtime_setting, read_runtime_row_value, FEDERATION_POLICY_PEER_ALLOWLIST_KEY,
    FEDERATION_POLICY_PENDING_DISCOVERIES_KEY, FEDERATION_POLICY_RELAY_URLS_KEY, RECOVERY_MODE_ENV,
};
use crate::api::moderation_defaults::SYSTEM_DID;
use crate::error::PdsError;
use crate::admin::audit_chain::{self, AppendEntryParams};
use crate::context::AppContext;
use crate::federation::trusted_peer_set::PeerEntry;
use axum::http::StatusCode;
use axum::Json;

/// Bounded CAS retry budget (§5.5.4 Phase B precedent / design §2.3).
const MAX_CAS_RETRIES: usize = 3;

// Audit action names (§5.1). Free-form strings — no registry (recon R6).
const ACTION_PEER_ADDED: &str = "federation.peer_added";
const ACTION_PEER_REMOVED: &str = "federation.peer_removed";
const ACTION_PEER_MODIFIED: &str = "federation.peer_modified";
const ACTION_PEER_SEEDED: &str = "federation.peer_seeded";
const ACTION_PEER_ADD_ABORTED: &str = "federation.peer_add_aborted";
const ACTION_PEER_REMOVE_ABORTED: &str = "federation.peer_remove_aborted";
const ACTION_PEER_MODIFY_ABORTED: &str = "federation.peer_modify_aborted";
// Phase D (#354 / addendum §A4, §A7) — relay-switch audit names.
pub(crate) const ACTION_RELAY_ADDED: &str = "federation.relay_added";
pub(crate) const ACTION_RELAY_REMOVED: &str = "federation.relay_removed";
pub(crate) const ACTION_RELAY_SWITCHED: &str = "federation.relay_switched";
const ACTION_RELAY_SEEDED: &str = "federation.relay_seeded";
pub(crate) const ACTION_RELAY_ADD_ABORTED: &str = "federation.relay_add_aborted";
pub(crate) const ACTION_RELAY_REMOVE_ABORTED: &str = "federation.relay_remove_aborted";
pub(crate) const ACTION_RELAY_SWITCH_ABORTED: &str = "federation.relay_switch_aborted";
const ACTION_BOOT_SEED_FAILED: &str = "federation.boot_seed_failed";

// Audit source attribution (§6.1; memory-#18 translation #2).
const SOURCE_MANUAL: &str = "manual";
const SOURCE_DIAGNOSTIC: &str = "system_diagnostic";
/// Phase C auto-accept additions (design §5.2 / commit 25). The §5.5.4 §6.4
/// source-filter dropdown gains this value in Phase E (display-only filter, so
/// emitting it now is forward-compatible — recon).
const SOURCE_DISCOVERY: &str = "discovery";

/// Typed error for the peer-CRUD surface. Carries the distinct HTTP error
/// codes the handlers need (`rule_err` only maps 400/404, not the 503 cases).
#[derive(Debug)]
pub enum FedPeerError {
    /// Recovery mode active — mutations refused before any state change.
    RecoveryMode,
    /// CAS retries exhausted under contention (abort audit already emitted).
    CasExhausted,
    /// DID already present on add.
    DuplicateDid(String),
    /// DID absent on remove/modify.
    NotPresent(String),
    /// Malformed DID (no `did:` prefix / empty).
    InvalidDid(String),
    /// URL not HTTPS.
    InvalidUrl(String),
    /// Invalid discovery-mode value (Phase C `setDiscoveryMode`).
    InvalidMode(String),
    /// v0.9 Phase D (#354): boot-seed failed at startup; mutations refused until
    /// the operator corrects config + restarts. (Design `FederationError`
    /// translated onto this enum — there is no separate `FederationError` in AL.)
    BootSeedFailureActive,
    /// Phase D: the 60s relay-switch lock-acquisition timeout fired.
    LockAcquisitionTimeout,
    /// Phase D: `RelayClient::reconfigure` failed after the CAS-write succeeded.
    ReconfigureFailed(String),
    /// Phase D: relay operation requested but no relay client is configured.
    NoRelayClient,
    /// Substrate failure (DB / serialization).
    Internal(String),
}

impl FedPeerError {
    /// Map to the axum error response shape used across the admin surface
    /// (`{error, message}` + status).
    pub fn into_http(self) -> (StatusCode, Json<serde_json::Value>) {
        let (status, code, message) = match self {
            FedPeerError::RecoveryMode => (
                StatusCode::SERVICE_UNAVAILABLE,
                "RecoveryModeActive",
                "Federation policy mutations are disabled during recovery mode".to_string(),
            ),
            FedPeerError::CasExhausted => (
                StatusCode::SERVICE_UNAVAILABLE,
                "CasExhausted",
                "peer-allowlist update contended after retries; retry shortly".to_string(),
            ),
            FedPeerError::DuplicateDid(did) => (
                StatusCode::BAD_REQUEST,
                "DuplicatePeer",
                format!("peer already in allowlist: {did}"),
            ),
            FedPeerError::NotPresent(did) => (
                StatusCode::NOT_FOUND,
                "PeerNotFound",
                format!("peer not in allowlist: {did}"),
            ),
            FedPeerError::InvalidDid(did) => (
                StatusCode::BAD_REQUEST,
                "InvalidDid",
                format!("malformed DID (expected 'did:' prefix): {did}"),
            ),
            FedPeerError::InvalidUrl(url) => (
                StatusCode::BAD_REQUEST,
                "InvalidUrl",
                format!("peer URL must be HTTPS: {url}"),
            ),
            FedPeerError::InvalidMode(mode) => (
                StatusCode::BAD_REQUEST,
                "InvalidDiscoveryMode",
                format!(
                    "discovery mode must be allowlist-only | auto-accept | discovery-disabled: {mode}"
                ),
            ),
            FedPeerError::BootSeedFailureActive => (
                StatusCode::SERVICE_UNAVAILABLE,
                "BootSeedFailureActive",
                "federation policy mutations are disabled: a boot-seed failed; \
                 inspect the audit log, correct configuration, and restart"
                    .to_string(),
            ),
            FedPeerError::LockAcquisitionTimeout => (
                StatusCode::SERVICE_UNAVAILABLE,
                "LockAcquisitionTimeout",
                "relay-switch lock acquisition timed out; retry shortly".to_string(),
            ),
            FedPeerError::ReconfigureFailed(m) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "ReconfigureFailed",
                format!("relay reconfigure failed: {m}; runtime store updated, retry or restart"),
            ),
            FedPeerError::NoRelayClient => (
                StatusCode::SERVICE_UNAVAILABLE,
                "NoRelayClient",
                "no relay client configured (federation may be disabled)".to_string(),
            ),
            FedPeerError::Internal(m) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError", m)
            }
        };
        (
            status,
            Json(serde_json::json!({ "error": code, "message": message })),
        )
    }
}

type FedResult<T> = Result<T, FedPeerError>;

/// v0.9 Federation Pattern-1 Phase D (#354 / addendum §A6) — outcome of a
/// boot-seed wrapper, so `main.rs`'s boot-completion check can tell seeded from
/// already-seeded from skipped without inspecting the DB again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeedOutcome {
    /// The key was unset and is now seeded with `entries_seeded` entries.
    Seeded { entries_seeded: usize },
    /// The key was already set on a prior boot; runtime contents preserved.
    AlreadySeeded,
    /// Seed skipped because there is no fallback to seed from (relay-urls when
    /// `federation.enabled = false`). Not a failure.
    SkippedNoFallback,
}

/// True when the PDS is in recovery mode (env-gated, mirrors the
/// moderation-mode read override at `aurora_admin.rs`).
fn recovery_active() -> bool {
    std::env::var(RECOVERY_MODE_ENV)
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
}

pub(crate) fn guard_recovery() -> FedResult<()> {
    if recovery_active() {
        Err(FedPeerError::RecoveryMode)
    } else {
        Ok(())
    }
}

/// Phase D (#354 / addendum §A6) — refuse federation-policy mutations when a
/// boot-seed failed. Called first at each of the 8 mutation XRPC handlers
/// (before superadmin/recovery/validation), so no state mutates while the
/// substrate is in the boot-seed-failure refusal state.
pub(crate) fn guard_boot_seed(ctx: &AppContext) -> FedResult<()> {
    use std::sync::atomic::Ordering;
    if ctx.boot_seed_failed.load(Ordering::Acquire) {
        Err(FedPeerError::BootSeedFailureActive)
    } else {
        Ok(())
    }
}

fn validate_did(did: &str) -> FedResult<()> {
    if did.is_empty() || !did.starts_with("did:") {
        Err(FedPeerError::InvalidDid(did.to_string()))
    } else {
        Ok(())
    }
}

fn validate_https_url(url: &str) -> FedResult<()> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(FedPeerError::InvalidUrl(url.to_string()))
    }
}

/// Read the current allowlist + its exact stored string (the CAS `expected`).
/// Absent row → `(empty, None)` (first-write inserts rather than CASes).
async fn read_allowlist(ctx: &AppContext) -> (Vec<PeerEntry>, Option<String>) {
    let raw = read_runtime_row_value(ctx, FEDERATION_POLICY_PEER_ALLOWLIST_KEY).await;
    let peers = raw
        .as_deref()
        .and_then(|s| serde_json::from_str::<Vec<PeerEntry>>(s).ok())
        .unwrap_or_default();
    (peers, raw)
}

/// Compare-and-swap the allowlist value. When the row exists, value-CAS via
/// `cas_runtime_setting` (UPDATE … WHERE value=expected). When absent
/// (first write), insert-if-absent so a concurrent inserter loses and retries.
async fn write_allowlist(
    ctx: &AppContext,
    expected: Option<&str>,
    new: &str,
    actor: &str,
) -> FedResult<bool> {
    match expected {
        Some(exp) => cas_runtime_setting(ctx, FEDERATION_POLICY_PEER_ALLOWLIST_KEY, exp, new, actor)
            .await
            .map_err(|e| FedPeerError::Internal(e.to_string())),
        None => {
            let res = sqlx::query(
                "INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) \
                 SELECT $1, $2, $3, $4 \
                 WHERE NOT EXISTS (SELECT 1 FROM runtime_settings WHERE key = $1)",
            )
            .bind(FEDERATION_POLICY_PEER_ALLOWLIST_KEY)
            .bind(new)
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(actor)
            .execute(&ctx.account_db)
            .await
            .map_err(|e| FedPeerError::Internal(e.to_string()))?;
            Ok(res.rows_affected() >= 1)
        }
    }
}

/// Emit a federation audit-chain entry (pool-managed, post-mutation).
pub(crate) async fn emit(
    ctx: &AppContext,
    actor: &str,
    action: &str,
    source: &str,
    payload: serde_json::Value,
    rationale: &str,
) -> FedResult<i64> {
    audit_chain::insert_chain_entry_pool(
        &ctx.account_db,
        ctx.config.database.backend,
        AppendEntryParams {
            actor_did: actor,
            action,
            source,
            payload: Some(payload),
            subject: None,
            rationale,
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .map_err(|e| FedPeerError::Internal(e.to_string()))
}

/// `addFederationPeer` — operator-initiated add (SuperAdmin-gated at handler).
pub async fn add_federation_peer(
    ctx: &AppContext,
    operator_did: &str,
    did: &str,
    url: &str,
) -> FedResult<()> {
    guard_recovery()?;
    validate_did(did)?;
    validate_https_url(url)?;
    add_peer_internal(ctx, did, url, operator_did, SOURCE_MANUAL, "manual", None).await
}

/// Phase C auto-accept add (design §3.2). Substrate-driven during a discovery
/// scan: no recovery/SuperAdmin gate, `source="discovery"`, scan_id in payload.
/// Emits its own `peer_added` / `peer_add_aborted`.
pub async fn add_discovered_peer(
    ctx: &AppContext,
    did: &str,
    url: &str,
    scan_id: &str,
) -> FedResult<()> {
    validate_did(did)?;
    validate_https_url(url)?;
    add_peer_internal(ctx, did, url, SYSTEM_DID, SOURCE_DISCOVERY, "discovery", Some(scan_id)).await
}

/// Shared add path: append to the allowlist, CAS-bounded-retry, audit on
/// success. When the DID is also in the pending-discoveries surface, the
/// allowlist add and the pending removal commit atomically in one transaction
/// (design §3.4 cross-key atomicity; Phase A multi-key primary path — the
/// per-key-sequential fallback is NOT used).
async fn add_peer_internal(
    ctx: &AppContext,
    did: &str,
    url: &str,
    actor: &str,
    source: &str,
    base_origin: &str,
    scan_id: Option<&str>,
) -> FedResult<()> {
    for _ in 0..MAX_CAS_RETRIES {
        let (mut peers, peers_expected) = read_allowlist(ctx).await;
        if peers.iter().any(|p| p.did == did) {
            return Err(FedPeerError::DuplicateDid(did.to_string()));
        }
        peers.push(PeerEntry { did: did.to_string(), url: url.to_string() });
        let peers_new = serde_json::to_string(&peers)
            .map_err(|e| FedPeerError::Internal(e.to_string()))?;

        let pending_raw =
            read_runtime_row_value(ctx, FEDERATION_POLICY_PENDING_DISCOVERIES_KEY).await;
        let from_pending = pending_contains(&pending_raw, did);
        let (wrote, origin) = if from_pending {
            let pending_new = remove_pending_did(&pending_raw, did)?;
            let ok = dual_write(
                ctx,
                peers_expected.as_deref(),
                &peers_new,
                pending_raw.as_deref(),
                &pending_new,
                actor,
            )
            .await?;
            // Operator-accept of a pending entry is labelled distinctly; an
            // auto-accept scan keeps "discovery" even when it clears pending.
            let origin = if base_origin == "discovery" { "discovery" } else { "accepted_from_pending" };
            (ok, origin)
        } else {
            (
                write_allowlist(ctx, peers_expected.as_deref(), &peers_new, actor).await?,
                base_origin,
            )
        };
        if wrote {
            let mut payload = serde_json::json!({ "did": did, "url": url, "origin": origin });
            if let Some(sid) = scan_id {
                payload["scan_id"] = serde_json::json!(sid);
            }
            emit(ctx, actor, ACTION_PEER_ADDED, source, payload, "federation peer added").await?;
            return Ok(());
        }
    }
    let mut payload = serde_json::json!({ "did": did, "url": url, "reason": "cas_exhausted" });
    if let Some(sid) = scan_id {
        payload["scan_id"] = serde_json::json!(sid);
    }
    emit(
        ctx,
        actor,
        ACTION_PEER_ADD_ABORTED,
        SOURCE_DIAGNOSTIC,
        payload,
        "federation peer add aborted: CAS exhausted",
    )
    .await?;
    Err(FedPeerError::CasExhausted)
}

/// Whether the pending-discoveries JSON array contains an entry for `did`.
fn pending_contains(raw: &Option<String>, did: &str) -> bool {
    raw.as_deref()
        .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(s).ok())
        .map(|arr| {
            arr.iter()
                .any(|e| e.get("did").and_then(|d| d.as_str()) == Some(did))
        })
        .unwrap_or(false)
}

/// The pending-discoveries array with `did` removed, re-serialized. Operates on
/// generic `Value` to avoid coupling to the discovery module's entry type.
fn remove_pending_did(raw: &Option<String>, did: &str) -> FedResult<String> {
    let arr: Vec<serde_json::Value> = raw
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let filtered: Vec<serde_json::Value> = arr
        .into_iter()
        .filter(|e| e.get("did").and_then(|d| d.as_str()) != Some(did))
        .collect();
    serde_json::to_string(&filtered).map_err(|e| FedPeerError::Internal(e.to_string()))
}

/// Atomic dual-key write: allowlist + pending in one transaction. Both
/// conditional writes must affect a row, else rollback (caller retries).
async fn dual_write(
    ctx: &AppContext,
    peers_expected: Option<&str>,
    peers_new: &str,
    pending_expected: Option<&str>,
    pending_new: &str,
    actor: &str,
) -> FedResult<bool> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut tx = ctx
        .account_db
        .begin()
        .await
        .map_err(|e| FedPeerError::Internal(e.to_string()))?;
    let peers_ok = conditional_write(
        &mut tx,
        FEDERATION_POLICY_PEER_ALLOWLIST_KEY,
        peers_expected,
        peers_new,
        &now,
        actor,
    )
    .await?;
    let pending_ok = conditional_write(
        &mut tx,
        FEDERATION_POLICY_PENDING_DISCOVERIES_KEY,
        pending_expected,
        pending_new,
        &now,
        actor,
    )
    .await?;
    if peers_ok && pending_ok {
        tx.commit()
            .await
            .map_err(|e| FedPeerError::Internal(e.to_string()))?;
        Ok(true)
    } else {
        tx.rollback()
            .await
            .map_err(|e| FedPeerError::Internal(e.to_string()))?;
        Ok(false)
    }
}

/// One conditional value-CAS (or insert-if-absent) inside a transaction.
async fn conditional_write(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    key: &str,
    expected: Option<&str>,
    new: &str,
    now: &str,
    actor: &str,
) -> FedResult<bool> {
    let res = match expected {
        Some(exp) => {
            sqlx::query(
                "UPDATE runtime_settings SET value = $1, last_modified = $2, last_modified_by = $3 \
                 WHERE key = $4 AND value = $5",
            )
            .bind(new)
            .bind(now)
            .bind(actor)
            .bind(key)
            .bind(exp)
            .execute(&mut **tx)
            .await
        }
        None => {
            sqlx::query(
                "INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) \
                 SELECT $1, $2, $3, $4 \
                 WHERE NOT EXISTS (SELECT 1 FROM runtime_settings WHERE key = $1)",
            )
            .bind(key)
            .bind(new)
            .bind(now)
            .bind(actor)
            .execute(&mut **tx)
            .await
        }
    }
    .map_err(|e| FedPeerError::Internal(e.to_string()))?;
    Ok(res.rows_affected() >= 1)
}

/// `removeFederationPeer` — drop a peer, CAS-bounded-retry, audit on success.
pub async fn remove_federation_peer(
    ctx: &AppContext,
    operator_did: &str,
    did: &str,
) -> FedResult<()> {
    guard_recovery()?;

    for _ in 0..MAX_CAS_RETRIES {
        let (peers, expected) = read_allowlist(ctx).await;
        let Some(removed) = peers.iter().find(|p| p.did == did).cloned() else {
            return Err(FedPeerError::NotPresent(did.to_string()));
        };
        let remaining: Vec<PeerEntry> =
            peers.into_iter().filter(|p| p.did != did).collect();
        let new = serde_json::to_string(&remaining)
            .map_err(|e| FedPeerError::Internal(e.to_string()))?;
        if write_allowlist(ctx, expected.as_deref(), &new, operator_did).await? {
            emit(
                ctx,
                operator_did,
                ACTION_PEER_REMOVED,
                SOURCE_MANUAL,
                serde_json::json!({ "did": removed.did, "url": removed.url, "reason": "manual" }),
                "federation peer removed",
            )
            .await?;
            return Ok(());
        }
    }
    emit(
        ctx,
        operator_did,
        ACTION_PEER_REMOVE_ABORTED,
        SOURCE_DIAGNOSTIC,
        serde_json::json!({ "did": did, "reason": "cas_exhausted" }),
        "federation peer remove aborted: CAS exhausted",
    )
    .await?;
    Err(FedPeerError::CasExhausted)
}

/// `modifyFederationPeer` — update a peer's URL, CAS-bounded-retry, audit with
/// a `change_summary` diff on success (§5.1 / design commit 28).
pub async fn modify_federation_peer(
    ctx: &AppContext,
    operator_did: &str,
    did: &str,
    new_url: &str,
) -> FedResult<()> {
    guard_recovery()?;
    validate_https_url(new_url)?;

    for _ in 0..MAX_CAS_RETRIES {
        let (mut peers, expected) = read_allowlist(ctx).await;
        let Some(target) = peers.iter_mut().find(|p| p.did == did) else {
            return Err(FedPeerError::NotPresent(did.to_string()));
        };
        let before = target.url.clone();
        target.url = new_url.to_string();
        let new = serde_json::to_string(&peers)
            .map_err(|e| FedPeerError::Internal(e.to_string()))?;
        if write_allowlist(ctx, expected.as_deref(), &new, operator_did).await? {
            emit(
                ctx,
                operator_did,
                ACTION_PEER_MODIFIED,
                SOURCE_MANUAL,
                serde_json::json!({
                    "did": did,
                    "change_summary": [
                        { "field": "url", "before": before, "after": new_url }
                    ],
                }),
                "federation peer modified",
            )
            .await?;
            return Ok(());
        }
    }
    emit(
        ctx,
        operator_did,
        ACTION_PEER_MODIFY_ABORTED,
        SOURCE_DIAGNOSTIC,
        serde_json::json!({ "did": did, "attempted_url": new_url, "reason": "cas_exhausted" }),
        "federation peer modify aborted: CAS exhausted",
    )
    .await?;
    Err(FedPeerError::CasExhausted)
}

/// Boot-time seed (design §2.4 / Step 1). When `federation.policy.peer-allowlist`
/// is unset, write the parsed `FederationConfig.peer_pds` (even empty → `[]`,
/// so the first CRUD CAS has a row) and emit one `federation.peer_seeded` per
/// peer (`system_diagnostic`). When already set, preserve runtime contents and
/// ignore the static config — the runtime store is the request-time truth (§1).
/// Idempotent across reboots (seed-if-absent). Phase D (#354): returns a typed
/// `SeedOutcome` so `main.rs` can drive the boot-seed-failure flag; a genuine DB
/// error now propagates as `Err` instead of being swallowed.
pub async fn seed_peer_allowlist(ctx: &AppContext) -> Result<SeedOutcome, PdsError> {
    // Already seeded? Leave runtime contents authoritative.
    let exists = sqlx::query("SELECT 1 FROM runtime_settings WHERE key = $1")
        .bind(FEDERATION_POLICY_PEER_ALLOWLIST_KEY)
        .fetch_optional(&ctx.account_db)
        .await?;
    if exists.is_some() {
        return Ok(SeedOutcome::AlreadySeeded);
    }

    let peers: Vec<PeerEntry> = ctx
        .config
        .federation
        .peer_pds
        .iter()
        .map(|p| PeerEntry { did: p.did.clone(), url: p.url.clone() })
        .collect();
    let value = serde_json::to_value(&peers).map_err(|e| PdsError::Internal(e.to_string()))?;
    // Reuse the Phase A multi-key seed-if-absent primitive (single key here).
    crate::federation::trusted_peer_set::seed_federation_policy(
        &ctx.account_db,
        &[(FEDERATION_POLICY_PEER_ALLOWLIST_KEY.to_string(), value)],
        SYSTEM_DID,
    )
    .await?;
    for p in &peers {
        if let Err(e) = emit(
            ctx,
            SYSTEM_DID,
            ACTION_PEER_SEEDED,
            SOURCE_DIAGNOSTIC,
            serde_json::json!({ "did": p.did, "url": p.url }),
            "federation peer seeded from config at boot",
        )
        .await
        {
            tracing::error!(error = ?e, did = %p.did, "peer-allowlist seed: audit emit failed");
        }
    }
    tracing::info!(count = peers.len(), "federation peer-allowlist seeded from config");
    Ok(SeedOutcome::Seeded { entries_seeded: peers.len() })
}

/// Phase D (#354 / addendum §A6) — boot-seed `federation.policy.relay-urls` from
/// `FederationConfig.relay_urls`. Gated on `federation_enabled`: a disabled
/// deployment skips (the empty relay set is its correct steady state); an enabled
/// deployment with no configured relays is a real error (min-1 invariant) that
/// raises the boot-seed-failure flag. Idempotent (seed-if-absent).
pub async fn seed_relay_urls(
    ctx: &AppContext,
    federation_enabled: bool,
) -> Result<SeedOutcome, PdsError> {
    if !federation_enabled {
        return Ok(SeedOutcome::SkippedNoFallback);
    }
    let relays = ctx.config.federation.relay_urls.clone();
    if relays.is_empty() {
        return Err(PdsError::SeedFailedMinimumViolation {
            key: FEDERATION_POLICY_RELAY_URLS_KEY.to_string(),
            reason: "federation enabled but FederationConfig.relay_urls is empty".to_string(),
        });
    }

    let exists = sqlx::query("SELECT 1 FROM runtime_settings WHERE key = $1")
        .bind(FEDERATION_POLICY_RELAY_URLS_KEY)
        .fetch_optional(&ctx.account_db)
        .await?;
    if exists.is_some() {
        return Ok(SeedOutcome::AlreadySeeded);
    }

    let value = serde_json::to_value(&relays).map_err(|e| PdsError::Internal(e.to_string()))?;
    crate::federation::trusted_peer_set::seed_federation_policy(
        &ctx.account_db,
        &[(FEDERATION_POLICY_RELAY_URLS_KEY.to_string(), value)],
        SYSTEM_DID,
    )
    .await?;
    for url in &relays {
        if let Err(e) = emit(
            ctx,
            SYSTEM_DID,
            ACTION_RELAY_SEEDED,
            SOURCE_DIAGNOSTIC,
            serde_json::json!({ "url": url }),
            "federation relay seeded from config at boot",
        )
        .await
        {
            tracing::error!(error = ?e, url = %url, "relay-urls seed: audit emit failed");
        }
    }
    tracing::info!(count = relays.len(), "federation relay-urls seeded from config");
    Ok(SeedOutcome::Seeded { entries_seeded: relays.len() })
}

/// Phase D (#354 / addendum §A6) — boot-seed orchestrator. Runs the four
/// independent seed wrappers in sequence, and if ANY returns `Err`, emits
/// `federation.boot_seed_failed`, sets the `boot_seed_failed` flag, and records
/// the details for the describe surface. Independent seeds: a failure in one key
/// does NOT roll back the others (operators keep partial functionality while
/// diagnosing). Called from `main.rs` after audit-chain init, before serving.
pub async fn run_federation_boot_seed(ctx: &AppContext) {
    use crate::context::BootSeedFailureDetails;
    use std::sync::atomic::Ordering;

    let mut seeded_keys: Vec<String> = Vec::new();
    let mut failed_keys: Vec<String> = Vec::new();
    let mut failure_reasons: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    let mut record = |key: &str, outcome: Result<SeedOutcome, PdsError>| match outcome {
        Ok(SeedOutcome::Seeded { .. }) | Ok(SeedOutcome::AlreadySeeded) => {
            seeded_keys.push(key.to_string());
        }
        Ok(SeedOutcome::SkippedNoFallback) => { /* not seeded, not failed */ }
        Err(e) => {
            failed_keys.push(key.to_string());
            failure_reasons.insert(key.to_string(), e.to_string());
            tracing::error!(key, error = %e, "federation boot-seed failed");
        }
    };

    record(FEDERATION_POLICY_PEER_ALLOWLIST_KEY, seed_peer_allowlist(ctx).await);
    record(
        crate::api::aurora_admin::FEDERATION_POLICY_DISCOVERY_MODE_KEY,
        crate::api::federation_discovery::seed_discovery_mode(ctx).await,
    );
    record(
        FEDERATION_POLICY_PENDING_DISCOVERIES_KEY,
        crate::api::federation_discovery::seed_pending_discoveries(ctx).await,
    );
    record(
        FEDERATION_POLICY_RELAY_URLS_KEY,
        seed_relay_urls(ctx, ctx.config.federation.enabled).await,
    );

    if !failed_keys.is_empty() {
        let _ = emit(
            ctx,
            SYSTEM_DID,
            ACTION_BOOT_SEED_FAILED,
            SOURCE_DIAGNOSTIC,
            serde_json::json!({
                "failed_keys": failed_keys,
                "seeded_keys": seeded_keys,
                "failure_reasons": failure_reasons,
            }),
            "federation boot-seed failed: federation-policy mutations refused until corrected",
        )
        .await;
        ctx.boot_seed_failed.store(true, Ordering::Release);
        *ctx.boot_seed_failure_details.write().await = Some(BootSeedFailureDetails {
            failed_keys,
            seeded_keys,
            failure_reasons,
        });
    }
}

// Shared test fixtures for the federation surface (peers + discovery). Both
// `federation_peers` and `federation_discovery` tests use the same context
// builder + the same `serial()` lock, so the recovery-env-mutating test never
// leaks into a concurrently-running discovery test (one process-wide lock).
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::config::*;
    use tempfile::tempdir;

    pub(crate) async fn create_test_context_with(
        mutate: impl FnOnce(&mut ServerConfig),
    ) -> AppContext {
        build_ctx(tempdir().unwrap().keep(), mutate).await
    }

    // Build a context at an explicit data dir so a "reboot" can open a second
    // context over the same account_db (cross-boot persistence test).
    pub(crate) async fn build_ctx(
        dir: std::path::PathBuf,
        mutate: impl FnOnce(&mut ServerConfig),
    ) -> AppContext {
        let db_path = dir.join("test.db");
        let mut config = ServerConfig {
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
                jwt_secret: "test-secret-key-aurora-federation-peers-x".to_string(),
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
        mutate(&mut config);
        AppContext::new(
            config,
            std::sync::Arc::new(crate::api::registry::RouteRegistry::default()),
        )
        .await
        .unwrap()
    }

    pub(crate) async fn ctx_with_peers(dids: &[(&str, &str)]) -> AppContext {
        create_test_context_with(|c| {
            c.federation.peer_pds = dids
                .iter()
                .map(|(d, u)| PeerPdsConfig { did: d.to_string(), url: u.to_string() })
                .collect();
        })
        .await
    }

    /// `RECOVERY_MODE_ENV` is process-global; the recovery test mutates it and
    /// would otherwise leak into tests running concurrently under cargo's
    /// parallel harness. Every federation test holds this lock
    /// (`let _g = serial().lock().await;`) so they run one-at-a-time. A
    /// `tokio::sync::Mutex` (not `std`) so the guard is held across `.await`
    /// without tripping `await_holding_lock`, and it doesn't poison on panic.
    pub(crate) fn serial() -> &'static tokio::sync::Mutex<()> {
        static SERIAL: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
        SERIAL.get_or_init(|| tokio::sync::Mutex::new(()))
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{build_ctx, ctx_with_peers, serial};
    use super::*;
    use tempfile::tempdir;

    /// Count audit-chain rows for a given action.
    async fn audit_count(ctx: &AppContext, action: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_chain_entry WHERE action = $1")
            .bind(action)
            .fetch_one(&ctx.account_db)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn add_then_trusted_and_audited() {
        let _g = serial().lock().await;
        let ctx = ctx_with_peers(&[]).await;
        add_federation_peer(&ctx, "did:plc:op", "did:plc:a", "https://a.example")
            .await
            .unwrap();
        assert!(ctx.trusted_peers.is_trusted("did:plc:a").await);
        assert_eq!(audit_count(&ctx, ACTION_PEER_ADDED).await, 1);
    }

    #[tokio::test]
    async fn remove_then_untrusted_and_audited() {
        let _g = serial().lock().await;
        let ctx = ctx_with_peers(&[]).await;
        add_federation_peer(&ctx, "did:plc:op", "did:plc:a", "https://a.example")
            .await
            .unwrap();
        remove_federation_peer(&ctx, "did:plc:op", "did:plc:a")
            .await
            .unwrap();
        assert!(!ctx.trusted_peers.is_trusted("did:plc:a").await);
        assert_eq!(audit_count(&ctx, ACTION_PEER_REMOVED).await, 1);
    }

    #[tokio::test]
    async fn modify_updates_url_and_emits_change_summary() {
        let _g = serial().lock().await;
        let ctx = ctx_with_peers(&[]).await;
        add_federation_peer(&ctx, "did:plc:op", "did:plc:a", "https://a.example")
            .await
            .unwrap();
        modify_federation_peer(&ctx, "did:plc:op", "did:plc:a", "https://a2.example")
            .await
            .unwrap();
        let snap = ctx.trusted_peers.snapshot().await;
        assert_eq!(snap.peers[0].url, "https://a2.example");
        assert_eq!(audit_count(&ctx, ACTION_PEER_MODIFIED).await, 1);
    }

    #[tokio::test]
    async fn validation_rejections() {
        let _g = serial().lock().await;
        let ctx = ctx_with_peers(&[]).await;
        assert!(matches!(
            add_federation_peer(&ctx, "did:plc:op", "notadid", "https://a.example").await,
            Err(FedPeerError::InvalidDid(_))
        ));
        assert!(matches!(
            add_federation_peer(&ctx, "did:plc:op", "did:plc:a", "http://a.example").await,
            Err(FedPeerError::InvalidUrl(_))
        ));
        add_federation_peer(&ctx, "did:plc:op", "did:plc:a", "https://a.example")
            .await
            .unwrap();
        assert!(matches!(
            add_federation_peer(&ctx, "did:plc:op", "did:plc:a", "https://dup.example").await,
            Err(FedPeerError::DuplicateDid(_))
        ));
        assert!(matches!(
            remove_federation_peer(&ctx, "did:plc:op", "did:plc:absent").await,
            Err(FedPeerError::NotPresent(_))
        ));
        assert!(matches!(
            modify_federation_peer(&ctx, "did:plc:op", "did:plc:absent", "https://x.example").await,
            Err(FedPeerError::NotPresent(_))
        ));
    }

    #[tokio::test]
    async fn recovery_mode_blocks_all_three() {
        let _g = serial().lock().await;
        let ctx = ctx_with_peers(&[]).await;
        std::env::set_var(RECOVERY_MODE_ENV, "true");
        let a = add_federation_peer(&ctx, "did:plc:op", "did:plc:a", "https://a.example").await;
        let r = remove_federation_peer(&ctx, "did:plc:op", "did:plc:a").await;
        let m = modify_federation_peer(&ctx, "did:plc:op", "did:plc:a", "https://a.example").await;
        std::env::remove_var(RECOVERY_MODE_ENV);
        assert!(matches!(a, Err(FedPeerError::RecoveryMode)));
        assert!(matches!(r, Err(FedPeerError::RecoveryMode)));
        assert!(matches!(m, Err(FedPeerError::RecoveryMode)));
        // Rejected at entry: no state mutation, no audit.
        assert_eq!(audit_count(&ctx, ACTION_PEER_ADDED).await, 0);
    }

    #[tokio::test]
    async fn seed_populates_from_config_with_per_peer_audit() {
        let _g = serial().lock().await;
        let ctx = ctx_with_peers(&[
            ("did:plc:a", "https://a.example"),
            ("did:plc:b", "https://b.example"),
        ])
        .await;
        seed_peer_allowlist(&ctx).await.unwrap();
        let snap = ctx.trusted_peers.snapshot().await;
        assert_eq!(snap.peers.len(), 2);
        assert_eq!(audit_count(&ctx, ACTION_PEER_SEEDED).await, 2);
        // Idempotent: a second seed does not re-write or re-audit.
        seed_peer_allowlist(&ctx).await.unwrap();
        assert_eq!(audit_count(&ctx, ACTION_PEER_SEEDED).await, 2);
    }

    #[tokio::test]
    async fn persists_across_reboot() {
        // §10.2 #2 persistence sanity-check: a peer added via the CRUD path is
        // visible to a fresh AppContext opened over the same account_db (the
        // runtime-settings layer survives "restart").
        let _g = serial().lock().await;
        let dir = tempdir().unwrap().keep();
        let ctx1 = build_ctx(dir.clone(), |_| {}).await;
        add_federation_peer(&ctx1, "did:plc:op", "did:plc:x", "https://x.example")
            .await
            .unwrap();
        drop(ctx1);
        let ctx2 = build_ctx(dir.clone(), |_| {}).await;
        assert!(ctx2.trusted_peers.is_trusted("did:plc:x").await);
    }

    #[tokio::test]
    async fn seed_then_runtime_contents_authoritative_over_config() {
        let _g = serial().lock().await;
        let ctx = ctx_with_peers(&[("did:plc:cfg", "https://cfg.example")]).await;
        seed_peer_allowlist(&ctx).await.unwrap();
        // A runtime mutation diverges from config.
        add_federation_peer(&ctx, "did:plc:op", "did:plc:runtime", "https://rt.example")
            .await
            .unwrap();
        // Re-seed (simulating a reboot with the same config) must NOT clobber.
        seed_peer_allowlist(&ctx).await.unwrap();
        assert!(ctx.trusted_peers.is_trusted("did:plc:runtime").await);
        assert!(ctx.trusted_peers.is_trusted("did:plc:cfg").await);
    }

    // --- Phase D (#354) boot-seed-failure architecture ---

    #[tokio::test]
    async fn seed_relay_urls_enabled_gate() {
        let _g = serial().lock().await;
        // Disabled → skipped, no error (the empty relay set is the steady state).
        let ctx = test_support::create_test_context_with(|c| {
            c.federation.enabled = false;
            c.federation.relay_urls = vec![];
        })
        .await;
        assert_eq!(
            seed_relay_urls(&ctx, false).await.unwrap(),
            SeedOutcome::SkippedNoFallback
        );

        // Enabled but no relays configured → real error (min-1 violation).
        assert!(matches!(
            seed_relay_urls(&ctx, true).await,
            Err(PdsError::SeedFailedMinimumViolation { .. })
        ));
    }

    #[tokio::test]
    async fn boot_seed_failure_sets_flag_and_audit() {
        let _g = serial().lock().await;
        // Federation enabled with NO relays → relay-urls seed fails → flag set.
        let ctx = test_support::create_test_context_with(|c| {
            c.federation.enabled = true;
            c.federation.relay_urls = vec![];
        })
        .await;
        run_federation_boot_seed(&ctx).await;
        use std::sync::atomic::Ordering;
        assert!(ctx.boot_seed_failed.load(Ordering::Acquire));
        let details = ctx.boot_seed_failure_details.read().await.clone().unwrap();
        assert!(details.failed_keys.contains(&FEDERATION_POLICY_RELAY_URLS_KEY.to_string()));
        // The other three keys still seeded (independent-seed robustness).
        assert!(details.seeded_keys.contains(&FEDERATION_POLICY_PEER_ALLOWLIST_KEY.to_string()));
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_chain_entry WHERE action = 'federation.boot_seed_failed'",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(n, 1);
        // The flag refuses operator mutations.
        assert!(matches!(guard_boot_seed(&ctx), Err(FedPeerError::BootSeedFailureActive)));
    }

    #[tokio::test]
    async fn boot_seed_clean_leaves_flag_unset() {
        let _g = serial().lock().await;
        // Federation disabled → no relay-urls failure → flag stays unset.
        let ctx = ctx_with_peers(&[]).await;
        run_federation_boot_seed(&ctx).await;
        use std::sync::atomic::Ordering;
        assert!(!ctx.boot_seed_failed.load(Ordering::Acquire));
        assert!(guard_boot_seed(&ctx).is_ok());
    }
}
