-- Postgres variant of migrations/0005_audit_chain_cascade_snapshot_ids.sql.
-- See that file for the chainlink #111 motivation.

ALTER TABLE audit_chain_entry ADD COLUMN cascade_snapshot_ids TEXT;
