-- v0.9 Integration hooks Phase A / chainlink #350 — declaration table (§3.1).
--
-- Declaration-without-execution: this table records hook DECLARATIONS (where
-- + on what events a future cycle would deliver). v0.9 does NOT execute them;
-- the tripwire (design addendum §3) structurally prevents an execution sink.
-- enabled is a nullable-INTEGER bool; soft-delete one-way (no restore).
CREATE TABLE moderation_integration_hook (
    id                   TEXT PRIMARY KEY,
    name                 TEXT NOT NULL,
    url                  TEXT NOT NULL,           -- validated + WHATWG-normalized (§2.4)
    event_classes        TEXT NOT NULL,           -- JSON array of class strings (§3.2)
    description          TEXT,                     -- ≤ 4096 chars (design-commit 10)
    enabled              INTEGER NOT NULL,
    created_at           TEXT NOT NULL,
    created_by_did       TEXT NOT NULL,
    last_modified_at     TEXT NOT NULL,            -- optimistic-concurrency token (§4)
    last_modified_by_did TEXT NOT NULL,
    rationale            TEXT,
    deleted_at           TEXT                      -- one-way soft-delete (design-commit 21)
);

CREATE INDEX idx_integration_hook_enabled ON moderation_integration_hook(enabled);
CREATE INDEX idx_integration_hook_active ON moderation_integration_hook(deleted_at) WHERE deleted_at IS NULL;
