-- v0.9 Arc H §7.4.3 / chainlink #291 — bulk repository-repair scan findings.
--
-- The persistent findings store for the across-accounts repository scan
-- (§7.4.3). A scan walks every account, structurally reconstructs its repo
-- from the sequencer (the #289 fast path, no full-sig verification), and
-- compares the reconstructed head against the live repo head. Each
-- inconsistency lands a row here so the operator can review findings across
-- admin-UI sessions (run scan → leave → return → triage → repair).
--
-- Severity (locked, category-based — derived from the structural scan alone,
-- no extra per-account walk):
--   * high   = reconstruction fails / live repo unrebuildable (no live root,
--              or no sequencer history backing a live repo)
--   * medium = reconstructed head CID != live head CID (a real inconsistency
--              a rebuild fixes)
--   * low    = heads match but rev differs (minor drift)
-- Consistent accounts produce no row.
--
-- Dual-backend type discipline (mirrors 0014 / the #130 invariant): every
-- timestamp is TEXT (RFC3339), never TIMESTAMPTZ — the read path is sqlx::Any,
-- whose type-compat set excludes chrono::DateTime<Utc>. No FK on `did`
-- (AS-only accounts / mid-scan deletions must not break the scan), and no FK
-- to a scan-run table (the scan run is in-memory live state per #224's
-- RewriteJob shape; the findings ARE the persistent artifact). `scan_id` is an
-- opaque per-run UUID, also carried by the ScanCompleted audit event so a
-- finding set ties back to the run that produced it.
--
-- PK (scan_id, did): one finding per account per run; a re-find within a run
-- upserts rather than duplicating. The covering index backs the
-- getRepoScanResults read path — by scan, optionally filtered by severity,
-- keyset-paginated by did.

CREATE TABLE repo_scan_finding (
    scan_id       TEXT NOT NULL,
    did           TEXT NOT NULL,
    severity      TEXT NOT NULL,
    live_head     TEXT,
    recon_head    TEXT,
    detail        TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    PRIMARY KEY (scan_id, did)
);

CREATE INDEX idx_repo_scan_finding_severity ON repo_scan_finding (scan_id, severity, did);
