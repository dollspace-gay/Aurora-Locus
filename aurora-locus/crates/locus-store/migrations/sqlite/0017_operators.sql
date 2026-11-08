-- Operators run the PDS; they are not atproto accounts.
CREATE TABLE operator (
    username       TEXT PRIMARY KEY NOT NULL,
    password_hash  TEXT NOT NULL,
    totp_secret    TEXT,
    role           TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'moderator', 'viewer')),
    created_at     INTEGER NOT NULL,
    disabled       INTEGER NOT NULL DEFAULT 0
) STRICT;

-- API tokens for the admin API (the CLI over HTTP, automation). Only a
-- hash is kept.
CREATE TABLE operator_token (
    id            TEXT PRIMARY KEY NOT NULL,
    token_hash    TEXT NOT NULL UNIQUE,
    operator      TEXT NOT NULL,
    name          TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    last_used_at  INTEGER,
    expires_at    INTEGER
) STRICT;
CREATE INDEX operator_token_operator ON operator_token (operator);
