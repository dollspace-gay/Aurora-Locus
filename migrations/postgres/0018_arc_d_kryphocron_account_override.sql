-- v0.9 Arc D / chainlink #316 — per-account kryphocron overrides (Postgres).
-- See migrations/0017_arc_d_kryphocron_account_override.sql for the full
-- rationale; the schema is backend-identical — nullable INTEGER tri-state
-- booleans (no BOOLEAN, per the sqlx::Any type-compat discipline) and TEXT
-- (RFC3339) timestamps.

CREATE TABLE kryphocron_account_override (
    did                     TEXT PRIMARY KEY,
    rate_limit_exempt       INTEGER,
    capability_issuance     INTEGER,
    last_modified_at        TEXT NOT NULL,
    last_modified_by_did    TEXT NOT NULL,
    last_modified_rationale TEXT
);

CREATE INDEX idx_kryphocron_account_override_modified
    ON kryphocron_account_override (last_modified_at);
