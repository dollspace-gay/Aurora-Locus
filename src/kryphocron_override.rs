//! Per-account kryphocron override store (#316 / design §6.6.2 item 4).
//!
//! SuperAdmin per-account exceptions to deployment-wide kryphocron policy. Recon
//! (docs/internal/v09/v09_per_account_override_recon.md) pared the surface to the
//! design's two fields — rate-limit exemption + a capability-issuance block
//! ("host-side capability-issuance gate, not a kryphocron substrate concept");
//! per-account cadence is incoherent with the deployment-wide Laquna slug.
//!
//! Tri-state per field: `None` = unset (use the deployment default), `Some(true
//! | false)` = explicit. Stored as nullable INTEGER (the sqlx::Any dual-backend
//! discipline forbids BOOLEAN). The capability-issuance block is consumed now by
//! the private-tier write path ([`capability_blocked`]); rate-limit exemption is
//! stored until the per-tier kryphocron rate-limit feature (§6.6.2 item 3) lands.

use crate::error::PdsResult;
use sqlx::{AnyPool, Row};

/// One account's override row.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountOverride {
    pub did: String,
    pub rate_limit_exempt: Option<bool>,
    pub capability_issuance: Option<bool>,
    pub last_modified_at: String,
    pub last_modified_by_did: String,
    pub last_modified_rationale: Option<String>,
}

const COLS: &str = "did, rate_limit_exempt, capability_issuance, \
                    last_modified_at, last_modified_by_did, last_modified_rationale";

fn row_to_override(r: sqlx::any::AnyRow) -> AccountOverride {
    AccountOverride {
        did: r.get("did"),
        rate_limit_exempt: r.get::<Option<i64>, _>("rate_limit_exempt").map(|v| v != 0),
        capability_issuance: r.get::<Option<i64>, _>("capability_issuance").map(|v| v != 0),
        last_modified_at: r.get("last_modified_at"),
        last_modified_by_did: r.get("last_modified_by_did"),
        last_modified_rationale: r.get::<Option<String>, _>("last_modified_rationale"),
    }
}

/// Read one account's override row; `None` when no override is set.
pub async fn get_override(pool: &AnyPool, did: &str) -> PdsResult<Option<AccountOverride>> {
    let row = sqlx::query(&format!(
        "SELECT {COLS} FROM kryphocron_account_override WHERE did = $1"
    ))
    .bind(did)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(row_to_override))
}

/// Full-state upsert of an account's override within a caller-provided
/// transaction (DELETE+INSERT for cross-backend portability; `None` fields
/// store NULL = "use the deployment default"). The audited handler
/// (`set_account_override`) wraps this and the chain write in one transaction so
/// the override and its audit entry land atomically; tests drive it with their
/// own tx.
pub async fn upsert_override_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    did: &str,
    rate_limit_exempt: Option<bool>,
    capability_issuance: Option<bool>,
    actor_did: &str,
    rationale: Option<&str>,
    now: &str,
) -> PdsResult<()> {
    let to_int = |b: Option<bool>| b.map(|v| if v { 1i64 } else { 0i64 });
    sqlx::query("DELETE FROM kryphocron_account_override WHERE did = $1")
        .bind(did)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO kryphocron_account_override \
         (did, rate_limit_exempt, capability_issuance, last_modified_at, \
          last_modified_by_did, last_modified_rationale) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(did)
    .bind(to_int(rate_limit_exempt))
    .bind(to_int(capability_issuance))
    .bind(now)
    .bind(actor_did)
    .bind(rationale)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Consumer helper (#316): is this account explicitly blocked from issuing
/// kryphocron capabilities (`capability_issuance == Some(false)`)? Fail-soft —
/// a read error returns `false` (not blocked), so an override-store hiccup can't
/// wedge private-tier writes deployment-wide.
pub async fn capability_blocked(pool: &AnyPool, did: &str) -> bool {
    matches!(get_override(pool, did).await, Ok(Some(o)) if o.capability_issuance == Some(false))
}
