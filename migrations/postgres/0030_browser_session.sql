-- Phase β.2 (chainlink #420, Arc 2): browser-session substrate for the
-- atproto-OAuth provider's resource-owner authentication. Postgres
-- counterpart of sqlite 0029. See that file for the full rationale (LOCKED
-- design §3.4 / R1 F-3.3). The session authenticates the holder to the
-- consent flow via the AS-login challenge-response (login-α); it never
-- carries signing authority (pre-decision 3).

CREATE TABLE browser_session (
    id            TEXT PRIMARY KEY,
    did           TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    last_seen_at  TEXT NOT NULL,
    expires_at    TEXT NOT NULL,
    csrf_token    TEXT NOT NULL,
    user_agent    TEXT,
    ip_hash       TEXT
);

CREATE INDEX idx_browser_session_did ON browser_session(did);
CREATE INDEX idx_browser_session_expires ON browser_session(expires_at);
