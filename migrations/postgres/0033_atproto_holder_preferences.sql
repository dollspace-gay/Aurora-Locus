-- Holder UI Phase 1 (chainlink #424, Arc 2 follow-on): per-holder display
-- preferences for the did:web holder self-service UI. Postgres counterpart of
-- sqlite 0032. See that file for the full rationale (first per-account
-- preferences store; `runtime_settings` is operator-tier, not per-holder).
--
-- Sub-phase 1.1 ships the table only; the manager + preferences page (the
-- reader/writer) land in sub-phase 1.4.

CREATE TABLE atproto_holder_preferences (
    did        TEXT PRIMARY KEY,
    theme      TEXT,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);
