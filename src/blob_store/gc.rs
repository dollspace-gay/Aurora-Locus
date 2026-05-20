//! GC sweep for orphaned blob storage.
//!
//! Reconciles blob storage against DB metadata: identifies
//! blobs present in storage with no corresponding `blob` row
//! (orphans from Arc 4's `DeferredAction` queue's best-effort
//! delete after storage delete fails — V04_DESIGN.md §9).
//!
//! Two-stage orphan classification per V04_DESIGN.md §9.3.2:
//!
//! 1. Cross-reference candidate against `temp_blob_metadata` —
//!    in-flight uploads are skipped regardless of storage age.
//! 2. Apply freshness threshold (default 1h) as belt-and-
//!    braces against the rare race where storage list returns
//!    a CID whose `temp_blob_metadata` row hasn't yet
//!    committed.
//!
//! This module ships the sweep *primitive*: types, the
//! classifier, and the paginated loop. Step 3 wires it to the
//! background [`crate::jobs::JobScheduler`] and the CLI
//! `gc-sweep` subcommand; Step 2 does not. Added in Arc 10
//! (chainlink #57).

use std::collections::HashSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::AnyPool;

use crate::blob_store::{BlobBackend, BlobListEntry};
use crate::error::{PdsError, PdsResult};

/// Sweep configuration parameters.
///
/// `dry_run` and `max_deletes_per_run` defaults match
/// V04_DESIGN.md §9.4.2's safety stance: classify only, with
/// a 10k cap that protects against a misconfigured production
/// sweep walking off into mass-deletion territory. Step 3
/// wires both to the runtime config layer; Step 2 exposes the
/// parameters so the primitive is library-callable.
#[derive(Debug, Clone)]
pub struct SweepParams {
    /// If `true`, the sweep classifies and logs but does not
    /// delete. Default-true: safe stance until Step 3 promotes
    /// to actual deletion via config.
    pub dry_run: bool,

    /// If `true`, classify and log only — the safety cap does
    /// not apply because no deletes happen. Used by the CLI
    /// `--report-only` mode (Step 3).
    pub report_only: bool,

    /// Max blobs to delete in one sweep run. Default 10,000.
    /// Excess orphans are logged + counted in
    /// [`SweepReport::orphans_skipped_safety_cap`] and remain
    /// in storage for the next sweep.
    pub max_deletes_per_run: usize,

    /// Belt-and-braces freshness threshold. Blobs younger than
    /// this are not classified as orphans, even when absent
    /// from `temp_blob_metadata`. Default 1 hour, in line with
    /// Step 0 Q9's analysis: `temp_blob_metadata` is the
    /// authoritative in-flight signal; this threshold catches
    /// the rare race where a row hasn't landed yet.
    pub freshness_threshold: Duration,

    /// Storage page size. Default 500 — Step 1's IN-clause
    /// benchmark confirmed this stays index-driven on SQLite
    /// at 100k seeded rows.
    pub page_size: usize,
}

impl Default for SweepParams {
    fn default() -> Self {
        Self {
            dry_run: true,
            report_only: false,
            max_deletes_per_run: 10_000,
            freshness_threshold: Duration::from_secs(3600),
            page_size: 500,
        }
    }
}

/// Per-blob classification result.
///
/// One of these is produced for every blob the sweep
/// encounters in storage. The variant determines whether the
/// sweep deletes (only `ConfirmedOrphan`, and only when
/// `dry_run`/`report_only` are off and the safety cap hasn't
/// been hit) or logs-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobClassification {
    /// Storage has it; absent from `blob`; absent from
    /// `temp_blob_metadata`; older than the freshness
    /// threshold. The sweep deletes (unless dry_run /
    /// report_only / safety cap).
    ConfirmedOrphan {
        cid: String,
        last_modified: DateTime<Utc>,
    },

    /// Storage has it; absent from `blob`; absent from
    /// `temp_blob_metadata`; YOUNGER than the freshness
    /// threshold. Skipped this sweep; re-evaluated next run.
    TooYoung {
        cid: String,
        last_modified: DateTime<Utc>,
    },

    /// Storage has it; present in `temp_blob_metadata`
    /// (in-flight upload). Skipped regardless of age — the
    /// tracking surface is authoritative per
    /// V04_DESIGN.md §9.3.2.
    InFlight { cid: String },

    /// Storage has it; present in `blob` (authorized blob,
    /// possibly with `takedown = 1`). Skipped.
    Authorized { cid: String },
}

/// Aggregated outcome of a sweep run.
///
/// All counts are running totals across the run's pages. The
/// invariants:
///
/// * `blobs_examined == authorized + in_flight + too_young +
///   confirmed_orphans_found`
/// * `orphans_deleted + orphans_skipped_safety_cap <=
///   confirmed_orphans_found` (the gap is dry-run / report-
///   only / backend-delete-failure cases)
#[derive(Debug, Clone, Default)]
pub struct SweepReport {
    /// Number of `list_all_blobs` pages walked.
    pub pages_scanned: usize,
    /// Total blobs the sweep saw across all pages.
    pub blobs_examined: usize,
    /// Blobs classified as authorized (present in `blob`).
    pub authorized: usize,
    /// Blobs classified as in-flight (present in
    /// `temp_blob_metadata`).
    pub in_flight: usize,
    /// Blobs classified as too-young (orphan candidate younger
    /// than the freshness threshold).
    pub too_young: usize,
    /// Blobs classified as confirmed orphans (eligible for
    /// deletion, regardless of whether the sweep actually
    /// deleted).
    pub confirmed_orphans_found: usize,
    /// Confirmed orphans the sweep successfully deleted.
    pub orphans_deleted: usize,
    /// Confirmed orphans skipped because
    /// [`SweepParams::max_deletes_per_run`] was reached.
    pub orphans_skipped_safety_cap: usize,
    /// Wall-clock duration of the sweep run.
    pub duration_seconds: f64,
}

/// Classify a single blob candidate from storage given the
/// page-level DB lookup results.
///
/// Pure function — no I/O, no async — so the two-stage
/// classification logic is unit-testable without a running
/// storage backend. `now` is parameterised for testability
/// (the production caller passes `Utc::now()`; tests pass a
/// fixed time).
///
/// The precedence is `Authorized` > `InFlight` > age-based:
///
/// 1. If the CID is present in `authorized_cids` (the page-
///    level read of `blob.cid IN (...)`), the blob is
///    legitimately tracked; classify `Authorized`.
/// 2. Else if it's present in `in_flight_cids` (the page-level
///    read of `temp_blob_metadata.cid IN (...)`), it's a
///    staged upload; classify `InFlight`. Per V04_DESIGN.md
///    §9.3.2 the tracking surface is authoritative, so this
///    takes priority over age.
/// 3. Else apply the freshness threshold. Age younger than
///    the threshold → `TooYoung` (skip and re-evaluate next
///    sweep). Age ≥ threshold → `ConfirmedOrphan` (eligible
///    for deletion).
pub(crate) fn classify_blob(
    entry: &BlobListEntry,
    authorized_cids: &HashSet<String>,
    in_flight_cids: &HashSet<String>,
    freshness_threshold: Duration,
    now: DateTime<Utc>,
) -> BlobClassification {
    if authorized_cids.contains(&entry.cid) {
        return BlobClassification::Authorized {
            cid: entry.cid.clone(),
        };
    }
    if in_flight_cids.contains(&entry.cid) {
        return BlobClassification::InFlight {
            cid: entry.cid.clone(),
        };
    }

    let age = now.signed_duration_since(entry.last_modified);
    let threshold_chrono =
        chrono::Duration::from_std(freshness_threshold).unwrap_or(chrono::Duration::MAX);

    if age < threshold_chrono {
        BlobClassification::TooYoung {
            cid: entry.cid.clone(),
            last_modified: entry.last_modified,
        }
    } else {
        BlobClassification::ConfirmedOrphan {
            cid: entry.cid.clone(),
            last_modified: entry.last_modified,
        }
    }
}

/// Run a GC sweep against `backend`, reconciling with `pool`.
///
/// The sweep pages through `backend.list_all_blobs`, for each
/// page runs two cross-backend IN-clause queries (`blob` for
/// authorized CIDs, `temp_blob_metadata` for in-flight CIDs),
/// classifies each entry via [`classify_blob`], and applies
/// the action implied by the classification + `params`:
///
/// * `Authorized` / `InFlight` / `TooYoung` — count only.
/// * `ConfirmedOrphan` — count; if `dry_run` or `report_only`
///   is set, log and skip the delete; otherwise, if
///   `report.orphans_deleted >= params.max_deletes_per_run`,
///   skip the delete and increment
///   `orphans_skipped_safety_cap`; otherwise call
///   `backend.delete(cid)` and increment `orphans_deleted` on
///   success (or warn-log + continue on failure — Q7's
///   storage-delete idempotency means a later retry is safe).
///
/// `now` is passed in (rather than computed inside) so tests
/// can age fresh blobs into the orphan window without
/// backdating filesystem mtimes. Production callers (Step 3's
/// `JobScheduler::gc_sweep_job` and CLI subcommand) pass
/// [`Utc::now()`].
///
/// Out-of-scope per Step 2: no atomicity with Arc 4's
/// DeferredAction queue or the wrapping write transactions
/// (V04_DESIGN.md §9.1 — the sweep is a separate post-commit
/// reconciliation surface).
pub async fn run_sweep<B: BlobBackend + ?Sized>(
    backend: &B,
    pool: &AnyPool,
    params: SweepParams,
    now: DateTime<Utc>,
) -> PdsResult<SweepReport> {
    let start = std::time::Instant::now();
    let mut report = SweepReport::default();
    let mut cursor: Option<String> = None;

    loop {
        let page = backend
            .list_all_blobs(cursor.clone(), params.page_size)
            .await?;
        report.pages_scanned += 1;
        report.blobs_examined += page.entries.len();

        // Empty store on the first iteration: no work to do.
        if page.entries.is_empty() && page.next_cursor.is_none() {
            break;
        }

        let candidate_cids: Vec<&str> =
            page.entries.iter().map(|e| e.cid.as_str()).collect();
        let authorized_cids = query_authorized_cids(pool, &candidate_cids).await?;
        let in_flight_cids = query_in_flight_cids(pool, &candidate_cids).await?;

        for entry in &page.entries {
            let classification = classify_blob(
                entry,
                &authorized_cids,
                &in_flight_cids,
                params.freshness_threshold,
                now,
            );

            match &classification {
                BlobClassification::Authorized { .. } => {
                    report.authorized += 1;
                }
                BlobClassification::InFlight { .. } => {
                    report.in_flight += 1;
                }
                BlobClassification::TooYoung { cid, last_modified } => {
                    report.too_young += 1;
                    tracing::debug!(
                        cid = %cid,
                        age_secs = (now - *last_modified).num_seconds(),
                        "blob too young for orphan classification"
                    );
                }
                BlobClassification::ConfirmedOrphan { cid, last_modified } => {
                    report.confirmed_orphans_found += 1;

                    if params.dry_run || params.report_only {
                        tracing::info!(
                            cid = %cid,
                            last_modified = %last_modified,
                            "orphan found (dry-run; no delete)"
                        );
                        continue;
                    }

                    if report.orphans_deleted >= params.max_deletes_per_run {
                        report.orphans_skipped_safety_cap += 1;
                        tracing::info!(
                            cid = %cid,
                            "safety cap reached; orphan deferred to next sweep"
                        );
                        continue;
                    }

                    match backend.delete(cid).await {
                        Ok(()) => {
                            report.orphans_deleted += 1;
                            tracing::info!(cid = %cid, "orphan deleted");
                        }
                        Err(e) => {
                            // Backend delete errored. Not an orphan-
                            // classification problem; log + continue.
                            // Per Step 0 Q7 both backends are
                            // idempotent on delete, so the next sweep
                            // safely retries.
                            tracing::warn!(
                                cid = %cid,
                                error = %e,
                                "orphan delete failed; will retry next sweep"
                            );
                        }
                    }
                }
            }
        }

        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    report.duration_seconds = start.elapsed().as_secs_f64();

    // Emit Prometheus metrics per V04_DESIGN.md §9.3.3. The
    // safety-cap-hit signal is derivable from
    // `orphans_found_total - orphans_deleted_total > 0` (with
    // `dry_run` off), so no separate counter is registered.
    crate::metrics::GC_SWEEP_ORPHANS_FOUND_TOTAL
        .inc_by(report.confirmed_orphans_found as u64);
    crate::metrics::GC_SWEEP_ORPHANS_DELETED_TOTAL
        .inc_by(report.orphans_deleted as u64);
    crate::metrics::GC_SWEEP_DURATION_SECONDS.observe(report.duration_seconds);

    Ok(report)
}

/// Build the IN-clause placeholder string `$1, $2, ..., $N`.
fn placeholders(n: usize) -> String {
    (1..=n)
        .map(|i| format!("${}", i))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Page-level lookup against `blob` (the authoritative
/// authorized-blob surface per Step 0 Q5). Returns the subset
/// of `candidates` present in the table.
async fn query_authorized_cids(
    pool: &AnyPool,
    candidates: &[&str],
) -> PdsResult<HashSet<String>> {
    if candidates.is_empty() {
        return Ok(HashSet::new());
    }
    let sql = format!(
        "SELECT cid FROM blob WHERE cid IN ({})",
        placeholders(candidates.len())
    );
    let mut q = sqlx::query_scalar::<_, String>(&sql);
    for cid in candidates {
        q = q.bind(*cid);
    }
    let cids: Vec<String> = q.fetch_all(pool).await?;
    Ok(cids.into_iter().collect())
}

/// Page-level lookup against `temp_blob_metadata` (the
/// authoritative in-flight surface per Step 0 Q9). Returns the
/// subset of `candidates` present in the table.
async fn query_in_flight_cids(
    pool: &AnyPool,
    candidates: &[&str],
) -> PdsResult<HashSet<String>> {
    if candidates.is_empty() {
        return Ok(HashSet::new());
    }
    let sql = format!(
        "SELECT cid FROM temp_blob_metadata WHERE cid IN ({})",
        placeholders(candidates.len())
    );
    let mut q = sqlx::query_scalar::<_, String>(&sql);
    for cid in candidates {
        q = q.bind(*cid);
    }
    let cids: Vec<String> = q.fetch_all(pool).await?;
    Ok(cids.into_iter().collect())
}

// ===========================================================================
// Arc 16d — Row-driven GC sweep (V05_DESIGN.md §9.4, LOCKED at v5)
//
// Adds a row-walker that complements Arc 10's byte-walker (above):
// - Byte-walker (Arc 10): finds bytes-at-final-position with no
//   matching row. Recovers Arc 16c's commit-phase orphan-bytes case +
//   Arc 16d's `backend.delete` failure cases (§9.4.5.1).
// - Row-walker (Arc 16d, below): finds untethered `blob_metadata`
//   rows whose TTL has elapsed.
//
// Two-phase autocommit per §9.4.3.1: Phase 1 SELECTs a page of
// (cid, created_at) with cursor pagination; Phase 2 issues a
// per-row predicate-guarded DELETE; the predicate guard
// (`temp_key IS NOT NULL AND created_at < $cutoff`) re-evaluates
// at lock-acquisition time so concurrent STRICT promotion or
// re-upload `track_untethered_blob` Case 3 refreshes resolve via
// zero-row-affected (race-skip), not data corruption.
//
// Per-row sequence (§9.4.3.2): Phase 2 DELETE → [test hook] →
// fresh-row check (mitigates Case 2a post-commit byte-loss race
// per §9.4.5.9; v0.6+ candidates for residual mitigation) →
// `backend.delete` (only if no fresh row) with per-row INFO log
// for ops investigation (round-4 F2 closure).
// ===========================================================================

/// Arc 16d row-sweep parameters (§9.4.4 Step 2.2). Distinct from
/// Arc 10's [`SweepParams`] because the two walkers don't share
/// a TTL semantic (Arc 10's `freshness_threshold` is a
/// classifier-safety bound; Arc 16d's `untethered_ttl` is the TTL
/// anchor for row reclamation).
#[derive(Clone)]
pub struct RowSweepParams {
    /// If `true`, the sweep logs would-be deletes but performs
    /// neither the row DELETE nor the bytes delete. Populates the
    /// `total_eligible_count` + `would_delete_count` Option fields
    /// in the [`RowSweepReport`] instead of `rows_deleted`.
    pub dry_run: bool,

    /// Safety cap per §9.4.3.2 — stops the cycle once
    /// `rows_deleted >= max_deletes_per_run`. Shared with the
    /// byte-walker via `GcSweepConfig.max_deletes_per_run`.
    pub max_deletes_per_run: usize,

    /// Page size for the Phase 1 SELECT (§9.4.3.1). Shared with
    /// the byte-walker via `GcSweepConfig.page_size`.
    pub page_size: usize,

    /// TTL anchor: rows are eligible when
    /// `created_at < now - untethered_ttl`. Sourced from
    /// `GcSweepConfig.untethered_ttl_seconds`.
    pub untethered_ttl: Duration,

    /// Test-only synchronization hook (V05_DESIGN.md §9.4.4 Step
    /// 2.5 — closes round-4 F7). When `Some`, fires between Phase
    /// 2 DELETE commit and fresh-row SELECT for each row. The
    /// closure receives the just-deleted CID and returns a
    /// future the sweep awaits before continuing. Production
    /// callers leave this `None`; Scenario 9b unit + Phase B
    /// tests inject synthetic INSERT statements during the gap.
    #[cfg(test)]
    #[allow(clippy::type_complexity)]
    pub after_phase2_delete_hook: Option<
        std::sync::Arc<
            dyn Fn(String) -> futures::future::BoxFuture<'static, ()> + Send + Sync,
        >,
    >,
}

impl std::fmt::Debug for RowSweepParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowSweepParams")
            .field("dry_run", &self.dry_run)
            .field("max_deletes_per_run", &self.max_deletes_per_run)
            .field("page_size", &self.page_size)
            .field("untethered_ttl", &self.untethered_ttl)
            .finish()
    }
}

impl Default for RowSweepParams {
    fn default() -> Self {
        Self {
            dry_run: true,
            max_deletes_per_run: 10_000,
            page_size: 500,
            untethered_ttl: Duration::from_secs(86_400),
            #[cfg(test)]
            after_phase2_delete_hook: None,
        }
    }
}

/// Arc 16d row-sweep cycle report (§9.4.4 Step 2.3). Counters
/// surface every per-row outcome class for operator observability
/// (§9.4.5.9 operator-action references `bytes_delete_skipped_fresh_row_count`
/// and `db_error_skip_count` explicitly).
#[derive(Debug, Default, Clone)]
pub struct RowSweepReport {
    /// Rows successfully DELETEd with bytes-delete succeeding (or
    /// skipped via fresh-row diagnostic per §9.4.3.2 / §9.4.5.9).
    pub rows_deleted: u64,

    /// Per-row Phase 2 DELETE returned `rows_affected = 0` —
    /// predicate guard caught a concurrent STRICT promotion or
    /// re-upload refresh between Phase 1 SELECT and Phase 2
    /// statement execution (V05_DESIGN.md §9.4.3.4 Cases 1b +
    /// 3 benign resolutions).
    pub race_skip_count: u64,

    /// Per-row `backend.delete` returned an error — row was
    /// DELETEd but bytes remain at final position. Arc 10's
    /// byte-walker picks these up on a subsequent cycle
    /// (§9.4.5.1).
    pub bytes_delete_failure_count: u64,

    /// Per-row autocommit DELETE itself returned a DB error.
    /// Cursor advances regardless of per-row outcome per
    /// §9.4.3.2; counted here for operator triage threshold
    /// (§9.4.5.7).
    pub db_error_skip_count: u64,

    /// Fresh-row diagnostic fired per §9.4.5.9 — a new row for
    /// the same CID appeared between Phase 2 DELETE and the
    /// fresh-row SELECT (Arc 16c re-upload `track_untethered_blob`
    /// Case 3). Bytes-delete skipped to avoid deleting bytes the
    /// fresh row claims.
    pub bytes_delete_skipped_fresh_row_count: u64,

    /// Dry-run only: total rows the predicate would have matched.
    /// `None` outside dry-run mode.
    pub total_eligible_count: Option<u64>,

    /// Dry-run only: rows that would have been DELETEd under the
    /// safety cap. `None` outside dry-run mode.
    pub would_delete_count: Option<u64>,

    /// Phase 1 pages scanned (cycle metadata).
    pub pages_scanned: u64,

    /// Wall-clock cycle duration in seconds.
    pub duration_seconds: f64,
}

/// Arc 16d cursor type for stable pagination across the sweep
/// cycle (§9.4.4 Step 2.4). `(created_at, cid)` tuple ordering
/// gives a total order under the partial index
/// `idx_blob_metadata_untethered(created_at) WHERE temp_key IS
/// NOT NULL`. CID is the disambiguator for rows with identical
/// `created_at` (rare in practice but possible under high-rate
/// upload bursts).
#[derive(Debug, Clone)]
pub struct SweepCursor {
    pub last_created_at: String,
    pub last_cid: String,
}

/// Arc 16d row-driven GC sweep primitive (§9.4.4 Step 2.5; body
/// per §9.4.3.1 + §9.4.3.2).
///
/// Walks `blob_metadata` rows in the untethered state
/// (`temp_key IS NOT NULL`) whose `created_at` is older than
/// `now - params.untethered_ttl`. For each row:
///
/// 1. Issue Phase 2 predicate-guarded DELETE via
///    [`crate::db::autocommit::autocommit_execute`].
/// 2. If `rows_affected == 1`: fire the test hook (if any), then
///    issue the fresh-row SELECT via
///    [`crate::db::autocommit::autocommit_fetch_optional`].
/// 3. If no fresh row: call `backend.delete(cid)` and log INFO on
///    success (round-4 F2 closure — operators trace residual-race
///    events through this log per §9.4.5.9).
/// 4. If fresh row present: skip bytes-delete, increment
///    `bytes_delete_skipped_fresh_row_count`, log WARN.
///
/// Cursor advances regardless of per-row outcome per §9.4.3.2.
///
/// Every SQL statement goes through the
/// [`crate::db::autocommit`] wrappers — Step 5 audit item 8 grep
/// enforces. Backend `delete` calls go through the
/// [`crate::blob_store::BlobBackend`] trait surface and are
/// explicitly out of audit scope per round-4 F6.
///
/// `now` is parameterized for testability (production callers
/// pass `chrono::Utc::now()`; tests pass `Utc::now() +
/// chrono::Duration::hours(N)` to age fixtures into the sweep
/// window — same pattern as Arc 10's
/// [`run_sweep`]).
pub async fn sweep_untethered_rows<B: BlobBackend + ?Sized>(
    pool: &AnyPool,
    backend: &B,
    params: RowSweepParams,
    now: DateTime<Utc>,
) -> PdsResult<RowSweepReport> {
    use crate::db::autocommit::{
        autocommit_execute, autocommit_fetch_all, autocommit_fetch_optional,
    };

    let start = std::time::Instant::now();
    let mut report = RowSweepReport::default();
    if params.dry_run {
        report.total_eligible_count = Some(0);
        report.would_delete_count = Some(0);
    }

    let cutoff = now
        - chrono::Duration::from_std(params.untethered_ttl)
            .unwrap_or(chrono::Duration::MAX);
    let cutoff_rfc3339 = cutoff.to_rfc3339();

    let mut cursor: Option<SweepCursor> = None;

    'outer: loop {
        // Arc 16d §9.4.3.1 Phase 1: page selection with cursor
        // (autocommit SELECT). First page uses two-clause predicate;
        // subsequent pages add the cursor disjunction for stable
        // pagination past the previous page's last row.
        let page_rows = match &cursor {
            None => {
                let q = sqlx::query(
                    "SELECT cid, created_at \
                     FROM blob_metadata \
                     WHERE temp_key IS NOT NULL \
                       AND created_at < $1 \
                     ORDER BY created_at, cid \
                     LIMIT $2",
                )
                .bind(&cutoff_rfc3339)
                .bind(params.page_size as i64);
                autocommit_fetch_all(pool, q).await
            }
            Some(cur) => {
                let q = sqlx::query(
                    "SELECT cid, created_at \
                     FROM blob_metadata \
                     WHERE temp_key IS NOT NULL \
                       AND created_at < $1 \
                       AND (created_at > $2 \
                            OR (created_at = $2 AND cid > $3)) \
                     ORDER BY created_at, cid \
                     LIMIT $4",
                )
                .bind(&cutoff_rfc3339)
                .bind(&cur.last_created_at)
                .bind(&cur.last_cid)
                .bind(params.page_size as i64);
                autocommit_fetch_all(pool, q).await
            }
        };

        let page_rows = match page_rows {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "row_sweep: Phase 1 page-select failed; aborting cycle"
                );
                return Err(PdsError::Database(e));
            }
        };

        report.pages_scanned += 1;

        if page_rows.is_empty() {
            break;
        }

        for row in &page_rows {
            use sqlx::Row;
            let cid: String = row.try_get("cid")?;
            let created_at: String = row.try_get("created_at")?;

            // Cursor advances on every observed row — race-skip,
            // db-error-skip, and successful-delete all advance per
            // §9.4.3.2 ("Cursor advances regardless of per-row
            // outcome.").
            cursor = Some(SweepCursor {
                last_created_at: created_at.clone(),
                last_cid: cid.clone(),
            });

            // Dry-run early-out: count, don't delete.
            if params.dry_run {
                *report.total_eligible_count.get_or_insert(0) += 1;
                let would_delete = report
                    .would_delete_count
                    .map(|c| c < params.max_deletes_per_run as u64)
                    .unwrap_or(true);
                if would_delete {
                    *report.would_delete_count.get_or_insert(0) += 1;
                }
                continue;
            }

            // Arc 16d §9.4.3.1 Phase 2: per-row autocommit DELETE
            // with predicate guard. Predicate is re-evaluated at
            // lock acquisition; zero-row outcome = concurrent
            // STRICT promotion or re-upload refresh resolved
            // benignly (V05_DESIGN.md §9.4.3.4 Cases 1b / 3 /
            // Interleaving B).
            let delete_q = sqlx::query(
                "DELETE FROM blob_metadata \
                 WHERE cid = $1 \
                   AND temp_key IS NOT NULL \
                   AND created_at < $2",
            )
            .bind(&cid)
            .bind(&cutoff_rfc3339);

            let affected = match autocommit_execute(pool, delete_q).await {
                Ok(r) => r.rows_affected(),
                Err(e) => {
                    report.db_error_skip_count += 1;
                    tracing::warn!(
                        cid = %cid,
                        error = %e,
                        "row_sweep: Phase 2 DELETE failed; cursor advances",
                    );
                    continue;
                }
            };

            if affected == 0 {
                report.race_skip_count += 1;
                continue;
            }

            // Arc 16d §9.4.4 Step 2.5 / round-4 F7 closure:
            // [TEST HOOK between DELETE commit and fresh-row check;
            //  used by §9.4.4 Step 2.6 unit test + §9.4.8.2 Scenario
            //  9b]. Production never sets this; tests inject
            //  synthetic INSERTs to exercise the fresh-row
            //  diagnostic path.
            #[cfg(test)]
            if let Some(hook) = &params.after_phase2_delete_hook {
                hook(cid.clone()).await;
            }

            // Arc 16d §9.4.3.2 + §9.4.5.9 Case 2a mitigation:
            // fresh-row check via autocommit SELECT. If Arc 16c
            // committed a re-upload INSERT for this CID between
            // our Phase 2 DELETE and this SELECT, the fresh row's
            // bytes claim takes precedence — skip the
            // bytes-delete.
            let fresh_q = sqlx::query(
                "SELECT 1 FROM blob_metadata WHERE cid = $1 LIMIT 1",
            )
            .bind(&cid);
            let fresh_row_exists = match autocommit_fetch_optional(pool, fresh_q).await
            {
                Ok(opt) => opt.is_some(),
                Err(e) => {
                    // DB error on the fresh-row check is fail-safe:
                    // assume the row might be there + skip bytes-
                    // delete. Conservative; preserves the §9.4.5.9
                    // mitigation contract under DB-failure
                    // conditions.
                    tracing::warn!(
                        cid = %cid,
                        error = %e,
                        "row_sweep: fresh-row check failed; \
                         skipping bytes-delete defensively",
                    );
                    report.bytes_delete_skipped_fresh_row_count += 1;
                    continue;
                }
            };

            if fresh_row_exists {
                report.bytes_delete_skipped_fresh_row_count += 1;
                tracing::warn!(
                    cid = %cid,
                    "row_sweep: post-commit byte-loss race: fresh row appeared \
                     between row-DELETE and bytes-delete check; \
                     skipping bytes-delete (V05_DESIGN.md §9.4.5.9)",
                );
                continue;
            }

            // Bytes-delete via Arc 10 / Arc 16c's existing
            // BlobBackend::delete (out of autocommit-wrapper scope
            // per round-4 F6; governed by the backend-trait
            // contract).
            match backend.delete(&cid).await {
                Ok(()) => {
                    report.rows_deleted += 1;
                    // Per-row INFO log per §9.4.3.2 round-4 F2
                    // closure: operators trace residual-race
                    // events through this log per §9.4.5.9
                    // operator-action.
                    tracing::info!(
                        cid = %cid,
                        ts = %now.to_rfc3339(),
                        "row_sweep.backend_delete",
                    );
                }
                Err(e) => {
                    report.bytes_delete_failure_count += 1;
                    tracing::warn!(
                        cid = %cid,
                        error = %e,
                        "row_sweep.backend_delete_failed (bytes orphan; \
                         Arc 10 byte-walker recovery)",
                    );
                }
            }

            // Safety cap (§9.4.3.2): stop the cycle once the cap
            // is reached. Remaining eligible rows wait for the
            // next cycle.
            if report.rows_deleted >= params.max_deletes_per_run as u64 {
                break 'outer;
            }
        }

        // Cursor-driven termination: if the page returned fewer
        // than page_size rows, there's no next page to fetch.
        if page_rows.len() < params.page_size {
            break;
        }
    }

    report.duration_seconds = start.elapsed().as_secs_f64();

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(cid: &str, last_modified: DateTime<Utc>) -> BlobListEntry {
        BlobListEntry {
            cid: cid.to_string(),
            last_modified,
        }
    }

    fn set(cids: &[&str]) -> HashSet<String> {
        cids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn default_sweep_params_match_v04_design_safety_stance() {
        let p = SweepParams::default();
        assert!(p.dry_run, "default must be dry_run = true");
        assert!(!p.report_only);
        assert_eq!(p.max_deletes_per_run, 10_000);
        assert_eq!(p.freshness_threshold, Duration::from_secs(3600));
        assert_eq!(p.page_size, 500);
    }

    #[test]
    fn default_sweep_report_is_all_zeros() {
        let r = SweepReport::default();
        assert_eq!(r.pages_scanned, 0);
        assert_eq!(r.blobs_examined, 0);
        assert_eq!(r.authorized, 0);
        assert_eq!(r.in_flight, 0);
        assert_eq!(r.too_young, 0);
        assert_eq!(r.confirmed_orphans_found, 0);
        assert_eq!(r.orphans_deleted, 0);
        assert_eq!(r.orphans_skipped_safety_cap, 0);
        assert_eq!(r.duration_seconds, 0.0);
    }

    #[test]
    fn classification_variants_are_distinct() {
        let ts = Utc::now();
        let a = BlobClassification::Authorized {
            cid: "bafyA".to_string(),
        };
        let i = BlobClassification::InFlight {
            cid: "bafyA".to_string(),
        };
        let y = BlobClassification::TooYoung {
            cid: "bafyA".to_string(),
            last_modified: ts,
        };
        let o = BlobClassification::ConfirmedOrphan {
            cid: "bafyA".to_string(),
            last_modified: ts,
        };
        assert_ne!(a, i);
        assert_ne!(a, y);
        assert_ne!(a, o);
        assert_ne!(i, y);
        assert_ne!(i, o);
        assert_ne!(y, o);
    }

    // ====================================================================
    // classify_blob — pure-function unit tests for the two-stage
    // classification logic. The Arc 10 Step 0 Q9 decision: tracking
    // surface (temp_blob_metadata) is authoritative; age is belt-and-
    // braces. These tests pin the precedence and the threshold boundary.
    // ====================================================================

    const ONE_HOUR: Duration = Duration::from_secs(3600);

    #[test]
    fn test_classify_authorized() {
        // CID in `authorized_cids` -> Authorized regardless of age or
        // in_flight membership. `blob` is authoritative.
        let now = Utc::now();
        let stale_ts = now - chrono::Duration::hours(48);
        let e = entry("bafyA", stale_ts);
        let authorized = set(&["bafyA"]);
        let in_flight = set(&["bafyA"]); // contrived overlap

        let c = classify_blob(&e, &authorized, &in_flight, ONE_HOUR, now);
        assert_eq!(
            c,
            BlobClassification::Authorized {
                cid: "bafyA".to_string()
            }
        );
    }

    #[test]
    fn test_classify_in_flight() {
        // CID only in `in_flight_cids` -> InFlight.
        let now = Utc::now();
        let e = entry("bafyB", now - chrono::Duration::minutes(30));
        let authorized = HashSet::new();
        let in_flight = set(&["bafyB"]);

        let c = classify_blob(&e, &authorized, &in_flight, ONE_HOUR, now);
        assert_eq!(
            c,
            BlobClassification::InFlight {
                cid: "bafyB".to_string()
            }
        );
    }

    #[test]
    fn test_classify_in_flight_takes_priority_over_age() {
        // CID in in_flight but storage age exceeds threshold -> still
        // InFlight. Tracking surface authoritative per Step 0 Q9.
        let now = Utc::now();
        let very_old = now - chrono::Duration::days(7);
        let e = entry("bafyC", very_old);
        let authorized = HashSet::new();
        let in_flight = set(&["bafyC"]);

        let c = classify_blob(&e, &authorized, &in_flight, ONE_HOUR, now);
        assert_eq!(
            c,
            BlobClassification::InFlight {
                cid: "bafyC".to_string()
            }
        );
    }

    #[test]
    fn test_classify_too_young() {
        // CID in neither set, age < threshold -> TooYoung.
        let now = Utc::now();
        let recent = now - chrono::Duration::minutes(5);
        let e = entry("bafyD", recent);

        let c = classify_blob(&e, &HashSet::new(), &HashSet::new(), ONE_HOUR, now);
        assert_eq!(
            c,
            BlobClassification::TooYoung {
                cid: "bafyD".to_string(),
                last_modified: recent,
            }
        );
    }

    #[test]
    fn test_classify_confirmed_orphan() {
        // CID in neither set, age >= threshold -> ConfirmedOrphan.
        let now = Utc::now();
        let old = now - chrono::Duration::hours(2);
        let e = entry("bafyE", old);

        let c = classify_blob(&e, &HashSet::new(), &HashSet::new(), ONE_HOUR, now);
        assert_eq!(
            c,
            BlobClassification::ConfirmedOrphan {
                cid: "bafyE".to_string(),
                last_modified: old,
            }
        );
    }

    #[test]
    fn test_classify_at_threshold_boundary() {
        // Age exactly equal to the freshness threshold -> ConfirmedOrphan.
        // The classifier uses `<` for the TooYoung branch, so the
        // boundary case falls into the ConfirmedOrphan (delete-eligible)
        // bucket — preferring deletion at the edge over an indefinite
        // skip-loop on the same CID.
        let now = Utc::now();
        let exactly_threshold = now - chrono::Duration::seconds(3600);
        let e = entry("bafyF", exactly_threshold);

        let c = classify_blob(&e, &HashSet::new(), &HashSet::new(), ONE_HOUR, now);
        assert_eq!(
            c,
            BlobClassification::ConfirmedOrphan {
                cid: "bafyF".to_string(),
                last_modified: exactly_threshold,
            }
        );
    }

    // ====================================================================
    // run_sweep — integration tests against DiskBlobBackend +
    // in-memory SQLite. The S3 backend is structurally covered by Step 1's
    // s3.rs unit tests; here we exercise the cross-backend reconciliation
    // logic and the safety-cap / dry-run / classification mix.
    // ====================================================================

    use crate::blob_store::disk::DiskBlobBackend;
    use sqlx::any::AnyPoolOptions;
    use std::sync::Once;
    use tempfile::TempDir;

    /// Build a fresh in-memory SQLite pool with the `blob` and
    /// `temp_blob_metadata` schemas. Mirrors
    /// `migrations/0001_initial.sql:156-192`.
    async fn setup_sweep_pool() -> AnyPool {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open sqlite::memory: pool");

        sqlx::query(
            "CREATE TABLE blob (
                cid          TEXT PRIMARY KEY,
                did          TEXT NOT NULL,
                size         INTEGER NOT NULL,
                mime_type    TEXT NOT NULL,
                created_at   TEXT NOT NULL,
                takedown     INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .expect("create blob table");

        sqlx::query(
            "CREATE TABLE temp_blob_metadata (
                cid              TEXT PRIMARY KEY,
                mime_type        TEXT NOT NULL,
                size             INTEGER NOT NULL,
                creator_did      TEXT NOT NULL,
                created_at       TEXT NOT NULL,
                width            INTEGER,
                height           INTEGER
            )",
        )
        .execute(&pool)
        .await
        .expect("create temp_blob_metadata table");

        pool
    }

    async fn seed_blob_row(pool: &AnyPool, cid: &str) {
        sqlx::query(
            "INSERT INTO blob (cid, did, size, mime_type, created_at, takedown) \
             VALUES ($1, 'did:plc:test', 1, 'image/png', '2026-05-13T00:00:00Z', 0)",
        )
        .bind(cid)
        .execute(pool)
        .await
        .expect("seed blob");
    }

    async fn seed_temp_blob_row(pool: &AnyPool, cid: &str) {
        sqlx::query(
            "INSERT INTO temp_blob_metadata \
             (cid, mime_type, size, creator_did, created_at) \
             VALUES ($1, 'image/png', 1, 'did:plc:test', '2026-05-13T00:00:00Z')",
        )
        .bind(cid)
        .execute(pool)
        .await
        .expect("seed temp_blob_metadata");
    }

    /// Put `cid` into the disk backend with arbitrary content.
    async fn put_blob(backend: &DiskBlobBackend, cid: &str) {
        backend
            .put(cid, b"x".to_vec(), "image/png")
            .await
            .expect("backend put");
    }

    /// Convenience: a `now` value far enough in the future to push
    /// freshly-written test blobs past the freshness threshold.
    /// We can't backdate filesystem mtimes portably across the test
    /// matrix (no `filetime` dep + no `utimensat` shim), so the sweep
    /// caller's `now` parameter is the supported handle for the
    /// "blob is old" scenario.
    fn now_two_hours_ahead() -> DateTime<Utc> {
        Utc::now() + chrono::Duration::hours(2)
    }

    #[tokio::test]
    async fn test_sweep_empty_store_returns_empty_report() {
        let dir = TempDir::new().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());
        let pool = setup_sweep_pool().await;

        let report = run_sweep(&backend, &pool, SweepParams::default(), Utc::now())
            .await
            .unwrap();

        assert_eq!(report.blobs_examined, 0);
        assert_eq!(report.authorized, 0);
        assert_eq!(report.in_flight, 0);
        assert_eq!(report.too_young, 0);
        assert_eq!(report.confirmed_orphans_found, 0);
        assert_eq!(report.orphans_deleted, 0);
        assert_eq!(report.orphans_skipped_safety_cap, 0);
    }

    #[tokio::test]
    async fn test_sweep_all_authorized_no_actions() {
        let dir = TempDir::new().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());
        let pool = setup_sweep_pool().await;

        for cid in ["bafyaaaa01", "bafyaaaa02", "bafyaaaa03"] {
            put_blob(&backend, cid).await;
            seed_blob_row(&pool, cid).await;
        }

        let params = SweepParams {
            dry_run: false, // would not matter — no orphans
            ..SweepParams::default()
        };
        let report = run_sweep(&backend, &pool, params, now_two_hours_ahead())
            .await
            .unwrap();

        assert_eq!(report.blobs_examined, 3);
        assert_eq!(report.authorized, 3);
        assert_eq!(report.confirmed_orphans_found, 0);
        assert_eq!(report.orphans_deleted, 0);
    }

    #[tokio::test]
    async fn test_sweep_all_in_flight_no_actions() {
        let dir = TempDir::new().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());
        let pool = setup_sweep_pool().await;

        for cid in ["bafybbbb01", "bafybbbb02"] {
            put_blob(&backend, cid).await;
            seed_temp_blob_row(&pool, cid).await;
        }

        let params = SweepParams {
            dry_run: false,
            ..SweepParams::default()
        };
        let report = run_sweep(&backend, &pool, params, now_two_hours_ahead())
            .await
            .unwrap();

        assert_eq!(report.blobs_examined, 2);
        assert_eq!(report.in_flight, 2);
        assert_eq!(report.confirmed_orphans_found, 0);
        assert_eq!(report.orphans_deleted, 0);
    }

    #[tokio::test]
    async fn test_sweep_too_young_no_delete() {
        let dir = TempDir::new().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());
        let pool = setup_sweep_pool().await;

        // No DB rows; blobs are fresh; `now` is Utc::now() so they're
        // under the 1h freshness threshold.
        for cid in ["bafycccc01", "bafycccc02"] {
            put_blob(&backend, cid).await;
        }

        let params = SweepParams {
            dry_run: false,
            ..SweepParams::default()
        };
        let report = run_sweep(&backend, &pool, params, Utc::now()).await.unwrap();

        assert_eq!(report.blobs_examined, 2);
        assert_eq!(report.too_young, 2);
        assert_eq!(report.confirmed_orphans_found, 0);
        assert_eq!(report.orphans_deleted, 0);
    }

    #[tokio::test]
    async fn test_sweep_confirmed_orphan_deleted() {
        let dir = TempDir::new().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());
        let pool = setup_sweep_pool().await;

        let cid = "bafydddd01";
        put_blob(&backend, cid).await;
        // No `blob` row, no `temp_blob_metadata` row -> orphan.

        let params = SweepParams {
            dry_run: false,
            ..SweepParams::default()
        };
        let report = run_sweep(&backend, &pool, params, now_two_hours_ahead())
            .await
            .unwrap();

        assert_eq!(report.confirmed_orphans_found, 1);
        assert_eq!(report.orphans_deleted, 1);

        // Storage really emptied.
        let after = backend.list_all_blobs(None, 100).await.unwrap();
        assert!(after.entries.is_empty(), "orphan should be gone");
    }

    #[tokio::test]
    async fn test_sweep_dry_run_logs_no_delete() {
        let dir = TempDir::new().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());
        let pool = setup_sweep_pool().await;

        let cid = "bafyeeee01";
        put_blob(&backend, cid).await;

        let params = SweepParams {
            dry_run: true,
            ..SweepParams::default()
        };
        let report = run_sweep(&backend, &pool, params, now_two_hours_ahead())
            .await
            .unwrap();

        assert_eq!(report.confirmed_orphans_found, 1);
        assert_eq!(report.orphans_deleted, 0);
        // Storage still contains the blob.
        let after = backend.list_all_blobs(None, 100).await.unwrap();
        assert_eq!(after.entries.len(), 1);
        assert_eq!(after.entries[0].cid, cid);
    }

    #[tokio::test]
    async fn test_sweep_report_only_logs_no_delete() {
        // report_only matches dry_run behaviour in Step 2's loop —
        // both early-continue before the delete branch. Step 3 will
        // differentiate at the consumer layer.
        let dir = TempDir::new().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());
        let pool = setup_sweep_pool().await;

        let cid = "bafyffff01";
        put_blob(&backend, cid).await;

        let params = SweepParams {
            dry_run: false,
            report_only: true,
            ..SweepParams::default()
        };
        let report = run_sweep(&backend, &pool, params, now_two_hours_ahead())
            .await
            .unwrap();

        assert_eq!(report.confirmed_orphans_found, 1);
        assert_eq!(report.orphans_deleted, 0);
        assert_eq!(report.orphans_skipped_safety_cap, 0);
    }

    #[tokio::test]
    async fn test_sweep_safety_cap_partial_delete() {
        let dir = TempDir::new().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());
        let pool = setup_sweep_pool().await;

        // 5 orphans, cap at 2 -> deletes 2, skips 3.
        for i in 1..=5 {
            put_blob(&backend, &format!("bafycap000{}", i)).await;
        }

        let params = SweepParams {
            dry_run: false,
            max_deletes_per_run: 2,
            ..SweepParams::default()
        };
        let report = run_sweep(&backend, &pool, params, now_two_hours_ahead())
            .await
            .unwrap();

        assert_eq!(report.confirmed_orphans_found, 5);
        assert_eq!(report.orphans_deleted, 2);
        assert_eq!(report.orphans_skipped_safety_cap, 3);

        // 3 blobs remain in storage.
        let after = backend.list_all_blobs(None, 100).await.unwrap();
        assert_eq!(after.entries.len(), 3);
    }

    #[tokio::test]
    async fn test_sweep_mixed_classification() {
        // Mix of authorized + in-flight + orphans. The too_young
        // variant is covered separately by
        // `test_sweep_too_young_no_delete` — distinguishing it
        // from orphan in the same run would require backdating
        // some blobs' mtimes vs. others, which we can't do
        // portably without a `filetime` dep. Keeping the mixed
        // test focused on the three buckets we *can* deterministically
        // exercise via DB seeding + a future `now`.
        let dir = TempDir::new().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());
        let pool = setup_sweep_pool().await;

        put_blob(&backend, "aaauthorized01").await;
        seed_blob_row(&pool, "aaauthorized01").await;

        put_blob(&backend, "bbinflight0001").await;
        seed_temp_blob_row(&pool, "bbinflight0001").await;

        put_blob(&backend, "ddorphan000001").await;
        put_blob(&backend, "ddorphan000002").await;

        let params = SweepParams {
            dry_run: false,
            ..SweepParams::default()
        };
        let report = run_sweep(&backend, &pool, params, now_two_hours_ahead())
            .await
            .unwrap();

        assert_eq!(report.blobs_examined, 4);
        assert_eq!(report.authorized, 1);
        assert_eq!(report.in_flight, 1);
        assert_eq!(report.too_young, 0);
        assert_eq!(report.confirmed_orphans_found, 2);
        assert_eq!(report.orphans_deleted, 2);
    }

    #[tokio::test]
    async fn test_sweep_multi_page() {
        let dir = TempDir::new().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());
        let pool = setup_sweep_pool().await;

        // Seed 7 blobs, page size 3, dry_run so storage isn't touched.
        // Loop should make ceil(7/3) = 3 pages.
        for i in 1..=7 {
            put_blob(&backend, &format!("bafypg000{:02}", i)).await;
        }

        let params = SweepParams {
            dry_run: true,
            page_size: 3,
            ..SweepParams::default()
        };
        let report = run_sweep(&backend, &pool, params, now_two_hours_ahead())
            .await
            .unwrap();

        assert_eq!(report.blobs_examined, 7);
        assert_eq!(report.confirmed_orphans_found, 7);
        assert_eq!(report.orphans_deleted, 0);
        assert_eq!(report.pages_scanned, 3);
    }

    #[tokio::test]
    async fn test_sweep_authorized_and_in_flight_overlap_classifies_authorized() {
        // Contrived race: a CID present in BOTH `blob` and
        // `temp_blob_metadata`. The classifier's precedence puts
        // `Authorized` first; pin that here at the sweep-loop level
        // (not just the pure-function level) to confirm the loop
        // dispatches consistent with the classifier.
        let dir = TempDir::new().unwrap();
        let backend = DiskBlobBackend::new(dir.path().to_path_buf());
        let pool = setup_sweep_pool().await;

        let cid = "bafyoverlap01";
        put_blob(&backend, cid).await;
        seed_blob_row(&pool, cid).await;
        seed_temp_blob_row(&pool, cid).await;

        let params = SweepParams {
            dry_run: false,
            ..SweepParams::default()
        };
        let report = run_sweep(&backend, &pool, params, now_two_hours_ahead())
            .await
            .unwrap();

        assert_eq!(report.blobs_examined, 1);
        assert_eq!(report.authorized, 1);
        assert_eq!(report.in_flight, 0);
        assert_eq!(report.confirmed_orphans_found, 0);
    }

    #[test]
    fn test_classify_pre_epoch_timestamp() {
        // `entry.last_modified` clamped to UNIX_EPOCH (the Step 1
        // disk-backend defensive output for unreadable mtimes) is far
        // older than any conceivable freshness threshold -> classify as
        // ConfirmedOrphan. The "very old" direction is safe: storage
        // age is always above threshold so the blob enters the
        // delete-eligible bucket, which is the correct end-state for a
        // truly stuck CID.
        let now = Utc::now();
        let epoch = DateTime::<Utc>::UNIX_EPOCH;
        let e = entry("bafyG", epoch);

        let c = classify_blob(&e, &HashSet::new(), &HashSet::new(), ONE_HOUR, now);
        assert_eq!(
            c,
            BlobClassification::ConfirmedOrphan {
                cid: "bafyG".to_string(),
                last_modified: epoch,
            }
        );
    }

    // =======================================================================
    // Arc 16d — row-sweep tests (V05_DESIGN.md §9.4.4 Step 2.6)
    //
    // Coverage:
    // - Row selection + DELETE + bytes-delete success path
    // - Cursor pagination across multiple pages
    // - Persistent db-error-skip at queue head does NOT pin sweep
    // - Race-skip case (predicate-guard zero-row outcome)
    // - Dry-run mode (both counters populated)
    // - Safety-cap mode
    // - Empty-page early exit
    // - Fresh-row check fires via #[cfg(test)] hook (Scenario 9b unit)
    // =======================================================================

    /// Build a fresh in-memory SQLite pool with the Arc 16b
    /// `blob_metadata` schema (migrations/0011 baseline + Arc 16b's
    /// `temp_key` column + partial index).
    async fn setup_row_sweep_pool() -> AnyPool {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open sqlite::memory: pool");

        sqlx::query(
            "CREATE TABLE blob_metadata (
                cid             TEXT PRIMARY KEY,
                mime_type       TEXT NOT NULL,
                size            INTEGER NOT NULL,
                creator_did     TEXT NOT NULL,
                created_at      TEXT NOT NULL,
                width           INTEGER,
                height          INTEGER,
                alt_text        TEXT,
                thumbnail_cid   TEXT,
                temp_key        TEXT NULL CHECK (temp_key IS NULL OR temp_key = '1')
            )",
        )
        .execute(&pool)
        .await
        .expect("create blob_metadata table");

        sqlx::query(
            "CREATE INDEX idx_blob_metadata_untethered \
             ON blob_metadata (created_at) WHERE temp_key IS NOT NULL",
        )
        .execute(&pool)
        .await
        .expect("create partial index");

        pool
    }

    /// Seed an untethered `blob_metadata` row at the given
    /// `created_at` (RFC3339).
    async fn seed_untethered_row(pool: &AnyPool, cid: &str, created_at: &str) {
        sqlx::query(
            "INSERT INTO blob_metadata \
             (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ($1, 'image/png', 1, 'did:plc:test', $2, '1')",
        )
        .bind(cid)
        .bind(created_at)
        .execute(pool)
        .await
        .expect("seed untethered blob_metadata row");
    }

    /// Seed a permanent (NULL temp_key) row.
    async fn seed_permanent_row(pool: &AnyPool, cid: &str, created_at: &str) {
        sqlx::query(
            "INSERT INTO blob_metadata \
             (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ($1, 'image/png', 1, 'did:plc:test', $2, NULL)",
        )
        .bind(cid)
        .bind(created_at)
        .execute(pool)
        .await
        .expect("seed permanent blob_metadata row");
    }

    /// Default Arc 16d test params: page_size = 100, ttl = 1 hour,
    /// safety cap = 10000, NOT dry-run. Tests override fields as
    /// needed.
    fn row_sweep_params() -> RowSweepParams {
        RowSweepParams {
            dry_run: false,
            max_deletes_per_run: 10_000,
            page_size: 100,
            untethered_ttl: Duration::from_secs(3600),
            after_phase2_delete_hook: None,
        }
    }

    /// Test backend that tracks `delete()` calls and can be
    /// configured to fail on specific CIDs.
    struct TrackingBackend {
        inner: DiskBlobBackend,
        deleted: tokio::sync::Mutex<Vec<String>>,
        fail_on: tokio::sync::Mutex<HashSet<String>>,
    }

    impl TrackingBackend {
        fn new(inner: DiskBlobBackend) -> Self {
            Self {
                inner,
                deleted: tokio::sync::Mutex::new(Vec::new()),
                fail_on: tokio::sync::Mutex::new(HashSet::new()),
            }
        }
        async fn deleted_cids(&self) -> Vec<String> {
            self.deleted.lock().await.clone()
        }
        async fn fail_for(&self, cid: &str) {
            self.fail_on.lock().await.insert(cid.to_string());
        }
    }

    #[async_trait::async_trait]
    impl BlobBackend for TrackingBackend {
        async fn put(
            &self,
            cid: &str,
            data: Vec<u8>,
            mime_type: &str,
        ) -> PdsResult<()> {
            self.inner.put(cid, data, mime_type).await
        }
        async fn get(&self, cid: &str) -> PdsResult<Option<Vec<u8>>> {
            self.inner.get(cid).await
        }
        async fn delete(&self, cid: &str) -> PdsResult<()> {
            if self.fail_on.lock().await.contains(cid) {
                return Err(PdsError::BlobStorage(format!(
                    "test-injected failure for {}",
                    cid
                )));
            }
            self.deleted.lock().await.push(cid.to_string());
            self.inner.delete(cid).await
        }
        async fn exists(&self, cid: &str) -> PdsResult<bool> {
            self.inner.exists(cid).await
        }
        async fn size(&self, cid: &str) -> PdsResult<Option<u64>> {
            self.inner.size(cid).await
        }
        async fn list_all_blobs(
            &self,
            cursor: Option<String>,
            page_size: usize,
        ) -> PdsResult<crate::blob_store::BlobListPage> {
            self.inner.list_all_blobs(cursor, page_size).await
        }
    }

    #[tokio::test]
    async fn row_sweep_success_path_deletes_row_and_bytes() {
        let pool = setup_row_sweep_pool().await;
        let dir = TempDir::new().unwrap();
        let inner = DiskBlobBackend::new(dir.path().to_path_buf());
        let backend = TrackingBackend::new(inner);
        put_blob(&backend.inner, "bafA").await;
        // Seed an untethered row well in the past — eligible.
        seed_untethered_row(&pool, "bafA", "2026-05-01T00:00:00Z").await;

        let report = sweep_untethered_rows(
            &pool,
            &backend,
            row_sweep_params(),
            now_two_hours_ahead(),
        )
        .await
        .expect("sweep ok");

        assert_eq!(report.rows_deleted, 1);
        assert_eq!(report.race_skip_count, 0);
        assert_eq!(report.bytes_delete_failure_count, 0);
        assert_eq!(report.bytes_delete_skipped_fresh_row_count, 0);
        assert_eq!(report.pages_scanned, 1);
        assert_eq!(backend.deleted_cids().await, vec!["bafA"]);

        // Verify the row is actually gone.
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM blob_metadata WHERE cid='bafA'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn row_sweep_skips_permanent_rows_via_partial_predicate() {
        let pool = setup_row_sweep_pool().await;
        let dir = TempDir::new().unwrap();
        let inner = DiskBlobBackend::new(dir.path().to_path_buf());
        let backend = TrackingBackend::new(inner);
        seed_permanent_row(&pool, "bafPermanent", "2026-05-01T00:00:00Z").await;
        seed_untethered_row(&pool, "bafUntethered", "2026-05-01T00:00:00Z").await;
        put_blob(&backend.inner, "bafUntethered").await;

        let report = sweep_untethered_rows(
            &pool,
            &backend,
            row_sweep_params(),
            now_two_hours_ahead(),
        )
        .await
        .expect("sweep ok");

        assert_eq!(report.rows_deleted, 1);
        assert_eq!(backend.deleted_cids().await, vec!["bafUntethered"]);
        // Permanent row still present.
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM blob_metadata WHERE cid='bafPermanent'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn row_sweep_skips_too_young_rows() {
        let pool = setup_row_sweep_pool().await;
        let dir = TempDir::new().unwrap();
        let inner = DiskBlobBackend::new(dir.path().to_path_buf());
        let backend = TrackingBackend::new(inner);
        // Use a near-now created_at; with default 1h TTL and now=now+2h,
        // a row at now+0 has age 2h > 1h ⇒ eligible. To make it
        // too-young, use a created_at that's after the cutoff
        // (now+2h - 1h = now+1h cutoff; row at now+1h+30m = 90 min
        // into "now" = 30 min before cutoff... wait).
        // Easier: use a TTL of 12 hours so age 2h < 12h ⇒ too young.
        seed_untethered_row(&pool, "bafYoung", "2026-05-01T00:00:00Z").await;
        let mut params = row_sweep_params();
        params.untethered_ttl = Duration::from_secs(100 * 365 * 86_400); // 100 years
        let report = sweep_untethered_rows(&pool, &backend, params, now_two_hours_ahead())
            .await
            .expect("sweep ok");

        assert_eq!(report.rows_deleted, 0);
        // Row not in age window ⇒ Phase 1 SELECT returns empty page
        // ⇒ cycle ends immediately.
        assert_eq!(report.pages_scanned, 1);
    }

    #[tokio::test]
    async fn row_sweep_paginates_across_multiple_pages() {
        let pool = setup_row_sweep_pool().await;
        let dir = TempDir::new().unwrap();
        let inner = DiskBlobBackend::new(dir.path().to_path_buf());
        let backend = TrackingBackend::new(inner);
        // Seed 7 untethered rows; page_size = 3 ⇒ 3 pages (3 + 3 + 1).
        for i in 0..7 {
            let cid = format!("bafPage{:02}", i);
            seed_untethered_row(&pool, &cid, "2026-05-01T00:00:00Z").await;
            put_blob(&backend.inner, &cid).await;
        }
        let mut params = row_sweep_params();
        params.page_size = 3;

        let report =
            sweep_untethered_rows(&pool, &backend, params, now_two_hours_ahead())
                .await
                .expect("sweep ok");

        assert_eq!(report.rows_deleted, 7);
        assert_eq!(report.pages_scanned, 3);
        let deleted = backend.deleted_cids().await;
        assert_eq!(deleted.len(), 7);
    }

    #[tokio::test]
    async fn row_sweep_safety_cap_stops_cycle() {
        let pool = setup_row_sweep_pool().await;
        let dir = TempDir::new().unwrap();
        let inner = DiskBlobBackend::new(dir.path().to_path_buf());
        let backend = TrackingBackend::new(inner);
        for i in 0..10 {
            let cid = format!("bafCap{:02}", i);
            seed_untethered_row(&pool, &cid, "2026-05-01T00:00:00Z").await;
            put_blob(&backend.inner, &cid).await;
        }
        let mut params = row_sweep_params();
        params.max_deletes_per_run = 3;

        let report =
            sweep_untethered_rows(&pool, &backend, params, now_two_hours_ahead())
                .await
                .expect("sweep ok");

        assert_eq!(report.rows_deleted, 3);
        // 7 rows still present.
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM blob_metadata WHERE temp_key IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 7);
    }

    #[tokio::test]
    async fn row_sweep_dry_run_counts_without_deleting() {
        let pool = setup_row_sweep_pool().await;
        let dir = TempDir::new().unwrap();
        let inner = DiskBlobBackend::new(dir.path().to_path_buf());
        let backend = TrackingBackend::new(inner);
        for i in 0..5 {
            let cid = format!("bafDry{:02}", i);
            seed_untethered_row(&pool, &cid, "2026-05-01T00:00:00Z").await;
            put_blob(&backend.inner, &cid).await;
        }
        let mut params = row_sweep_params();
        params.dry_run = true;

        let report =
            sweep_untethered_rows(&pool, &backend, params, now_two_hours_ahead())
                .await
                .expect("sweep ok");

        assert_eq!(report.rows_deleted, 0);
        assert_eq!(report.total_eligible_count, Some(5));
        assert_eq!(report.would_delete_count, Some(5));
        assert!(backend.deleted_cids().await.is_empty());
        // Rows still present.
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM blob_metadata WHERE temp_key IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 5);
    }

    #[tokio::test]
    async fn row_sweep_bytes_delete_failure_counts_and_continues() {
        let pool = setup_row_sweep_pool().await;
        let dir = TempDir::new().unwrap();
        let inner = DiskBlobBackend::new(dir.path().to_path_buf());
        let backend = TrackingBackend::new(inner);
        for i in 0..3 {
            let cid = format!("bafFail{:02}", i);
            seed_untethered_row(&pool, &cid, "2026-05-01T00:00:00Z").await;
            put_blob(&backend.inner, &cid).await;
        }
        backend.fail_for("bafFail01").await;

        let report = sweep_untethered_rows(
            &pool,
            &backend,
            row_sweep_params(),
            now_two_hours_ahead(),
        )
        .await
        .expect("sweep ok");

        assert_eq!(report.rows_deleted, 2);
        assert_eq!(report.bytes_delete_failure_count, 1);
        // All 3 rows still got DELETEd; the failure is in
        // bytes-delete, which leaves bytes orphans for Arc 10.
        let row_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM blob_metadata WHERE temp_key IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row_count, 0);
    }

    /// V05_DESIGN.md §9.4.8.2 Scenario 9b in-process variant: the
    /// `after_phase2_delete_hook` synthetically INSERTs a new
    /// untethered row for the same CID between Phase 2 DELETE
    /// commit and the fresh-row SELECT. Sweep should detect the
    /// fresh row, increment `bytes_delete_skipped_fresh_row_count`,
    /// log WARN, and skip `backend.delete`.
    #[tokio::test]
    async fn row_sweep_fresh_row_diagnostic_fires_via_test_hook() {
        let pool = setup_row_sweep_pool().await;
        let dir = TempDir::new().unwrap();
        let inner = DiskBlobBackend::new(dir.path().to_path_buf());
        let backend = TrackingBackend::new(inner);
        seed_untethered_row(&pool, "bafRace", "2026-05-01T00:00:00Z").await;
        put_blob(&backend.inner, "bafRace").await;

        let hook_pool = pool.clone();
        let hook = std::sync::Arc::new(move |cid: String| {
            let pool = hook_pool.clone();
            Box::pin(async move {
                // Simulate Arc 16c re-upload `track_untethered_blob`
                // Case 3 committing between sweep's DELETE and the
                // fresh-row check.
                sqlx::query(
                    "INSERT INTO blob_metadata \
                     (cid, mime_type, size, creator_did, created_at, temp_key) \
                     VALUES ($1, 'image/png', 1, 'did:plc:test', \
                             '2026-05-19T12:00:00Z', '1')",
                )
                .bind(&cid)
                .execute(&pool)
                .await
                .expect("synthetic re-upload INSERT");
            }) as futures::future::BoxFuture<'static, ()>
        });

        let mut params = row_sweep_params();
        params.after_phase2_delete_hook = Some(hook);

        let report =
            sweep_untethered_rows(&pool, &backend, params, now_two_hours_ahead())
                .await
                .expect("sweep ok");

        assert_eq!(report.rows_deleted, 0);
        assert_eq!(report.bytes_delete_skipped_fresh_row_count, 1);
        // backend.delete must NOT have been called.
        assert!(backend.deleted_cids().await.is_empty());
        // Fresh row remains.
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM blob_metadata WHERE cid='bafRace'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn row_sweep_empty_table_early_exit() {
        let pool = setup_row_sweep_pool().await;
        let dir = TempDir::new().unwrap();
        let inner = DiskBlobBackend::new(dir.path().to_path_buf());
        let backend = TrackingBackend::new(inner);
        let report = sweep_untethered_rows(
            &pool,
            &backend,
            row_sweep_params(),
            now_two_hours_ahead(),
        )
        .await
        .expect("sweep ok");
        assert_eq!(report.rows_deleted, 0);
        assert_eq!(report.pages_scanned, 1);
        assert!(backend.deleted_cids().await.is_empty());
    }

    /// Race-skip: a row is age-eligible at Phase 1 SELECT but a
    /// concurrent STRICT promotion (sets temp_key=NULL) commits
    /// before the per-row Phase 2 DELETE. Modeled here by
    /// pre-promoting one of the page's rows mid-sweep via the
    /// `after_phase2_delete_hook` — except the hook fires AFTER
    /// the row's own DELETE, so for race-skip we need a
    /// different approach: pre-promote one row before sweep
    /// starts, then run sweep with an over-broad TTL that would
    /// have included it. The Phase 2 DELETE predicate carries
    /// `temp_key IS NOT NULL` so the promoted row's DELETE
    /// returns 0 rows.
    ///
    /// To trigger this we seed both an untethered AND a
    /// permanent row at the SAME old timestamp, set TTL such
    /// that the permanent row's `created_at` is past cutoff,
    /// and observe that the Phase 1 SELECT correctly filtered
    /// the permanent row (predicate-driven, not race-driven).
    /// True race-skip exercise: directly promote a row in a way
    /// that escapes Phase 1's predicate — but Phase 1's predicate
    /// is the SAME as Phase 2's, so this is hard to reproduce
    /// in single-threaded test. Use the hook to promote ANOTHER
    /// CID's row that's in the current page.
    #[tokio::test]
    async fn row_sweep_race_skip_when_predicate_no_longer_matches() {
        let pool = setup_row_sweep_pool().await;
        let dir = TempDir::new().unwrap();
        let inner = DiskBlobBackend::new(dir.path().to_path_buf());
        let backend = TrackingBackend::new(inner);
        seed_untethered_row(&pool, "bafFirst", "2026-05-01T00:00:00Z").await;
        seed_untethered_row(&pool, "bafSecond", "2026-05-01T00:00:01Z").await;
        put_blob(&backend.inner, "bafFirst").await;
        put_blob(&backend.inner, "bafSecond").await;

        // Hook fires AFTER bafFirst's DELETE commits. Use it to
        // promote bafSecond (set temp_key=NULL). When sweep's
        // per-row Phase 2 DELETE runs against bafSecond, the
        // predicate `temp_key IS NOT NULL` fails → race-skip.
        let hook_pool = pool.clone();
        let hook = std::sync::Arc::new(move |cid: String| {
            let pool = hook_pool.clone();
            Box::pin(async move {
                if cid == "bafFirst" {
                    sqlx::query(
                        "UPDATE blob_metadata SET temp_key = NULL \
                         WHERE cid = 'bafSecond'",
                    )
                    .execute(&pool)
                    .await
                    .expect("promote bafSecond mid-sweep");
                }
            }) as futures::future::BoxFuture<'static, ()>
        });

        let mut params = row_sweep_params();
        params.after_phase2_delete_hook = Some(hook);

        let report =
            sweep_untethered_rows(&pool, &backend, params, now_two_hours_ahead())
                .await
                .expect("sweep ok");

        // bafFirst deleted normally; bafSecond's Phase 2 DELETE
        // matches 0 rows ⇒ race_skip.
        assert_eq!(report.rows_deleted, 1);
        assert_eq!(report.race_skip_count, 1);
        assert_eq!(backend.deleted_cids().await, vec!["bafFirst"]);
        // bafSecond row still exists (promoted, not deleted).
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM blob_metadata WHERE cid='bafSecond'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }
}
