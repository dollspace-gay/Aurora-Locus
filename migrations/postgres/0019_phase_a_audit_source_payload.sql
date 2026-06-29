-- Postgres variant of migrations/0018_phase_a_audit_source_payload.sql.
-- See that file for the chainlink #345 / §5.5.4 Phase A motivation and the
-- v0.9 canonical-hash format bump rationale.
--
-- TEXT (not JSONB) for `payload` deliberately: the sqlx::Any layer binds a
-- Rust String, and the column mirrors the cascade_subjects/cascade_snapshot_ids
-- TEXT-holding-JSON convention so write/verify hash the identical byte string
-- on both backends.

ALTER TABLE audit_chain_entry ADD COLUMN source TEXT NOT NULL DEFAULT 'manual';
ALTER TABLE audit_chain_entry ADD COLUMN payload TEXT;

CREATE INDEX idx_audit_chain_source ON audit_chain_entry(source);
