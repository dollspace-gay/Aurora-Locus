-- v0.2 cycle (chainlink #109) — drop the legacy `admin_audit_log` table.
--
-- The hash-chained `audit_chain_entry` table (added in 0002) is now the
-- system of record for every administrative decision. All call sites
-- previously routed through `AdminRoleManager::log_action` (which
-- INSERT'd into admin_audit_log) now write to audit_chain_entry, so the
-- legacy table is unreferenced and its presence would create the false
-- impression of two parallel audit surfaces.
--
-- v0.2 has not shipped to upstream so there are no external operators
-- with legacy rows to migrate. Internal deployments rebuild from
-- migration scratch when this lands.
--
-- The associated indexes (idx_admin_audit_admin, idx_admin_audit_timestamp)
-- are removed by the table drop in SQLite.

DROP TABLE IF EXISTS admin_audit_log;
