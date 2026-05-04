-- v0.2 cycle (chainlink #109) — Postgres variant of the SQLite migration
-- under migrations/0004_drop_admin_audit_log.sql.
--
-- DROP TABLE cascades to its indexes (idx_admin_audit_admin,
-- idx_admin_audit_timestamp) on Postgres without an explicit drop.

DROP TABLE IF EXISTS admin_audit_log;
