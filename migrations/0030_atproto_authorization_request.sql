-- Phase β.3 (chainlink #420, Arc 2): the atproto-OAuth provider's
-- authorization-request store (LOCKED design §3.2 / R1 F-3.2).
--
-- A DEDICATED table, parallel to the legacy `authorization_request` table
-- (which is owned by `src/oauth/flow_state_adapter.rs` + the legacy
-- authorize/consent/token path). The strangler-fig boundary (SD-A2 = (c))
-- holds in the data model too: the new `/oauth/atproto/*` provider never
-- shares a row with the legacy provider, so the two evolve independently.
--
-- Two concrete reasons a parallel table (not a column-add on the legacy one):
--   1. PAR (§3.2 PAR endpoint): a pushed authorization request is persisted
--      BEFORE any holder authenticates, so `did` must be nullable here. The
--      legacy table pins `did NOT NULL`.
--   2. The legacy table is decoded by `row_to_authorization_request`
--      (flow_state_adapter) with its own column contract; layering atproto
--      rows onto it would couple the two providers' schemas.
--
-- Columns:
--   request_id            opaque CSPRNG id; the consent-form hidden field and
--                         the authorize→consent correlation key.
--   request_uri           the PAR `urn:ietf:params:oauth:request_uri:<opaque>`
--                         value; NULL for a direct (non-PAR) authorize. The
--                         authorize endpoint accepts `request_uri` in lieu of
--                         the individual parameters.
--   client_id             the client's `client-metadata.json` URL.
--   redirect_uri          exact-match redirect target (verified against client
--                         metadata at authorize + token time).
--   scope                 space-separated atproto-spec scopes.
--   state                 opaque client value, echoed on the redirect; nullable.
--   code_challenge        PKCE S256 challenge.
--   code_challenge_method always "S256" (the only method advertised).
--   did                   the holder DID, bound at the authorize/consent step
--                         from the browser session; NULL until then (PAR).
--   code_hash             SHA-256 of the issued authorization code; NULL until
--                         consent mints the code. The raw code is never stored
--                         (mirrors the β.1 bearer-hash discipline).
--   code_used_at          RFC3339; set once at token redemption — the single-use
--                         enforcement column (CAS: ... WHERE code_used_at IS NULL).
--   denied_at             RFC3339; set when the holder denies consent.
--   created_at            RFC3339.
--   expires_at            RFC3339; ~10 min for an authorize request, ~60s for a
--                         bare PAR request awaiting authorize.

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
