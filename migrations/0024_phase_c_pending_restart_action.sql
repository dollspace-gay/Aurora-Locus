-- v0.9 Federation runtime-mutability arc §3.2 (#391) — restart-coordination
-- marker table. The save-and-restart handlers (D-phase) upsert a marker here in
-- the SAME outer transaction as the runtime-settings write (§3.5); the boot hook
-- (§3.3) reads + dispatches markers on the next start. This is NOT a
-- runtime_settings key — not allowlisted, not exposed via the settings XRPC, not
-- audited. It's internal restart coordination, not an operator setting; the
-- operator-visible audit lives on the runtime_settings write it composes with.
--
-- `action` is the PRIMARY KEY: re-queuing the same field upserts via
-- INSERT ... ON CONFLICT(action) DO UPDATE (portable across SQLite + Postgres).
-- `payload` is opaque JSON and always carries an integer "version" field for
-- forward compatibility (the boot hook skips unknown versions).
CREATE TABLE pending_restart_action (
    action     TEXT PRIMARY KEY,
    payload    TEXT NOT NULL,
    created_at TEXT NOT NULL
);
