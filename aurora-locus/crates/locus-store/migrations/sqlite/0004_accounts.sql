-- Accounts, legacy sessions, app passwords, invites and email tokens.
-- Handles and emails are stored lowercased; uniqueness covers every account,
-- whatever its status.

CREATE TABLE account (
    did                   TEXT PRIMARY KEY NOT NULL,
    handle                TEXT NOT NULL,
    email                 TEXT,
    email_confirmed_at    INTEGER,
    password_hash         TEXT,
    created_at            INTEGER NOT NULL,
    status                TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'deactivated', 'takendown', 'suspended', 'deleted')),
    status_ref            TEXT,
    deactivated_at        INTEGER,
    delete_after          INTEGER,
    suspended_until       INTEGER,
    invites_disabled      INTEGER NOT NULL DEFAULT 0,
    private_tier_blocked  INTEGER NOT NULL DEFAULT 0,
    did_method            TEXT NOT NULL CHECK (did_method IN ('plc', 'web')),
    write_block_owner     TEXT,
    write_block_op        TEXT
) STRICT;
CREATE UNIQUE INDEX account_handle ON account (handle);
CREATE UNIQUE INDEX account_email ON account (email) WHERE email IS NOT NULL;

-- A session is one login. Access tokens carry its id; revoking it ends every
-- token issued for it on the next request.
CREATE TABLE session (
    id                 TEXT PRIMARY KEY NOT NULL,
    did                TEXT NOT NULL,
    kind               TEXT NOT NULL CHECK (kind IN ('legacy', 'oauth')),
    scope              TEXT NOT NULL,
    app_password_name  TEXT,
    created_at         INTEGER NOT NULL,
    expires_at         INTEGER NOT NULL,
    revoked_at         INTEGER
) STRICT;
CREATE INDEX session_did ON session (did);

-- Refresh tokens rotate; a used token keeps next_id so a replay inside the
-- grace period gets the same next token.
CREATE TABLE refresh_token (
    id          TEXT PRIMARY KEY NOT NULL,
    session_id  TEXT NOT NULL REFERENCES session (id) ON DELETE CASCADE,
    expires_at  INTEGER NOT NULL,
    next_id     TEXT
) STRICT;
CREATE INDEX refresh_token_session ON refresh_token (session_id);

CREATE TABLE app_password (
    did          TEXT NOT NULL,
    name         TEXT NOT NULL,
    lookup_hash  TEXT NOT NULL,
    hash         TEXT NOT NULL,
    privileged   INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL,
    PRIMARY KEY (did, name)
) STRICT;
CREATE INDEX app_password_lookup ON app_password (did, lookup_hash);

CREATE TABLE invite_code (
    code             TEXT PRIMARY KEY NOT NULL,
    available_uses   INTEGER NOT NULL,
    disabled         INTEGER NOT NULL DEFAULT 0,
    for_account      TEXT NOT NULL,
    created_by       TEXT NOT NULL,
    created_at       INTEGER NOT NULL
) STRICT;
CREATE INDEX invite_code_for ON invite_code (for_account);

CREATE TABLE invite_code_use (
    code     TEXT NOT NULL,
    used_by  TEXT NOT NULL,
    used_at  INTEGER NOT NULL,
    PRIMARY KEY (code, used_by)
) STRICT;

CREATE TABLE email_token (
    did           TEXT NOT NULL,
    purpose       TEXT NOT NULL,
    token_hash    TEXT NOT NULL,
    requested_at  INTEGER NOT NULL,
    PRIMARY KEY (did, purpose)
) STRICT;
