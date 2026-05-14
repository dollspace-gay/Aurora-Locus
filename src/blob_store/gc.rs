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
use crate::error::PdsResult;

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
}
