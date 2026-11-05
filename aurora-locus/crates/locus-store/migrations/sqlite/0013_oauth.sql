-- OAuth: authorization requests, devices and their accounts, authorized
-- clients, tokens and spent refresh tokens, replay windows, and resolved
-- permission-set lexicons. Times are ms since the epoch.

CREATE TABLE oauth_request (
    id           TEXT PRIMARY KEY NOT NULL,
    did          TEXT,
    device_id    TEXT,
    client_id    TEXT NOT NULL,
    client_auth  TEXT,
    parameters   TEXT NOT NULL,
    expires_at   INTEGER NOT NULL,
    code         TEXT UNIQUE
) STRICT;
CREATE INDEX oauth_request_expires ON oauth_request (expires_at);

CREATE TABLE oauth_device (
    id            TEXT PRIMARY KEY NOT NULL,
    session_id    TEXT NOT NULL,
    user_agent    TEXT,
    ip_address    TEXT NOT NULL,
    last_seen_at  INTEGER NOT NULL
) STRICT;

CREATE TABLE oauth_account_device (
    did         TEXT NOT NULL,
    device_id   TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (did, device_id)
) STRICT;
CREATE INDEX oauth_account_device_device ON oauth_account_device (device_id);

CREATE TABLE oauth_authorized_client (
    did         TEXT NOT NULL,
    client_id   TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    data        TEXT NOT NULL,
    PRIMARY KEY (did, client_id)
) STRICT;

CREATE TABLE oauth_token (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    did                    TEXT NOT NULL,
    token_id               TEXT NOT NULL UNIQUE,
    created_at             INTEGER NOT NULL,
    updated_at             INTEGER NOT NULL,
    expires_at             INTEGER NOT NULL,
    client_id              TEXT NOT NULL,
    client_auth            TEXT NOT NULL,
    device_id              TEXT,
    parameters             TEXT NOT NULL,
    details                TEXT,
    code                   TEXT UNIQUE,
    current_refresh_token  TEXT UNIQUE,
    scope                  TEXT
) STRICT;
CREATE INDEX oauth_token_did ON oauth_token (did);
CREATE INDEX oauth_token_device ON oauth_token (device_id);

CREATE TABLE oauth_used_refresh (
    refresh_token  TEXT PRIMARY KEY NOT NULL,
    token_id       INTEGER NOT NULL
) STRICT;
CREATE INDEX oauth_used_refresh_token ON oauth_used_refresh (token_id);

-- Values that may be used once within a window (DPoP jti, client
-- assertions, request objects, code challenges).
CREATE TABLE oauth_replay (
    namespace   TEXT NOT NULL,
    value       TEXT NOT NULL,
    expires_at  INTEGER NOT NULL,
    PRIMARY KEY (namespace, value)
) STRICT;
CREATE INDEX oauth_replay_expires ON oauth_replay (expires_at);

CREATE TABLE oauth_lexicon (
    nsid               TEXT PRIMARY KEY NOT NULL,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    last_succeeded_at  INTEGER,
    uri                TEXT,
    lexicon            TEXT
) STRICT;
