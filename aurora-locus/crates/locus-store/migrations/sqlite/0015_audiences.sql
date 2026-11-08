-- A local audience's locus-minted rkey → the rkey the AppView minted for it.
CREATE TABLE kr_audience_alias (
    did          TEXT NOT NULL,
    local_rkey   TEXT NOT NULL,
    prism_rkey   TEXT NOT NULL,
    mapped_at    INTEGER NOT NULL,
    flagged      TEXT,
    PRIMARY KEY (did, local_rkey)
) STRICT;
CREATE INDEX kr_audience_alias_prism ON kr_audience_alias (did, prism_rkey);

-- Every local delete of an audience row, so a stale listing page doesn't
-- bring it back.
CREATE TABLE kr_audience_tombstone (
    did         TEXT NOT NULL,
    rkey        TEXT NOT NULL,
    deleted_at  INTEGER NOT NULL,
    PRIMARY KEY (did, rkey)
) STRICT;
CREATE INDEX kr_audience_tombstone_at ON kr_audience_tombstone (deleted_at);

-- Audience creates sent to the AppView, kept while the outcome is unknown.
CREATE TABLE kr_audience_attempt (
    id                 TEXT PRIMARY KEY NOT NULL,
    did                TEXT NOT NULL,
    value_json         TEXT NOT NULL,
    created_at_value   TEXT NOT NULL,
    updated_at_value   TEXT NOT NULL,
    created_at         INTEGER NOT NULL,
    expires_at         INTEGER NOT NULL,
    outcome            TEXT NOT NULL,
    local_rkey         TEXT,
    local_row_version  INTEGER
) STRICT;
CREATE INDEX kr_audience_attempt_did ON kr_audience_attempt (did, outcome);
CREATE UNIQUE INDEX kr_audience_attempt_open_local ON kr_audience_attempt (did, local_rkey)
    WHERE local_rkey IS NOT NULL AND outcome IN ('sending', 'unknown');

-- Per-account refresh state and the two refresh limiters.
CREATE TABLE kr_audience_sync (
    did                          TEXT PRIMARY KEY NOT NULL,
    last_listing_started_at      INTEGER,
    last_complete_at             INTEGER,
    next_poll_at                 INTEGER NOT NULL DEFAULT 0,
    last_owner_refresh_at        INTEGER NOT NULL DEFAULT 0,
    last_third_party_refresh_at  INTEGER NOT NULL DEFAULT 0,
    last_error                   TEXT
) STRICT;
CREATE INDEX kr_audience_sync_poll ON kr_audience_sync (next_poll_at);
