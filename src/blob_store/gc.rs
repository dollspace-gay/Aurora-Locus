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

use std::time::Duration;

use chrono::{DateTime, Utc};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
