-- First-run setup: one row, made by the first node to start. Until
-- completed_at is set the install is in setup mode.
CREATE TABLE setup_state (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    token_hash    TEXT NOT NULL,
    created_at    BIGINT NOT NULL,
    completed_at  BIGINT,
    completed_by  TEXT,
    -- The PDS hostname setup chose; nodes serve it from then on.
    hostname      TEXT,
    -- The wizard's answers so far (no passwords, no SMTP URL).
    draft_json    TEXT NOT NULL DEFAULT '{}',
    -- Probe nonce for the reachability check through the public URL, and
    -- how the last probe arrived.
    probe_nonce   TEXT,
    probe_seen    TEXT
);

-- Wizard sessions, made by entering the setup token. Only a hash is kept.
CREATE TABLE setup_session (
    id_hash     TEXT PRIMARY KEY,
    expires_at  BIGINT NOT NULL
);
