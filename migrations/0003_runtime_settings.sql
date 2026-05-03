-- Phase 3.10 — runtime settings infrastructure (chainlink #117 /
-- docs/AURORA_ADMIN_UI_DESIGN.md §8.16, §5.5.2).
--
-- Two-tier configuration: file-level config (env vars, yaml) is
-- fallback; runtime setting takes precedence. Writes go here.
-- AURORA_RECOVERY_MODE=true env var bypasses runtime settings on
-- startup for emergency recovery.
--
-- Known keys (v0.2):
--   moderation-mode               "full" | "reduced" | "disabled"
--   moderation-mode-redirect-url  string URL or empty

CREATE TABLE runtime_settings (
    key              TEXT PRIMARY KEY,
    value            TEXT NOT NULL,
    last_modified    TEXT NOT NULL,
    last_modified_by TEXT NOT NULL
);

CREATE INDEX idx_runtime_settings_modified ON runtime_settings(last_modified);
