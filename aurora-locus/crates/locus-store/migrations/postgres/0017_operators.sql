-- Operators run the PDS; they are not atproto accounts.
CREATE TABLE operator (
    username       TEXT PRIMARY KEY,
    password_hash  TEXT NOT NULL,
    totp_secret    TEXT,
    role           TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'moderator', 'viewer')),
    created_at     BIGINT NOT NULL,
    disabled       BOOLEAN NOT NULL DEFAULT FALSE
);

-- API tokens for the admin API (the CLI over HTTP, automation). Only a
-- hash is kept.
CREATE TABLE operator_token (
    id            TEXT PRIMARY KEY,
    token_hash    TEXT NOT NULL UNIQUE,
    operator      TEXT NOT NULL,
    name          TEXT NOT NULL,
    created_at    BIGINT NOT NULL,
    last_used_at  BIGINT,
    expires_at    BIGINT
);
CREATE INDEX operator_token_operator ON operator_token (operator);
