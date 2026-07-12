-- Phase ε (chainlink #422, Arc 2): the atproto-OAuth provider's device
-- registry. Postgres counterpart of sqlite 0031. See that file for the full
-- rationale (LOCKED §3.2 step 8 / β Inv-9): a DEDICATED did-keyed device table
-- parallel to the legacy `device`/`account_device` tables, so the strangler-fig
-- boundary (SD-A2 = (c)) holds in the device model. Phase ε.3 gates general-XRPC
-- OAuth-bearer access on the DPoP proof key matching a registered device row.

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
CREATE UNIQUE INDEX idx_atproto_device_jkt ON atproto_device (dpop_jkt) WHERE revoked_at IS NULL;
