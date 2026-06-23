-- Postgres variant of migrations/0023_integration_hooks.sql.
-- See that file for the chainlink #350 / Integration hooks Phase A motivation.
CREATE TABLE moderation_integration_hook (
    id                   TEXT PRIMARY KEY,
    name                 TEXT NOT NULL,
    url                  TEXT NOT NULL,
    event_classes        TEXT NOT NULL,
    description          TEXT,
    enabled              INTEGER NOT NULL,
    created_at           TEXT NOT NULL,
    created_by_did       TEXT NOT NULL,
    last_modified_at     TEXT NOT NULL,
    last_modified_by_did TEXT NOT NULL,
    rationale            TEXT,
    deleted_at           TEXT
);

CREATE INDEX idx_integration_hook_enabled ON moderation_integration_hook(enabled);
CREATE INDEX idx_integration_hook_active ON moderation_integration_hook(deleted_at) WHERE deleted_at IS NULL;
