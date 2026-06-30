-- Phase β.2 (chainlink #420, Arc 2): the browser-session substrate for the
-- atproto-OAuth provider's resource-owner authentication (LOCKED design §3.4 /
-- R1 F-3.3). The new `/oauth/atproto/*` provider's authorize + consent
-- endpoints resolve the holder DID from a server-side session keyed by an
-- opaque HttpOnly cookie — Aurora had no browser-shaped session substrate at
-- HEAD (both existing auth extractors are Bearer-token / XRPC-shaped).
--
-- The session is created by the AS-login endpoint (login-α: a DPoP-style
-- challenge-response proving control of the holder's #atproto key) and never
-- carries signing authority — it authenticates the holder to the consent
-- flow, it does not authorize the substrate to sign (pre-decision 3).
--
-- Columns:
--   id            opaque CSPRNG session id (the cookie value).
--   did           the authenticated holder DID.
--   created_at    RFC3339; session creation.
--   last_seen_at  RFC3339; refreshed each validated request (idle-expiry input).
--   expires_at    RFC3339; absolute lifetime ceiling.
--   csrf_token    per-session anti-CSRF token for the consent POSTs (the
--                 authorization request_id is NOT a trust token — F-3.2).
--   user_agent    diagnostic only.
--   ip_hash       SHA-256 of the client IP (never the raw IP); optional
--                 diagnostic / anomaly signal.

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
