-- Phase 3.10 — runtime settings infrastructure (chainlink #117 /
-- docs/AURORA_ADMIN_UI_DESIGN.md §8.16, §5.5.2).
-- Postgres counterpart of migrations/0003_runtime_settings.sql.

CREATE TABLE runtime_settings (
    key              TEXT PRIMARY KEY,
    value            TEXT NOT NULL,
    last_modified    TEXT NOT NULL,
    last_modified_by TEXT NOT NULL
);

CREATE INDEX idx_runtime_settings_modified ON runtime_settings(last_modified);
