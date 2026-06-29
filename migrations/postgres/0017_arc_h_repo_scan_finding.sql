-- v0.9 Arc H §7.4.3 / chainlink #291 — bulk repository-repair scan findings
-- (Postgres). See migrations/0016_arc_h_repo_scan_finding.sql for the full
-- rationale; the schema is backend-identical (no bool/serial columns), with
-- TEXT timestamps per the sqlx::Any type-compat discipline.

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
