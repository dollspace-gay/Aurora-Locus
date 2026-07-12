-- Phase ε (chainlink #422, Arc 2): the atproto-OAuth provider's device
-- registry (LOCKED §3.2 step 8 / β Inv-9 finding).
--
-- A DEDICATED table, parallel to the legacy `device` + `account_device` tables
-- (which are legacy-oauth-shaped: `device.session_id NOT NULL` references a
-- legacy oauth session, no `did`, and the legacy `DeviceManager` has no live
-- consumers). The strangler-fig boundary (SD-A2 = (c)) holds in the device
-- model too: the atproto provider ships its own did-keyed, browser-session-
-- aligned device registry rather than bending the legacy tables.
--
-- A device is a durable holder-side identity: it persists across many issued
-- tokens, carries a holder-supplied name, and holds the DPoP public key the
-- device signs its bearer-bound requests with. Phase ε.3 gates general-XRPC
-- OAuth-bearer access on the DPoP proof key matching a registered (non-revoked)
-- device row for the bearer's DID — a bearer alone is not enough; the key must
-- be a known device.
--
-- Columns:
--   device_id        opaque uuid PK.
--   did              holder DID (FK to actor, ON DELETE CASCADE — Arc 1 did:web
--                    pattern).
--   dpop_public_key  the device's DPoP public key, JWK-serialised.
--   dpop_jkt         RFC 7638 SHA-256 thumbprint of the JWK (fast lookup key for
--                    the ε.3 gate; equals the `token.dpop_thumbprint` a bearer
--                    issued to this device binds to).
--   device_name      holder-supplied label ("MacBook home"); nullable.
--   user_agent       captured at registration; diagnostic; nullable.
--   created_at       RFC3339.
--   last_seen_at     RFC3339; touched on each successful bearer request (ε.3).
--   revoked_at       RFC3339; soft-delete (audit-preserving). A revoked device
--                    no longer gates any bearer.

CREATE TABLE atproto_device (
    device_id        TEXT PRIMARY KEY,
    did              TEXT NOT NULL,
    dpop_public_key  TEXT NOT NULL,
    dpop_jkt         TEXT NOT NULL,
    device_name      TEXT,
    user_agent       TEXT,
    created_at       TEXT NOT NULL,
    last_seen_at     TEXT NOT NULL,
    revoked_at       TEXT,
    FOREIGN KEY (did) REFERENCES actor(did) ON DELETE CASCADE
);

CREATE INDEX idx_atproto_device_did ON atproto_device (did) WHERE revoked_at IS NULL;
-- One active device per DPoP key (global): a key identifies a single device, so
-- the same key cannot be an active device for two accounts at once. Revoked
-- rows are exempt, so re-registering a rotated-back key is fine.
CREATE UNIQUE INDEX idx_atproto_device_jkt ON atproto_device (dpop_jkt) WHERE revoked_at IS NULL;
