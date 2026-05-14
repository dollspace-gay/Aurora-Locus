//! `aurora-locus gc-sweep` CLI subcommand.
//!
//! Runs a one-shot Arc 10 GC sweep against the offline PDS.
//! The sweep itself is the Step 2 primitive at
//! [`crate::blob_store::gc::run_sweep`]; this module just
//! wires CLI args + config overrides + offline-lock
//! acquisition + human-readable output.
//!
//! **Offline-only.** The CLI acquires the same `LivenessLock`
//! that `serve` would, so it fast-fails if a PDS is running
//! against the same database. For online sweeps, operators
//! enable the scheduled `gc_sweep_job` via
//! `PDS_GC_SWEEP_ENABLED=true` (V04_DESIGN.md §9.4.3 ratified
//! the offline-only CLI / online-only scheduled split).
//!
//! Added in Arc 10 Step 3 (chainlink #57).

use std::time::Duration;

use crate::{
    context::AppContext,
    db::liveness_lock::LivenessLock,
    error::{PdsError, PdsResult},
};

/// Run the `gc-sweep` subcommand against `ctx`.
///
/// CLI overrides land on top of `config.gc_sweep`:
///
/// * `dry_run` flag forces `params.dry_run = true` regardless
///   of config — the safety direction. There is no
///   `--no-dry-run` because the config already exposes
///   destructive mode (`PDS_GC_SWEEP_DRY_RUN=false`) and
///   adding a CLI override would let an operator bypass the
///   intentional "edit config + restart" gate that v0.4 sets
///   for destructive sweeps.
/// * `report_only` flag forces `params.report_only = true`.
///   In the Step 2 sweep loop this currently has the same
///   effect as `dry_run` (both early-continue before the
///   delete branch), but the operator-intent disambiguation
///   matters for telemetry and the operator doc.
/// * `max_deletes`, `threshold_secs`, `page_size` overrides
///   replace the corresponding config values when present.
pub async fn run(
    ctx: &AppContext,
    dry_run: bool,
    report_only: bool,
    max_deletes: Option<usize>,
    threshold_secs: Option<u64>,
    page_size: Option<usize>,
) -> PdsResult<()> {
    // Offline check (V04_DESIGN.md §9.4.3 ratified the CLI as
    // offline-only). `LivenessLock::acquire` is non-blocking
    // on both backends; a held lock fast-fails.
    let _liveness_guard = LivenessLock::acquire(&ctx.config).await.map_err(|e| {
        PdsError::Validation(format!(
            "Cannot run gc-sweep: {} \
             Stop the PDS before running gc-sweep, or enable the \
             scheduled `gc_sweep_job` via PDS_GC_SWEEP_ENABLED=true \
             for online sweeps.",
            e
        ))
    })?;

    // Build SweepParams from config + CLI overrides.
    let mut params = ctx.config.gc_sweep.to_sweep_params(report_only);
    if dry_run {
        params.dry_run = true;
    }
    if let Some(max) = max_deletes {
        params.max_deletes_per_run = max;
    }
    if let Some(secs) = threshold_secs {
        params.freshness_threshold = Duration::from_secs(secs);
    }
    if let Some(size) = page_size {
        params.page_size = size;
    }

    println!("GC sweep starting:");
    println!("  dry_run:             {}", params.dry_run);
    println!("  report_only:         {}", params.report_only);
    println!("  max_deletes_per_run: {}", params.max_deletes_per_run);
    println!("  freshness_threshold: {:?}", params.freshness_threshold);
    println!("  page_size:           {}", params.page_size);
    println!();

    let now = chrono::Utc::now();
    let report = ctx.blob_store.run_gc_sweep(params, now).await?;

    println!("GC sweep complete:");
    println!("  pages scanned:               {}", report.pages_scanned);
    println!("  blobs examined:              {}", report.blobs_examined);
    println!("  authorized:                  {}", report.authorized);
    println!("  in-flight:                   {}", report.in_flight);
    println!("  too young:                   {}", report.too_young);
    println!(
        "  confirmed orphans found:     {}",
        report.confirmed_orphans_found
    );
    println!("  orphans deleted:             {}", report.orphans_deleted);
    println!(
        "  orphans skipped (safety cap): {}",
        report.orphans_skipped_safety_cap
    );
    println!("  duration:                    {:.2}s", report.duration_seconds);

    Ok(())
}
