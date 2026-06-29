-- v0.9 Federation runtime-mutability arc §3.2 (#391) — restart-coordination
-- marker table (Postgres). The save-and-restart handlers (D-phase) upsert a
-- marker here in the SAME outer transaction as the runtime-settings write
-- (§3.5); the boot hook (§3.3) reads + dispatches markers on the next start.
-- NOT a runtime_settings key — not allowlisted, not exposed via the settings
-- XRPC, not audited. Internal restart coordination; the operator-visible audit
-- lives on the runtime_settings write it composes with.
--
-- `action` is the PRIMARY KEY: re-queuing the same field upserts via
-- INSERT ... ON CONFLICT(action) DO UPDATE. `payload` is opaque JSON carrying an
-- integer "version" field for forward compatibility.
--
-- NOTE: the postgres migration tree is one version ahead of the sqlite tree
-- (0024 vs 0023 at integration_hooks); this is 0025 here, 0024 there.
CREATE TABLE pending_restart_action (
    action     TEXT PRIMARY KEY,
    payload    TEXT NOT NULL,
    created_at TEXT NOT NULL
);
