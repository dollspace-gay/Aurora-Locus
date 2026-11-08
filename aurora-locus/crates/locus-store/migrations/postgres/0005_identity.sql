-- The PDS's PLC rotation keys, which held keys each account's PLC document
-- lists, resolution caches and hosted did:web documents.

-- Every row is a held key; the private half is in the key store under
-- `plc_rotation_key/<did_key>`.
CREATE TABLE plc_rotation_key (
    did_key           TEXT PRIMARY KEY,
    role              TEXT NOT NULL CHECK (role IN ('pending', 'current', 'secondary')),
    created_at        BIGINT NOT NULL,
    current_since     BIGINT,
    secondary_since   BIGINT,
    confirmed_shards  TEXT NOT NULL DEFAULT '[]'
);
CREATE UNIQUE INDEX plc_rotation_key_one_current ON plc_rotation_key (role) WHERE role = 'current';
CREATE UNIQUE INDEX plc_rotation_key_one_pending ON plc_rotation_key (role) WHERE role = 'pending';

-- The held keys the account's latest PLC document lists, in document order
-- (a JSON array).
CREATE TABLE plc_key_listing (
    did          TEXT PRIMARY KEY,
    listed_keys  TEXT NOT NULL,
    checked_at   BIGINT NOT NULL
);

CREATE TABLE did_cache (
    did         TEXT PRIMARY KEY,
    doc         TEXT NOT NULL,
    updated_at  BIGINT NOT NULL
);

-- A NULL did is a negative entry (the handle didn't resolve).
CREATE TABLE handle_cache (
    handle      TEXT PRIMARY KEY,
    did         TEXT,
    updated_at  BIGINT NOT NULL
);

-- Documents for did:web accounts hosted here. A row outlives its account:
-- the host stays tied to the DID forever, so no other account can take it.
CREATE TABLE hosted_did_web (
    did         TEXT PRIMARY KEY,
    host        TEXT NOT NULL UNIQUE,
    doc         TEXT,
    updated_at  BIGINT NOT NULL
);

-- Identity changes that must converge after a partial failure: a handle
-- change whose PLC op may have applied (`value` is the new handle), or a
-- tombstone for a DID locus created but couldn't use. The repair job
-- retries each until it's done.
CREATE TABLE identity_repair (
    did         TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('handle', 'tombstone')),
    value       TEXT NOT NULL,
    created_at  BIGINT NOT NULL,
    attempts    INTEGER NOT NULL DEFAULT 0,
    next_at     BIGINT NOT NULL,
    last_error  TEXT,
    PRIMARY KEY (did, kind)
);
CREATE INDEX identity_repair_due ON identity_repair (next_at);
