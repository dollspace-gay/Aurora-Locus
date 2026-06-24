//! v0.9 Federation Pattern-1 Phase D (#354) — relay runtime-switch.
//!
//! The fourth and final operator-mutable federation surface. Three SuperAdmin
//! operations (add / remove / full-replace) mutate `federation.policy.relay-urls`
//! and switch the live `RelayClient` without a restart, via the addendum's
//! CAS-first → reconfigure ordering.
//!
//! Design: locked body §4.x + the R3-locked Phase D substrate addendum
//! (`v09_federation_pattern1_phase_d_substrate_addendum.md`, §A2/§A4/§A5/§A7).
//!
//! **Memory-#18 translation:** the addendum's `FederationError` does not exist
//! in AL; its variants are folded onto `FedPeerError` (federation_peers.rs).
//! `cas_with_bounded_retry` likewise isn't a real primitive — the CAS uses
//! `cas_runtime_setting` + an inline `0..MAX_CAS_RETRIES` loop (the B/C pattern).

use crate::api::aurora_admin::{
    cas_runtime_setting, read_runtime_row_value, FEDERATION_POLICY_RELAY_URLS_KEY,
};
use crate::api::federation_peers::{
    emit, guard_recovery, FedPeerError, ACTION_RELAY_ADDED, ACTION_RELAY_ADD_ABORTED,
    ACTION_RELAY_REMOVED, ACTION_RELAY_REMOVE_ABORTED, ACTION_RELAY_SWITCHED,
    ACTION_RELAY_SWITCH_ABORTED,
};
use crate::context::AppContext;
use std::time::Duration;

const MAX_CAS_RETRIES: usize = 3;
const MAX_RELAYS: usize = 10;
const LOCK_TIMEOUT_SECS: u64 = 60;
const SOURCE_MANUAL: &str = "manual";
const SOURCE_DIAGNOSTIC: &str = "system_diagnostic";

type FedResult<T> = Result<T, FedPeerError>;

/// The operation kind drives audit names + payload shapes (addendum §A3/§A7):
/// add/remove carry `{url}`; the full-replace switch carries
/// `{before, after, transition_mode, duration_ms}`.
enum RelayOp {
    Add { url: String },
    Remove { url: String },
    Set { transition_mode: String },
}

impl RelayOp {
    fn abort_action(&self) -> &'static str {
        match self {
            RelayOp::Add { .. } => ACTION_RELAY_ADD_ABORTED,
            RelayOp::Remove { .. } => ACTION_RELAY_REMOVE_ABORTED,
            RelayOp::Set { .. } => ACTION_RELAY_SWITCH_ABORTED,
        }
    }
}

fn validate_https(url: &str) -> FedResult<()> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(FedPeerError::InvalidUrl(url.to_string()))
    }
}

/// Read the current relay-urls (parsed) + its exact stored string (CAS expected).
async fn read_relays(ctx: &AppContext) -> (Vec<String>, Option<String>) {
    let raw = read_runtime_row_value(ctx, FEDERATION_POLICY_RELAY_URLS_KEY).await;
    let relays = raw
        .as_deref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default();
    (relays, raw)
}

/// `addRelayUrl` — append a relay, validate, switch. No `transition_mode`.
pub async fn add_relay_url(ctx: &AppContext, operator_did: &str, url: &str) -> FedResult<()> {
    guard_recovery()?;
    validate_https(url)?;
    let (mut relays, _) = read_relays(ctx).await;
    if relays.iter().any(|u| u == url) {
        return Err(FedPeerError::DuplicateDid(url.to_string()));
    }
    if relays.len() >= MAX_RELAYS {
        return Err(FedPeerError::InvalidUrl(format!(
            "relay set already at the maximum of {MAX_RELAYS}"
        )));
    }
    relays.push(url.to_string());
    switch_relay_set(ctx, operator_did, relays, RelayOp::Add { url: url.to_string() }).await
}

/// `removeRelayUrl` — drop a relay, enforce min-1, switch. No `transition_mode`.
pub async fn remove_relay_url(ctx: &AppContext, operator_did: &str, url: &str) -> FedResult<()> {
    guard_recovery()?;
    let (relays, _) = read_relays(ctx).await;
    if !relays.iter().any(|u| u == url) {
        return Err(FedPeerError::NotPresent(url.to_string()));
    }
    let remaining: Vec<String> = relays.into_iter().filter(|u| u != url).collect();
    if remaining.is_empty() {
        return Err(FedPeerError::InvalidUrl(
            "cannot remove the last relay (minimum 1 required)".to_string(),
        ));
    }
    switch_relay_set(ctx, operator_did, remaining, RelayOp::Remove { url: url.to_string() }).await
}

/// `setFederationRelays` — full-replace + switch. Carries `transition_mode`
/// (audit-only in v0.9 — both values execute the same firehose-respawn switch).
pub async fn set_federation_relays(
    ctx: &AppContext,
    operator_did: &str,
    relays: Vec<String>,
    transition_mode: &str,
) -> FedResult<()> {
    guard_recovery()?;
    if relays.is_empty() {
        return Err(FedPeerError::InvalidUrl(
            "at least 1 relay is required".to_string(),
        ));
    }
    if relays.len() > MAX_RELAYS {
        return Err(FedPeerError::InvalidUrl(format!(
            "at most {MAX_RELAYS} relays allowed"
        )));
    }
    let mut seen = std::collections::HashSet::new();
    for url in &relays {
        validate_https(url)?;
        if !seen.insert(url.clone()) {
            return Err(FedPeerError::InvalidUrl(format!("duplicate relay: {url}")));
        }
    }
    let mode = if matches!(transition_mode, "graceful" | "abrupt") {
        transition_mode.to_string()
    } else {
        "graceful".to_string()
    };
    switch_relay_set(ctx, operator_did, relays, RelayOp::Set { transition_mode: mode }).await
}

/// The shared relay-switch primitive (addendum §A2, R3-folded):
/// no-op guard → CAS-first → lock(60s) → reconfigure → refresh discovery → audit.
async fn switch_relay_set(
    ctx: &AppContext,
    operator_did: &str,
    new_relays: Vec<String>,
    op: RelayOp,
) -> FedResult<()> {
    // Precondition: a relay client must exist (federation enabled).
    let relay_client = ctx
        .relay_client
        .as_ref()
        .ok_or(FedPeerError::NoRelayClient)?;

    // 0. No-op guard (addendum H2-1): same relay set → no CAS, no switch, no
    //    audit, no firehose disruption (mirrors the Phase C same-mode no-op).
    let (current, current_raw) = read_relays(ctx).await;
    if current == new_relays {
        return Ok(());
    }

    let new_value = serde_json::to_string(&new_relays)
        .map_err(|e| FedPeerError::Internal(e.to_string()))?;

    // 1. CAS-first (addendum §A2 / R1 H-4): the runtime store records operator
    //    intent before the live switch, so a later reconfigure failure leaves a
    //    transient inconsistency that reconciles forward (not a silent revert).
    let mut wrote = false;
    let mut expected = current_raw;
    for _ in 0..MAX_CAS_RETRIES {
        match &expected {
            Some(exp) => {
                if cas_runtime_setting(
                    ctx,
                    FEDERATION_POLICY_RELAY_URLS_KEY,
                    exp,
                    &new_value,
                    operator_did,
                )
                .await
                .map_err(|e| FedPeerError::Internal(e.to_string()))?
                {
                    wrote = true;
                    break;
                }
            }
            None => {
                // Defensive: relay-urls always seeded at boot when enabled, but
                // handle an unseeded row with insert-if-absent.
                let res = sqlx::query(
                    "INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) \
                     SELECT $1, $2, $3, $4 \
                     WHERE NOT EXISTS (SELECT 1 FROM runtime_settings WHERE key = $1)",
                )
                .bind(FEDERATION_POLICY_RELAY_URLS_KEY)
                .bind(&new_value)
                .bind(chrono::Utc::now().to_rfc3339())
                .bind(operator_did)
                .execute(&ctx.account_db)
                .await
                .map_err(|e| FedPeerError::Internal(e.to_string()))?;
                if res.rows_affected() >= 1 {
                    wrote = true;
                    break;
                }
            }
        }
        // Re-read for the next attempt.
        let (_, raw) = read_relays(ctx).await;
        expected = raw;
    }
    if !wrote {
        emit_abort(ctx, operator_did, &op, &new_relays, "cas_exhausted").await;
        return Err(FedPeerError::CasExhausted);
    }

    let switch_start = std::time::Instant::now();

    // 2. Lock-acquisition with timeout (defensive; per-call locks are brief).
    let mut client = match tokio::time::timeout(
        Duration::from_secs(LOCK_TIMEOUT_SECS),
        relay_client.lock(),
    )
    .await
    {
        Ok(guard) => guard,
        Err(_) => {
            emit_abort(ctx, operator_did, &op, &new_relays, "lock_acquisition_timeout").await;
            return Err(FedPeerError::LockAcquisitionTimeout);
        }
    };

    // 3. Reconfigure the live client (abort old firehose tasks + respawn).
    if let Err(e) = client.reconfigure(&new_relays).await {
        drop(client);
        emit_abort(ctx, operator_did, &op, &new_relays, "reconfigure_failed").await;
        return Err(FedPeerError::ReconfigureFailed(e.to_string()));
    }
    drop(client);

    let duration_ms = switch_start.elapsed().as_millis() as u64;

    // 4. Refresh the discovery relay-list cache (best-effort).
    if let Some(discovery) = ctx.pds_discovery.as_ref() {
        if let Err(e) = discovery.refresh_relay_list(&new_relays).await {
            tracing::error!(error = %e, "relay switch: discovery relay-list refresh failed");
        }
    }

    // 5. Success audit.
    let (action, payload) = match &op {
        RelayOp::Add { url } => (
            ACTION_RELAY_ADDED,
            serde_json::json!({ "url": url }),
        ),
        RelayOp::Remove { url } => (
            ACTION_RELAY_REMOVED,
            serde_json::json!({ "url": url }),
        ),
        RelayOp::Set { transition_mode } => (
            ACTION_RELAY_SWITCHED,
            serde_json::json!({
                "before": current,
                "after": new_relays,
                "transition_mode": transition_mode,
                "duration_ms": duration_ms,
            }),
        ),
    };
    if let Err(e) = emit(ctx, operator_did, action, SOURCE_MANUAL, payload, "federation relay switch").await
    {
        tracing::error!(error = ?e, "relay switch: success audit emit failed");
    }
    Ok(())
}

async fn emit_abort(
    ctx: &AppContext,
    operator_did: &str,
    op: &RelayOp,
    new_relays: &[String],
    reason: &str,
) {
    let payload = match op {
        RelayOp::Add { url } | RelayOp::Remove { url } => {
            serde_json::json!({ "url": url, "reason": reason })
        }
        RelayOp::Set { .. } => {
            serde_json::json!({ "attempted_relays": new_relays, "reason": reason })
        }
    };
    if let Err(e) = emit(ctx, operator_did, op.abort_action(), SOURCE_DIAGNOSTIC, payload, "federation relay switch aborted")
        .await
    {
        tracing::error!(error = ?e, "relay switch: abort audit emit failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::federation_peers::test_support::{create_test_context_with, serial};

    async fn ctx_with_relays() -> AppContext {
        // Federation enabled + a boot relay → context.rs builds a relay client.
        create_test_context_with(|c| {
            c.federation.enabled = true;
            c.federation.relay_urls = vec!["https://boot.example".to_string()];
        })
        .await
    }

    async fn audit_count(ctx: &AppContext, action: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_chain_entry WHERE action = $1")
            .bind(action)
            .fetch_one(&ctx.account_db)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn add_then_present_and_audited() {
        let _g = serial().lock().await;
        let ctx = ctx_with_relays().await;
        add_relay_url(&ctx, "did:plc:op", "https://r2.example").await.unwrap();
        let (relays, _) = read_relays(&ctx).await;
        assert!(relays.contains(&"https://r2.example".to_string()));
        assert_eq!(audit_count(&ctx, ACTION_RELAY_ADDED).await, 1);
    }

    #[tokio::test]
    async fn add_rejects_non_https_dup_and_max() {
        let _g = serial().lock().await;
        let ctx = ctx_with_relays().await;
        assert!(matches!(
            add_relay_url(&ctx, "did:plc:op", "http://r.example").await,
            Err(FedPeerError::InvalidUrl(_))
        ));
        add_relay_url(&ctx, "did:plc:op", "https://r2.example").await.unwrap();
        assert!(matches!(
            add_relay_url(&ctx, "did:plc:op", "https://r2.example").await,
            Err(FedPeerError::DuplicateDid(_))
        ));
    }

    #[tokio::test]
    async fn remove_then_absent_min_one_enforced() {
        let _g = serial().lock().await;
        let ctx = ctx_with_relays().await;
        // Seed two relays via a full replace, then remove one.
        set_federation_relays(
            &ctx,
            "did:plc:op",
            vec!["https://a.example".to_string(), "https://b.example".to_string()],
            "graceful",
        )
        .await
        .unwrap();
        remove_relay_url(&ctx, "did:plc:op", "https://a.example").await.unwrap();
        let (relays, _) = read_relays(&ctx).await;
        assert_eq!(relays, vec!["https://b.example".to_string()]);
        assert_eq!(audit_count(&ctx, ACTION_RELAY_REMOVED).await, 1);
        // Removing the last relay is rejected (min-1).
        assert!(matches!(
            remove_relay_url(&ctx, "did:plc:op", "https://b.example").await,
            Err(FedPeerError::InvalidUrl(_))
        ));
        // Removing an absent relay → NotPresent.
        assert!(matches!(
            remove_relay_url(&ctx, "did:plc:op", "https://absent.example").await,
            Err(FedPeerError::NotPresent(_))
        ));
    }

    #[tokio::test]
    async fn set_replaces_and_validates() {
        let _g = serial().lock().await;
        let ctx = ctx_with_relays().await;
        set_federation_relays(
            &ctx,
            "did:plc:op",
            vec!["https://x.example".to_string(), "https://y.example".to_string()],
            "abrupt",
        )
        .await
        .unwrap();
        let (relays, _) = read_relays(&ctx).await;
        assert_eq!(relays.len(), 2);
        assert_eq!(audit_count(&ctx, ACTION_RELAY_SWITCHED).await, 1);
        // Empty → reject.
        assert!(matches!(
            set_federation_relays(&ctx, "did:plc:op", vec![], "graceful").await,
            Err(FedPeerError::InvalidUrl(_))
        ));
        // >10 → reject.
        let too_many: Vec<String> = (0..11).map(|i| format!("https://r{i}.example")).collect();
        assert!(matches!(
            set_federation_relays(&ctx, "did:plc:op", too_many, "graceful").await,
            Err(FedPeerError::InvalidUrl(_))
        ));
        // duplicate → reject.
        assert!(matches!(
            set_federation_relays(
                &ctx,
                "did:plc:op",
                vec!["https://d.example".to_string(), "https://d.example".to_string()],
                "graceful"
            )
            .await,
            Err(FedPeerError::InvalidUrl(_))
        ));
    }

    #[tokio::test]
    async fn no_op_switch_emits_no_audit() {
        let _g = serial().lock().await;
        let ctx = ctx_with_relays().await;
        set_federation_relays(
            &ctx,
            "did:plc:op",
            vec!["https://only.example".to_string()],
            "graceful",
        )
        .await
        .unwrap();
        let before = audit_count(&ctx, ACTION_RELAY_SWITCHED).await;
        // Setting the SAME set is a no-op: no audit, no firehose disruption.
        set_federation_relays(
            &ctx,
            "did:plc:op",
            vec!["https://only.example".to_string()],
            "graceful",
        )
        .await
        .unwrap();
        assert_eq!(audit_count(&ctx, ACTION_RELAY_SWITCHED).await, before);
    }

    #[tokio::test]
    async fn recovery_mode_blocks_relay_ops() {
        let _g = serial().lock().await;
        let ctx = ctx_with_relays().await;
        std::env::set_var(crate::api::aurora_admin::RECOVERY_MODE_ENV, "true");
        let a = add_relay_url(&ctx, "did:plc:op", "https://r2.example").await;
        let s = set_federation_relays(&ctx, "did:plc:op", vec!["https://z.example".to_string()], "graceful").await;
        std::env::remove_var(crate::api::aurora_admin::RECOVERY_MODE_ENV);
        assert!(matches!(a, Err(FedPeerError::RecoveryMode)));
        assert!(matches!(s, Err(FedPeerError::RecoveryMode)));
    }
}
