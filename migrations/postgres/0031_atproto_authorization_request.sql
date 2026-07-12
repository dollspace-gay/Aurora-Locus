-- Phase β.3 (chainlink #420, Arc 2): the atproto-OAuth provider's
-- authorization-request store. Postgres counterpart of sqlite 0030. See that
-- file for the full rationale (LOCKED design §3.2 / R1 F-3.2): a DEDICATED
-- table parallel to the legacy `authorization_request` table, so the
-- strangler-fig boundary (SD-A2 = (c)) holds in the data model. `did` is
-- nullable because PAR persists a request before any holder authenticates.

CREATE TABLE atproto_authorization_request (
    request_id              TEXT PRIMARY KEY,
    request_uri             TEXT,
    client_id               TEXT NOT NULL,
    redirect_uri            TEXT NOT NULL,
    scope                   TEXT NOT NULL,
    state                   TEXT,
    code_challenge          TEXT NOT NULL,
    code_challenge_method   TEXT NOT NULL,
    did                     TEXT,
    code_hash               TEXT,
    code_used_at            TEXT,
    denied_at               TEXT,
    created_at              TEXT NOT NULL,
    expires_at              TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_atproto_authreq_request_uri
    ON atproto_authorization_request (request_uri)
    WHERE request_uri IS NOT NULL;
CREATE INDEX idx_atproto_authreq_code_hash
    ON atproto_authorization_request (code_hash)
    WHERE code_hash IS NOT NULL;
CREATE INDEX idx_atproto_authreq_expires
    ON atproto_authorization_request (expires_at);
