-- Accounts, legacy sessions, app passwords, invites and email tokens.
-- Handles and emails are stored lowercased; uniqueness covers every account,
-- whatever its status.

CREATE TABLE account (
    did                   TEXT PRIMARY KEY,
    handle                TEXT NOT NULL,
    email                 TEXT,
    email_confirmed_at    BIGINT,
    password_hash         TEXT,
    created_at            BIGINT NOT NULL,
    status                TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'deactivated', 'takendown', 'suspended', 'deleted')),
    status_ref            TEXT,
    deactivated_at        BIGINT,
    delete_after          BIGINT,
    suspended_until       BIGINT,
    invites_disabled      BOOLEAN NOT NULL DEFAULT FALSE,
    private_tier_blocked  BOOLEAN NOT NULL DEFAULT FALSE,
    did_method            TEXT NOT NULL CHECK (did_method IN ('plc', 'web')),
    write_block_owner     TEXT,
    write_block_op        TEXT
);
CREATE UNIQUE INDEX account_handle ON account (handle);
CREATE UNIQUE INDEX account_email ON account (email) WHERE email IS NOT NULL;

-- A session is one login. Access tokens carry its id; revoking it ends every
-- token issued for it on the next request.
CREATE TABLE session (
    id                 TEXT PRIMARY KEY,
    did                TEXT NOT NULL,
    kind               TEXT NOT NULL CHECK (kind IN ('legacy', 'oauth')),
    scope              TEXT NOT NULL,
    app_password_name  TEXT,
    created_at         BIGINT NOT NULL,
    expires_at         BIGINT NOT NULL,
    revoked_at         BIGINT
);
CREATE INDEX session_did ON session (did);

-- Refresh tokens rotate; a used token keeps next_id so a replay inside the
-- grace period gets the same next token.
CREATE TABLE refresh_token (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES session (id) ON DELETE CASCADE,
    expires_at  BIGINT NOT NULL,
    next_id     TEXT
);
CREATE INDEX refresh_token_session ON refresh_token (session_id);

CREATE TABLE app_password (
    did          TEXT NOT NULL,
    name         TEXT NOT NULL,
    lookup_hash  TEXT NOT NULL,
    hash         TEXT NOT NULL,
    privileged   BOOLEAN NOT NULL DEFAULT FALSE,
    created_at   BIGINT NOT NULL,
    PRIMARY KEY (did, name)
);
CREATE INDEX app_password_lookup ON app_password (did, lookup_hash);

CREATE TABLE invite_code (
    code             TEXT PRIMARY KEY,
    available_uses   BIGINT NOT NULL,
    disabled         BOOLEAN NOT NULL DEFAULT FALSE,
    for_account      TEXT NOT NULL,
    created_by       TEXT NOT NULL,
    created_at       BIGINT NOT NULL
);
CREATE INDEX invite_code_for ON invite_code (for_account);

CREATE TABLE invite_code_use (
    code     TEXT NOT NULL,
    used_by  TEXT NOT NULL,
    used_at  BIGINT NOT NULL,
    PRIMARY KEY (code, used_by)
);

CREATE TABLE email_token (
    did           TEXT NOT NULL,
    purpose       TEXT NOT NULL,
    token_hash    TEXT NOT NULL,
    requested_at  BIGINT NOT NULL,
    PRIMARY KEY (did, purpose)
);
