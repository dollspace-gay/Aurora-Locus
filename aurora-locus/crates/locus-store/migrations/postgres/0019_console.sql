-- Console sign-ins. Only a hash of the session id is kept. A session
-- signed in with a password but not yet its sign-in code is at stage
-- 'code'; a full session is at stage 'full'.
CREATE TABLE operator_session (
    id_hash       TEXT PRIMARY KEY,
    operator      TEXT NOT NULL,
    stage         TEXT NOT NULL CHECK (stage IN ('code', 'full')),
    created_at    BIGINT NOT NULL,
    last_seen_at  BIGINT NOT NULL,
    expires_at    BIGINT NOT NULL,
    -- When the operator last confirmed their password (and code): sensitive
    -- actions need a recent one.
    confirmed_at  BIGINT NOT NULL,
    ip            TEXT,
    user_agent    TEXT
);
CREATE INDEX operator_session_operator ON operator_session (operator);

-- Each operator's own look of the console.
ALTER TABLE operator ADD COLUMN theme_mode TEXT NOT NULL DEFAULT 'system';
ALTER TABLE operator ADD COLUMN theme_scheme TEXT NOT NULL DEFAULT 'sky';
ALTER TABLE operator ADD COLUMN theme_font TEXT NOT NULL DEFAULT 'hyperlegible';
-- The last TOTP time step accepted, so a code works once.
ALTER TABLE operator ADD COLUMN totp_last_step BIGINT;
