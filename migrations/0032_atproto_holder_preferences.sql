-- Holder UI Phase 1 (chainlink #424, Arc 2 follow-on): per-holder display
-- preferences for the did:web holder self-service UI served under
-- `/oauth/atproto/holder/*`.
--
-- The FIRST per-account preferences store in the codebase. `runtime_settings`
-- (migration 0003) is operator-tier deployment config (key/value/tier), not
-- per-holder; nothing there is keyed by account. This table fills that gap for
-- the holder UI, keyed by the holder DID.
--
-- Sub-phase 1.1 ships the table only (no Rust reader/writer yet — SQL migration,
-- so no dead-code tax). The `AtprotoHolderPreferencesManager` + the preferences
-- page land in sub-phase 1.4, when the theme picker becomes the consumer.
--
-- Columns:
--   did         holder DID (PK, FK to actor, ON DELETE CASCADE — Arc 1 did:web
--               pattern, mirrors atproto_device). One preferences row per
--               holder; cascades away with the account.
--   theme       chosen display-theme id (e.g. "dark", "ember"); NULLABLE. NULL
--               means "use the operator's active theme" — the holder pages then
--               link `/theme/active.css` unqualified rather than
--               `/theme/active.css?id=<theme>`. An unknown/stale stored id
--               degrades gracefully: the theme serve route falls back to the
--               active theme rather than erroring.
--   updated_at  RFC3339 timestamp of the last preference write.

CREATE TABLE atproto_holder_preferences (
    did        TEXT PRIMARY KEY,
    theme      TEXT,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);
