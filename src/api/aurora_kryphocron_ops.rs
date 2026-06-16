//! v0.9 Arc D (#225) — operator read XRPC for the Kryphocron domain.
//!
//! Ten `tools.aurora.ops.kryphocron.*` handlers that wrap the released
//! kryphocron 0.3.1 substrate + Aurora-Locus's own host-side state (#223's
//! standard rotation oracle, #224's rewrite-on-rotate job + bookkeeping files,
//! the repo store) into the operator-facing reads the v0.9 Kryphocron domain
//! pages consume (§6.4.1–§6.4.4, §6.5). Kryphocron is a library, not a
//! transport — every endpoint here is Aurora-Locus-authored XRPC; the substrate
//! ships none of them (design §6.8 / §7.2.1).
//!
//! ## The ten endpoints
//!
//! | Endpoint | Page (§) | Role |
//! |---|---|---|
//! | `getSubstrateInfo` | Overview (§6.4.1) | Moderator+ |
//! | `getTierStats` | Overview, Tier activity (§6.4.4) | Moderator+ |
//! | `getOracleActivity` | Overview (§6.4.1, stub) | Moderator+ |
//! | `getRotationStatus` | Overview, Laquna (§6.4.2) | Moderator+ |
//! | `getRotationProgress` | Laquna (§6.4.2) | Admin+ |
//! | `cancelRotation` | Laquna (§6.4.2) | Admin+ |
//! | `listRotations` | Rotation history (§6.4.2.1) | Admin+ |
//! | `getAudienceAggregate` | Audience aggregate (§6.4.3) | Moderator+ |
//! | `listAudiences` (account-filter) | Account drawer (§6.5) | Moderator+ |
//! | `getBlockCascadeImpact` (account-filter) | Account drawer (§6.5) | Moderator+ |
//!
//! ## Role gating
//!
//! All ten live under `tools.aurora.ops.*`, whose namespace scope already
//! gates server-tier access; `AdminAuthContext` resolves to any admin role
//! (Moderator+). The design's per-page split (§6.4.x) then distinguishes the
//! **Admin+** Laquna-control reads (`getRotationProgress`, `cancelRotation`,
//! `listRotations`) from the **Moderator+** observability reads — enforced
//! in-handler via [`require_admin`] (the `getRuntimeSetting` precedent), so the
//! backend honors the matrix rather than leaving it to the frontend alone.
//! `getRotationStatus` is Moderator+ (it backs the Overview's rotation card as
//! well as Laquna).
//!
//! ## What is stub-gated today
//!
//! - `getOracleActivity` — Aurora-Locus's standard oracle ships no consultation
//!   instrumentation (kryphocron exposes no oracle-internal metrics by design,
//!   §6.4.1 note); the endpoint returns an explicit `instrumented: false` shape.
//! - `getBlockCascadeImpact` — reads `block-cascade.log`, which the block
//!   handler does not yet write (post-Arc-2 work); a missing log reads as a
//!   zero-impact `available: false` shape.
//! - `listRotations` cadence-organic track — reads `rotation-history.log`,
//!   which #223's oracle does not yet append to; the operator-triggered track
//!   (from #224's `rewrite-history.log`) is fully populated, the cadence-organic
//!   track is empty until the oracle write-side lands (tracked separately).

use std::collections::BTreeMap;
use std::time::SystemTime;

use axum::http::StatusCode;
use axum::{extract::Query, extract::State, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::kryphocron_endpoints::{NSID_AUDIENCE, NSID_POST_PRIVATE};
use crate::auth::AdminAuthContext;
use crate::context::AppContext;
use crate::error::PdsResult;

/// The structured error shape these handlers return (mirrors `trigger_rotation`).
type ApiErr = (StatusCode, Json<serde_json::Value>);

/// Accounts fetched per `list_accounts` page during aggregate walks.
const ACCOUNT_PAGE: i64 = 200;
/// Records fetched per `list_records` page during per-account walks.
const RECORD_PAGE: i64 = 500;
/// `getTierStats` time-series window (days), per design §6.4.4.
const TIER_WINDOW_DAYS: i64 = 30;
/// Runtime-settings key for the deployment slug-rotation cadence (§6.4.2).
const CADENCE_KEY: &str = "kryphocron.laquna.rotation-cadence";

// ============================================================================
// Shared helpers
// ============================================================================

/// `kryphocron is not enabled on this deployment` — the 400 every endpoint
/// returns when the Arc-D substrate Arcs are absent (mirrors `trigger_rotation`).
fn kryphocron_disabled() -> ApiErr {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": "KryphocronDisabled",
            "message": "kryphocron is not enabled on this deployment",
        })),
    )
}

/// Enforce the Admin+ floor for Laquna-control / rotation-history reads
/// (design §6.4.2 / §6.4.2.1). `AdminAuthContext` already establishes
/// Moderator+; this rejects a Moderator reaching for an Admin-tier endpoint.
fn require_admin(auth: &AdminAuthContext) -> Result<(), ApiErr> {
    use crate::admin::roles::Role;
    if auth.role.can_act_as(Role::Admin) {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "InsufficientRole",
                "message": format!(
                    "endpoint requires Admin+ role; caller has {:?}",
                    auth.role
                ),
            })),
        ))
    }
}

/// Map an internal error to a 500 with a stable error code.
fn internal(e: impl std::fmt::Display) -> ApiErr {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "InternalError", "message": e.to_string() })),
    )
}

/// Format a `SystemTime` as RFC3339 (the convention across Aurora-Locus's
/// time-bearing XRPC responses).
fn rfc3339(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()
}

/// Format unix-milliseconds (the bookkeeping-file timestamp encoding) as
/// RFC3339, or `None` if the value is out of range.
fn ms_to_rfc3339(ms: u128) -> Option<String> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms as i64).map(|d| d.to_rfc3339())
}

/// The deployment cadence string from runtime settings (`daily` default).
async fn read_cadence_setting(ctx: &AppContext) -> String {
    let row = sqlx::query_scalar::<_, String>(
        "SELECT value FROM runtime_settings WHERE key = $1",
    )
    .bind(CADENCE_KEY)
    .fetch_optional(&ctx.account_db)
    .await
    .ok()
    .flatten();
    // Stored as a JSON string ("daily") or a bare string; tolerate both.
    match row {
        Some(raw) => serde_json::from_str::<String>(&raw).unwrap_or(raw),
        None => "daily".to_string(),
    }
}

/// Decode a stored record's CBOR block into its JSON value (structural records
/// — `policy.audience` is stored in the clear, no codec involved). `None` if
/// the block is missing or undecodable.
async fn read_record_value(
    ctx: &AppContext,
    did: &str,
    cid: &str,
) -> Option<serde_json::Value> {
    let block = ctx.actor_store.get_block(did, cid).await.ok()??;
    let lex = proto_blue::lex_cbor::decode(&block).ok()?;
    Some(proto_blue::lex_json::lex_to_json(&lex))
}

/// `nsid -> "public" | "private"` from the kryphocron lexicon registry.
fn tier_by_nsid() -> BTreeMap<&'static str, &'static str> {
    kryphocron::KRYPHOCRON_LEXICON_REGISTRY
        .iter()
        .map(|e| {
            let tier = match e.tier {
                kryphocron::Tier::Public => "public",
                kryphocron::Tier::Private => "private",
                _ => "unknown",
            };
            (e.nsid, tier)
        })
        .collect()
}

/// Walk every local account once, returning `nsid -> per-account record counts`
/// (only counts `> 0`, so the distribution is over accounts that hold the
/// NSID). The single corpus walk shared by `getSubstrateInfo` (totals) and
/// `getTierStats` (totals + per-account distribution); the design's
/// "computed Aurora-Locus-side from the host's repo store" (§6.4.4).
async fn walk_nsid_counts(ctx: &AppContext) -> PdsResult<BTreeMap<&'static str, Vec<u64>>> {
    let mut counts: BTreeMap<&'static str, Vec<u64>> = kryphocron::KRYPHOCRON_IMPLEMENTED_NSIDS
        .iter()
        .map(|nsid| (*nsid, Vec::new()))
        .collect();

    let mut cursor: Option<String> = None;
    loop {
        let accounts = ctx
            .account_manager
            .list_accounts(cursor.as_deref(), ACCOUNT_PAGE)
            .await?;
        if accounts.is_empty() {
            break;
        }
        let page_len = accounts.len();
        let next_cursor = accounts.last().map(|a| a.did.clone());

        for account in &accounts {
            for nsid in kryphocron::KRYPHOCRON_IMPLEMENTED_NSIDS {
                // An account without a local repo store (federated/deactivated
                // actor) errors here; skip it rather than fail the aggregate —
                // mirrors the #224 rewrite walk's per-account resilience.
                let c = match ctx.actor_store.count_records(&account.did, nsid).await {
                    Ok(c) => c as u64,
                    Err(_) => continue,
                };
                if c > 0 {
                    if let Some(v) = counts.get_mut(*nsid) {
                        v.push(c);
                    }
                }
            }
        }

        if page_len < ACCOUNT_PAGE as usize {
            break;
        }
        cursor = next_cursor;
    }
    Ok(counts)
}

/// Bucket per-account record counts into a fixed histogram (design §6.4.4
/// "accounts-with-N-records buckets").
fn distribution_buckets(per_account: &[u64]) -> Vec<DistributionBucket> {
    const BUCKETS: &[(&str, u64, u64)] = &[
        ("1", 1, 1),
        ("2-5", 2, 5),
        ("6-20", 6, 20),
        ("21-100", 21, 100),
        ("100+", 101, u64::MAX),
    ];
    BUCKETS
        .iter()
        .map(|(label, lo, hi)| DistributionBucket {
            bucket: (*label).to_string(),
            accounts: per_account.iter().filter(|&&c| c >= *lo && c <= *hi).count() as u64,
        })
        .collect()
}

// ============================================================================
// getSubstrateInfo (§6.4.1) — Moderator+
// ============================================================================

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubstrateInfo {
    /// kryphocron crate version (`kryphocron::VERSION`).
    version: String,
    /// Lexicon codegen / registry identity hash (`KRYPHOCRON_CODEGEN_HASH`).
    lexicon_registry_hash: String,
    /// Active content-codec identity (`hooks.content_codec().codec_id()`).
    codec_id: String,
    /// Active rotation-oracle identity (Aurora-Locus knows what it installed).
    rotation_oracle: String,
    /// Install-time `validate_at_rest_install` outcome. Fail-closed at boot, so
    /// `"ok"` whenever the substrate is running.
    install_validation: String,
    /// Embedded aggregate counts (§6.4.1).
    aggregate_counts: AggregateCounts,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregateCounts {
    audience_records: u64,
    private_post_records: u64,
    public_tier_records: u64,
    private_tier_records: u64,
}

/// `tools.aurora.ops.kryphocron.getSubstrateInfo` — Overview substrate-identity
/// card + embedded aggregate counts (§6.4.1). Moderator+.
pub async fn get_substrate_info(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<SubstrateInfo>, ApiErr> {
    let hooks = ctx.kryphocron_at_rest_hooks.as_ref().ok_or_else(kryphocron_disabled)?;

    let counts = walk_nsid_counts(&ctx).await.map_err(internal)?;
    let tiers = tier_by_nsid();
    let sum = |nsid: &str| counts.get(nsid).map(|v| v.iter().sum::<u64>()).unwrap_or(0);
    let mut public_tier = 0u64;
    let mut private_tier = 0u64;
    for (nsid, per_account) in &counts {
        let total: u64 = per_account.iter().sum();
        match tiers.get(nsid).copied() {
            Some("public") => public_tier += total,
            Some("private") => private_tier += total,
            _ => {}
        }
    }

    Ok(Json(SubstrateInfo {
        version: kryphocron::VERSION.to_string(),
        lexicon_registry_hash: kryphocron::KRYPHOCRON_CODEGEN_HASH.to_string(),
        codec_id: hooks.content_codec().codec_id().to_string(),
        rotation_oracle: crate::kryphocron_rotation::AuroraLocusStandardRotationOracle::IDENTIFIER
            .to_string(),
        install_validation: "ok".to_string(),
        aggregate_counts: AggregateCounts {
            audience_records: sum(NSID_AUDIENCE),
            private_post_records: sum(NSID_POST_PRIVATE),
            public_tier_records: public_tier,
            private_tier_records: private_tier,
        },
    }))
}

// ============================================================================
// getTierStats (§6.4.4) — Moderator+
// ============================================================================

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TierStats {
    window_days: i64,
    tier_totals: TierTotals,
    nsids: Vec<NsidStats>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TierTotals {
    public: u64,
    private: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NsidStats {
    nsid: String,
    tier: String,
    total: u64,
    account_distribution: Vec<DistributionBucket>,
    time_series: Vec<TimeSeriesPoint>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DistributionBucket {
    bucket: String,
    accounts: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TimeSeriesPoint {
    date: String,
    count: u64,
}

/// `tools.aurora.ops.kryphocron.getTierStats` — per-NSID counts, per-account
/// distribution, and 30-day time series across the 8 implemented NSIDs
/// (§6.4.4). Moderator+.
pub async fn get_tier_stats(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<TierStats>, ApiErr> {
    if ctx.kryphocron_at_rest_hooks.is_none() {
        return Err(kryphocron_disabled());
    }
    let counts = walk_nsid_counts(&ctx).await.map_err(internal)?;
    let tiers = tier_by_nsid();
    let series = walk_time_series(&ctx).await.map_err(internal)?;

    let mut public_total = 0u64;
    let mut private_total = 0u64;
    let mut nsids = Vec::with_capacity(counts.len());
    for (nsid, per_account) in &counts {
        let total: u64 = per_account.iter().sum();
        let tier = tiers.get(nsid).copied().unwrap_or("unknown");
        match tier {
            "public" => public_total += total,
            "private" => private_total += total,
            _ => {}
        }
        nsids.push(NsidStats {
            nsid: (*nsid).to_string(),
            tier: tier.to_string(),
            total,
            account_distribution: distribution_buckets(per_account),
            time_series: series.get(nsid).cloned().unwrap_or_default(),
        });
    }

    Ok(Json(TierStats {
        window_days: TIER_WINDOW_DAYS,
        tier_totals: TierTotals {
            public: public_total,
            private: private_total,
        },
        nsids,
    }))
}

/// Walk recent-window record timestamps per NSID and bucket them by UTC day —
/// the bounded input for `getTierStats`'s time series. Only the last
/// [`TIER_WINDOW_DAYS`] of records are read per account/NSID.
async fn walk_time_series(
    ctx: &AppContext,
) -> PdsResult<BTreeMap<&'static str, Vec<TimeSeriesPoint>>> {
    let since = chrono::Utc::now() - chrono::Duration::days(TIER_WINDOW_DAYS);
    // nsid -> (yyyy-mm-dd -> count)
    let mut buckets: BTreeMap<&'static str, BTreeMap<String, u64>> =
        kryphocron::KRYPHOCRON_IMPLEMENTED_NSIDS
            .iter()
            .map(|nsid| (*nsid, BTreeMap::new()))
            .collect();

    let mut cursor: Option<String> = None;
    loop {
        let accounts = ctx
            .account_manager
            .list_accounts(cursor.as_deref(), ACCOUNT_PAGE)
            .await?;
        if accounts.is_empty() {
            break;
        }
        let page_len = accounts.len();
        let next_cursor = accounts.last().map(|a| a.did.clone());

        for account in &accounts {
            for nsid in kryphocron::KRYPHOCRON_IMPLEMENTED_NSIDS {
                // Skip accounts without a queryable store (see `walk_nsid_counts`).
                let Ok(stamps) = ctx
                    .actor_store
                    .list_record_indexed_at_since(&account.did, nsid, since)
                    .await
                else {
                    continue;
                };
                if let Some(day_map) = buckets.get_mut(*nsid) {
                    for ts in stamps {
                        let day = ts.format("%Y-%m-%d").to_string();
                        *day_map.entry(day).or_insert(0) += 1;
                    }
                }
            }
        }

        if page_len < ACCOUNT_PAGE as usize {
            break;
        }
        cursor = next_cursor;
    }

    Ok(buckets
        .into_iter()
        .map(|(nsid, day_map)| {
            let points = day_map
                .into_iter()
                .map(|(date, count)| TimeSeriesPoint { date, count })
                .collect();
            (nsid, points)
        })
        .collect())
}

// ============================================================================
// getOracleActivity (§6.4.1, stub) — Moderator+
// ============================================================================

/// `tools.aurora.ops.kryphocron.getOracleActivity` — oracle consultation
/// counts. Stub-gated (§6.4.1 / §7.2.1): Aurora-Locus's standard oracle ships
/// no consultation instrumentation, and kryphocron exposes no oracle-internal
/// metrics by design, so this returns an explicit `instrumented: false` shape
/// the Overview renders as its "available when …" placeholder. Moderator+.
pub async fn get_oracle_activity(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if ctx.kryphocron_rotation_oracle.is_none() {
        return Err(kryphocron_disabled());
    }
    Ok(Json(json!({
        "instrumented": false,
        "consultations": serde_json::Value::Null,
        "message": "Oracle consultation instrumentation is not installed in this \
                    substrate build; available when Aurora-Locus ships oracle-activity \
                    instrumentation.",
    })))
}

// ============================================================================
// getRotationStatus (§6.4.2) — Moderator+
// ============================================================================

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationStatus {
    /// Active generation mark (opaque; read-only, never triggers a rotation).
    generation_mark: String,
    /// Most recent slug rotation (organic or forced), RFC3339.
    last_slug_rotation: String,
    /// Next scheduled organic rotation (`last + cadence`), or `null` under
    /// manual-only cadence.
    next_scheduled_slug_rotation: Option<String>,
    /// Most recent rewrite-on-rotate completion, RFC3339, or `null` if none.
    last_record_rewrite_completed: Option<String>,
    /// Whether a rewrite-on-rotate job is currently running.
    rewrite_in_progress: bool,
    /// Active deployment cadence policy (`hourly|daily|weekly|manual-only`).
    cadence: String,
}

/// `tools.aurora.ops.kryphocron.getRotationStatus` — current rotation state for
/// the Overview + Laquna cards (§6.4.1 / §6.4.2). Moderator+.
pub async fn get_rotation_status(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<RotationStatus>, ApiErr> {
    let oracle = ctx
        .kryphocron_rotation_oracle
        .as_ref()
        .ok_or_else(kryphocron_disabled)?;

    let cadence_str = read_cadence_setting(&ctx).await;
    let cadence = crate::kryphocron_rotation::Cadence::from_setting(&cadence_str);
    let last_rotation = oracle.last_rotation_at();
    let next_scheduled = if cadence.is_scheduled() {
        last_rotation
            .checked_add(std::time::Duration::from_secs(cadence.as_secs()))
            .map(rfc3339)
    } else {
        None
    };

    let (in_progress, last_completed) = match &ctx.kryphocron_rewrite_job {
        Some(job) => (
            job.progress().running,
            job.last_completed_ms().and_then(ms_to_rfc3339),
        ),
        None => (false, None),
    };

    Ok(Json(RotationStatus {
        generation_mark: oracle.current_mark().to_string(),
        last_slug_rotation: rfc3339(last_rotation),
        next_scheduled_slug_rotation: next_scheduled,
        last_record_rewrite_completed: last_completed,
        rewrite_in_progress: in_progress,
        cadence: cadence.as_setting().to_string(),
    }))
}

// ============================================================================
// getRotationProgress (§6.4.2) — Admin+
// ============================================================================

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationProgress {
    running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    records_processed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    records_rewritten: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_mark: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cancel_requested: Option<bool>,
    /// No total-record count is tracked, so an ETA is never computed
    /// server-side (the frontend estimates from `getTierStats` totals); always
    /// `null` to keep the field present and stable.
    estimated_completion: Option<String>,
}

/// `tools.aurora.ops.kryphocron.getRotationProgress` — in-flight rewrite-on-
/// rotate progress (§6.4.2). When no rewrite is running, returns the
/// equivalent empty shape `{ "running": false }` (the 5s-poll frontend keys on
/// `running`). Admin+.
pub async fn get_rotation_progress(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<RotationProgress>, ApiErr> {
    require_admin(&auth)?;
    let job = ctx
        .kryphocron_rewrite_job
        .as_ref()
        .ok_or_else(kryphocron_disabled)?;
    let p = job.progress();
    if !p.running {
        return Ok(Json(RotationProgress {
            running: false,
            records_processed: None,
            records_rewritten: None,
            generation_mark: None,
            started_at: None,
            cancel_requested: None,
            estimated_completion: None,
        }));
    }
    Ok(Json(RotationProgress {
        running: true,
        records_processed: Some(p.processed),
        records_rewritten: Some(p.rewritten),
        generation_mark: p.generation_mark,
        started_at: p.started_at.map(rfc3339),
        cancel_requested: Some(p.cancel_requested),
        estimated_completion: None,
    }))
}

// ============================================================================
// cancelRotation (§6.4.2) — Admin+
// ============================================================================

/// `tools.aurora.ops.kryphocron.cancelRotation` — request cancellation of the
/// in-flight rewrite-on-rotate job (§6.4.2). The job observes the flag at its
/// next account/batch boundary and terminates cleanly (recording an `aborted`
/// entry in `rewrite-history.log`). Returns 200 + the live progress snapshot at
/// request time, or 409 if no rewrite is in flight to cancel. Admin+.
pub async fn cancel_rotation(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, ApiErr> {
    require_admin(&auth)?;
    let job = ctx
        .kryphocron_rewrite_job
        .as_ref()
        .ok_or_else(kryphocron_disabled)?;
    if !job.request_cancel() {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({
                "error": "NoRotationInProgress",
                "message": "no rewrite-on-rotate job is in progress to cancel",
            })),
        ));
    }
    let p = job.progress();
    Ok(Json(json!({
        "status": "cancel-requested",
        "recordsProcessed": p.processed,
        "recordsRewritten": p.rewritten,
        "generationMark": p.generation_mark,
        "startedAt": p.started_at.map(rfc3339),
    })))
}

// ============================================================================
// listRotations (§6.4.2.1) — Admin+
// ============================================================================

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationEntry {
    /// `"operator-triggered"` | `"cadence-organic"`.
    kind: String,
    /// Event time, RFC3339.
    at: String,
    /// Generation mark (opaque), if recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_mark: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    records_processed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    records_rewritten: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
}

/// `tools.aurora.ops.kryphocron.listRotations` — the two-track rotation history
/// (§6.4.2.1): operator-triggered rewrites (from #224's `rewrite-history.log`)
/// merged with cadence-organic slug rotations (from `rotation-history.log`,
/// which the oracle does not yet write — that track is empty until its
/// write-side lands). Single chronologically-sorted array; for equal
/// timestamps, operator-triggered sorts first (design §13.2). Admin+.
pub async fn list_rotations(
    State(ctx): State<AppContext>,
    auth: AdminAuthContext,
) -> Result<Json<serde_json::Value>, ApiErr> {
    require_admin(&auth)?;
    let job = ctx
        .kryphocron_rewrite_job
        .as_ref()
        .ok_or_else(kryphocron_disabled)?;

    // Operator-triggered track: one entry per `terminated` line (a completed
    // rewrite run, carrying its final counts + outcome + duration).
    let mut entries: Vec<(u128, u8, RotationEntry)> = job
        .history()
        .into_iter()
        .filter(|e| e.kind == "terminated")
        .filter_map(|e| {
            let at = ms_to_rfc3339(e.at_ms)?;
            Some((
                e.at_ms,
                0, // operator-triggered sorts first on tie
                RotationEntry {
                    kind: "operator-triggered".to_string(),
                    at,
                    generation_mark: e.generation,
                    records_processed: Some(e.processed),
                    records_rewritten: Some(e.rewritten),
                    outcome: e.outcome,
                    duration_ms: e.duration_ms,
                },
            ))
        })
        .collect();

    // Cadence-organic track: from rotation-history.log if present (empty today).
    for (at_ms, gen) in read_rotation_history(&ctx) {
        if let Some(at) = ms_to_rfc3339(at_ms) {
            entries.push((
                at_ms,
                1,
                RotationEntry {
                    kind: "cadence-organic".to_string(),
                    at,
                    generation_mark: gen,
                    records_processed: None,
                    records_rewritten: None,
                    outcome: None,
                    duration_ms: None,
                },
            ));
        }
    }

    // Chronological; operator-triggered (rank 0) before cadence-organic (1) on tie.
    entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let rotations: Vec<RotationEntry> = entries.into_iter().map(|(_, _, e)| e).collect();
    Ok(Json(json!({ "rotations": rotations })))
}

/// Read the cadence-organic `rotation-history.log` (one record per organic slug
/// rotation: unix-millis `at` + opaque `generation`). The oracle does not yet
/// write this file, so today this reads empty; the reader is shaped for the
/// JSONL the oracle write-side will append (tracked separately). Returns
/// `(at_ms, generation)` pairs.
fn read_rotation_history(ctx: &AppContext) -> Vec<(u128, Option<String>)> {
    let path = ctx
        .config
        .storage
        .data_directory
        .join("aurora-locus")
        .join("rotation-history.log");
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    contents
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            let at = v.get("at").and_then(|a| a.as_u64())? as u128;
            let gen = v.get("generation").and_then(|g| g.as_str()).map(str::to_string);
            Some((at, gen))
        })
        .collect()
}

// ============================================================================
// getAudienceAggregate (§6.4.3) — Moderator+
// ============================================================================

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudienceAggregate {
    total_audience_records: u64,
    accounts_with_audiences: u64,
    average_audiences_per_account: f64,
    /// Distribution over the 5-mode enum + `unset` for records with no `mode`.
    mode_distribution: ModeDistribution,
    /// Member-count buckets for `list`-mode audiences (non-list contribute 0).
    list_size_histogram: Vec<DistributionBucket>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModeDistribution {
    list: u64,
    everyone: u64,
    followers: u64,
    following: u64,
    nobody: u64,
    unset: u64,
}

/// `tools.aurora.ops.kryphocron.getAudienceAggregate` — deployment-wide
/// `policy.audience` statistics (§6.4.3): totals, 5-mode distribution, and a
/// `list`-mode member-size histogram. Audience records are structural (read in
/// the clear, no decode). Moderator+.
pub async fn get_audience_aggregate(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
) -> Result<Json<AudienceAggregate>, ApiErr> {
    if ctx.kryphocron_at_rest_hooks.is_none() {
        return Err(kryphocron_disabled());
    }

    let mut total = 0u64;
    let mut accounts_with = 0u64;
    let mut modes = ModeDistribution::default();
    let mut list_sizes: Vec<u64> = Vec::new();

    let mut cursor: Option<String> = None;
    loop {
        let accounts = ctx
            .account_manager
            .list_accounts(cursor.as_deref(), ACCOUNT_PAGE)
            .await
            .map_err(internal)?;
        if accounts.is_empty() {
            break;
        }
        let page_len = accounts.len();
        let next_cursor = accounts.last().map(|a| a.did.clone());

        for account in &accounts {
            let mut account_has = false;
            let mut record_cursor: Option<String> = None;
            loop {
                // Skip accounts without a queryable store rather than fail the
                // deployment-wide aggregate (see `walk_nsid_counts`).
                let Ok(records) = ctx
                    .actor_store
                    .list_records(&account.did, NSID_AUDIENCE, RECORD_PAGE, record_cursor.as_deref())
                    .await
                else {
                    break;
                };
                if records.is_empty() {
                    break;
                }
                let rec_page = records.len();
                let last_rkey = records.last().map(|r| r.rkey.clone());
                for rec in &records {
                    total += 1;
                    account_has = true;
                    if let Some(value) = read_record_value(&ctx, &account.did, &rec.cid).await {
                        tally_audience(&value, &mut modes, &mut list_sizes);
                    } else {
                        modes.unset += 1;
                    }
                }
                if rec_page < RECORD_PAGE as usize {
                    break;
                }
                record_cursor = last_rkey;
            }
            if account_has {
                accounts_with += 1;
            }
        }

        if page_len < ACCOUNT_PAGE as usize {
            break;
        }
        cursor = next_cursor;
    }

    let avg = if accounts_with > 0 {
        total as f64 / accounts_with as f64
    } else {
        0.0
    };

    Ok(Json(AudienceAggregate {
        total_audience_records: total,
        accounts_with_audiences: accounts_with,
        average_audiences_per_account: avg,
        mode_distribution: modes,
        list_size_histogram: distribution_buckets(&list_sizes),
    }))
}

/// Tally one decoded `policy.audience` record into the mode distribution and
/// (for `list` mode) the member-size sample. Per kryphocron-lexicons 0.3.0 the
/// `members` array binds only under `mode == "list"`.
fn tally_audience(value: &serde_json::Value, modes: &mut ModeDistribution, list_sizes: &mut Vec<u64>) {
    match value.get("mode").and_then(|m| m.as_str()) {
        Some("list") => {
            modes.list += 1;
            let size = value
                .get("members")
                .and_then(|m| m.as_array())
                .map(|a| a.len() as u64)
                .unwrap_or(0);
            list_sizes.push(size);
        }
        Some("everyone") => modes.everyone += 1,
        Some("followers") => modes.followers += 1,
        Some("following") => modes.following += 1,
        Some("nobody") => modes.nobody += 1,
        _ => modes.unset += 1,
    }
}

// ============================================================================
// listAudiences (§6.5) — Moderator+, account-filter
// ============================================================================

#[derive(Deserialize)]
pub struct AccountFilter {
    pub account: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudienceListing {
    account: String,
    audiences: Vec<AudienceEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudienceEntry {
    uri: String,
    rkey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    /// Member count for `list`-mode audiences. Contents are never surfaced
    /// (design §6.5 privacy rule) — count only.
    #[serde(skip_serializing_if = "Option::is_none")]
    member_count: Option<u64>,
    indexed_at: String,
}

/// `tools.aurora.ops.kryphocron.listAudiences?account=<did>` — per-account
/// audience record list for the Account-detail drawer (§6.5). Read-only;
/// surfaces name/mode/member-count + timestamps, never the `members` contents.
/// Moderator+.
pub async fn list_audiences(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<AccountFilter>,
) -> Result<Json<AudienceListing>, ApiErr> {
    if ctx.kryphocron_at_rest_hooks.is_none() {
        return Err(kryphocron_disabled());
    }
    let did = params.account;
    // Reject an unknown account so the drawer can distinguish "no audiences"
    // from "no such account". `get_account` errors with `NotFound` for an
    // unknown DID; surface that as a 404 and any other error as a 500.
    if let Err(e) = ctx.account_manager.get_account(&did).await {
        return match e {
            crate::error::PdsError::NotFound(_) => Err((
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "AccountNotFound", "message": format!("no account {did}") })),
            )),
            other => Err(internal(other)),
        };
    }

    let mut audiences = Vec::new();
    let mut record_cursor: Option<String> = None;
    loop {
        let records = ctx
            .actor_store
            .list_records(&did, NSID_AUDIENCE, RECORD_PAGE, record_cursor.as_deref())
            .await
            .map_err(internal)?;
        if records.is_empty() {
            break;
        }
        let rec_page = records.len();
        let last_rkey = records.last().map(|r| r.rkey.clone());
        for rec in &records {
            let value = read_record_value(&ctx, &did, &rec.cid).await;
            let name = value
                .as_ref()
                .and_then(|v| v.get("name"))
                .and_then(|n| n.as_str())
                .map(str::to_string);
            let mode = value
                .as_ref()
                .and_then(|v| v.get("mode"))
                .and_then(|m| m.as_str())
                .map(str::to_string);
            let member_count = if mode.as_deref() == Some("list") {
                value
                    .as_ref()
                    .and_then(|v| v.get("members"))
                    .and_then(|m| m.as_array())
                    .map(|a| a.len() as u64)
            } else {
                None
            };
            audiences.push(AudienceEntry {
                uri: rec.uri.clone(),
                rkey: rec.rkey.clone(),
                name,
                mode,
                member_count,
                indexed_at: rec.indexed_at.to_rfc3339(),
            });
        }
        if rec_page < RECORD_PAGE as usize {
            break;
        }
        record_cursor = last_rkey;
    }

    Ok(Json(AudienceListing { account: did, audiences }))
}

// ============================================================================
// getBlockCascadeImpact (§6.5) — Moderator+, account-filter
// ============================================================================

/// `tools.aurora.ops.kryphocron.getBlockCascadeImpact?account=<did>` — per-
/// account cascade-driven audience-removal counts (§6.5 / §7.2.4), read from
/// `<data-dir>/aurora-locus/block-cascade.log`. The block handler does not yet
/// write this log (post-Arc-2 work), so a missing log reads as a zero-impact
/// `available: false` shape — the drawer's field-level stub gate. Moderator+.
pub async fn get_block_cascade_impact(
    State(ctx): State<AppContext>,
    _auth: AdminAuthContext,
    Query(params): Query<AccountFilter>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if ctx.kryphocron_at_rest_hooks.is_none() {
        return Err(kryphocron_disabled());
    }
    let did = params.account;
    let path = ctx
        .config
        .storage
        .data_directory
        .join("aurora-locus")
        .join("block-cascade.log");
    let removals = match std::fs::read_to_string(&path) {
        Ok(contents) => count_cascade_removals(&contents, &did),
        Err(_) => {
            return Ok(Json(json!({
                "account": did,
                "available": false,
                "cascadeRemovals": 0,
                "message": "Block-cascade bookkeeping is not yet recorded on this deployment.",
            })));
        }
    };
    Ok(Json(json!({
        "account": did,
        "available": true,
        "cascadeRemovals": removals,
    })))
}

/// Count cascade-driven audience removals attributed to `did` from
/// `block-cascade.log` (JSONL; each line records an `account` + a removal). The
/// log format is the block handler's host-side bookkeeping (§7.2.4); this
/// reader is shaped for the `{ "account": <did>, "removed": <n> }` line the
/// write-side will append.
fn count_cascade_removals(contents: &str, did: &str) -> u64 {
    contents
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|v| v.get("account").and_then(|a| a.as_str()) == Some(did))
        .map(|v| v.get("removed").and_then(|r| r.as_u64()).unwrap_or(1))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_buckets_partition_counts() {
        let per_account = vec![1, 1, 3, 10, 50, 200];
        let b = distribution_buckets(&per_account);
        let get = |label: &str| b.iter().find(|x| x.bucket == label).unwrap().accounts;
        assert_eq!(get("1"), 2);
        assert_eq!(get("2-5"), 1);
        assert_eq!(get("6-20"), 1);
        assert_eq!(get("21-100"), 1);
        assert_eq!(get("100+"), 1);
    }

    #[test]
    fn tally_audience_classifies_modes_and_list_size() {
        let mut modes = ModeDistribution::default();
        let mut sizes = Vec::new();
        tally_audience(
            &json!({ "mode": "list", "members": ["a", "b", "c"] }),
            &mut modes,
            &mut sizes,
        );
        tally_audience(&json!({ "mode": "everyone" }), &mut modes, &mut sizes);
        tally_audience(&json!({ "mode": "nobody" }), &mut modes, &mut sizes);
        tally_audience(&json!({ "text": "no mode field" }), &mut modes, &mut sizes);
        assert_eq!(modes.list, 1);
        assert_eq!(modes.everyone, 1);
        assert_eq!(modes.nobody, 1);
        assert_eq!(modes.unset, 1);
        assert_eq!(sizes, vec![3]);
    }

    #[test]
    fn count_cascade_removals_filters_and_sums_by_account() {
        let log = r#"{"account":"did:plc:aaa","removed":2}
{"account":"did:plc:bbb","removed":5}
{"account":"did:plc:aaa","removed":1}
not json
{"account":"did:plc:aaa"}"#;
        // did:plc:aaa: 2 + 1 + (default 1 for the no-`removed` line) = 4.
        assert_eq!(count_cascade_removals(log, "did:plc:aaa"), 4);
        assert_eq!(count_cascade_removals(log, "did:plc:bbb"), 5);
        assert_eq!(count_cascade_removals(log, "did:plc:zzz"), 0);
    }

    #[test]
    fn ms_rfc3339_round_trips_a_known_epoch() {
        // 2021-01-01T00:00:00Z = 1609459200000 ms.
        let s = ms_to_rfc3339(1_609_459_200_000).unwrap();
        assert!(s.starts_with("2021-01-01T00:00:00"));
    }
}
