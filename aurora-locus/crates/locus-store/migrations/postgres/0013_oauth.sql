-- OAuth: authorization requests, devices and their accounts, authorized
-- clients, tokens and spent refresh tokens, replay windows, and resolved
-- permission-set lexicons. Times are ms since the epoch.

CREATE TABLE oauth_request (
    id           TEXT PRIMARY KEY,
    did          TEXT,
    device_id    TEXT,
    client_id    TEXT NOT NULL,
    client_auth  TEXT,
    parameters   TEXT NOT NULL,
    expires_at   BIGINT NOT NULL,
    code         TEXT UNIQUE
);
CREATE INDEX oauth_request_expires ON oauth_request (expires_at);

CREATE TABLE oauth_device (
    id            TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL,
    user_agent    TEXT,
    ip_address    TEXT NOT NULL,
    last_seen_at  BIGINT NOT NULL
);

CREATE TABLE oauth_account_device (
    did         TEXT NOT NULL,
    device_id   TEXT NOT NULL,
    created_at  BIGINT NOT NULL,
    updated_at  BIGINT NOT NULL,
    PRIMARY KEY (did, device_id)
);
CREATE INDEX oauth_account_device_device ON oauth_account_device (device_id);

CREATE TABLE oauth_authorized_client (
    did         TEXT NOT NULL,
    client_id   TEXT NOT NULL,
    created_at  BIGINT NOT NULL,
    updated_at  BIGINT NOT NULL,
    data        TEXT NOT NULL,
    PRIMARY KEY (did, client_id)
);

CREATE TABLE oauth_token (
    id                     BIGSERIAL PRIMARY KEY,
    did                    TEXT NOT NULL,
    token_id               TEXT NOT NULL UNIQUE,
    created_at             BIGINT NOT NULL,
    updated_at             BIGINT NOT NULL,
    expires_at             BIGINT NOT NULL,
    client_id              TEXT NOT NULL,
    client_auth            TEXT NOT NULL,
    device_id              TEXT,
    parameters             TEXT NOT NULL,
    details                TEXT,
    code                   TEXT UNIQUE,
    current_refresh_token  TEXT UNIQUE,
    scope                  TEXT
);
CREATE INDEX oauth_token_did ON oauth_token (did);
CREATE INDEX oauth_token_device ON oauth_token (device_id);

CREATE TABLE oauth_used_refresh (
    refresh_token  TEXT PRIMARY KEY,
    token_id       BIGINT NOT NULL
);
CREATE INDEX oauth_used_refresh_token ON oauth_used_refresh (token_id);

-- Values that may be used once within a window (DPoP jti, client
-- assertions, request objects, code challenges).
CREATE TABLE oauth_replay (
    namespace   TEXT NOT NULL,
    value       TEXT NOT NULL,
    expires_at  BIGINT NOT NULL,
    PRIMARY KEY (namespace, value)
);
CREATE INDEX oauth_replay_expires ON oauth_replay (expires_at);

CREATE TABLE oauth_lexicon (
    nsid               TEXT PRIMARY KEY,
    created_at         BIGINT NOT NULL,
    updated_at         BIGINT NOT NULL,
    last_succeeded_at  BIGINT,
    uri                TEXT,
    lexicon            TEXT
);
