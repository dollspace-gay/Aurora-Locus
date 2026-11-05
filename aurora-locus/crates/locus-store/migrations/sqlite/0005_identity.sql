-- The PDS's PLC rotation keys, which held keys each account's PLC document
-- lists, resolution caches and hosted did:web documents.

-- Every row is a held key; the private half is in the key store under
-- `plc_rotation_key/<did_key>`.
CREATE TABLE plc_rotation_key (
    did_key           TEXT PRIMARY KEY NOT NULL,
    role              TEXT NOT NULL CHECK (role IN ('pending', 'current', 'secondary')),
    created_at        INTEGER NOT NULL,
    current_since     INTEGER,
    secondary_since   INTEGER,
    confirmed_shards  TEXT NOT NULL DEFAULT '[]'
) STRICT;
CREATE UNIQUE INDEX plc_rotation_key_one_current ON plc_rotation_key (role) WHERE role = 'current';
CREATE UNIQUE INDEX plc_rotation_key_one_pending ON plc_rotation_key (role) WHERE role = 'pending';

-- The held keys the account's latest PLC document lists, in document order
-- (a JSON array).
CREATE TABLE plc_key_listing (
    did          TEXT PRIMARY KEY NOT NULL,
    listed_keys  TEXT NOT NULL,
    checked_at   INTEGER NOT NULL
) STRICT;

CREATE TABLE did_cache (
    did         TEXT PRIMARY KEY NOT NULL,
    doc         TEXT NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;

-- A NULL did is a negative entry (the handle didn't resolve).
CREATE TABLE handle_cache (
    handle      TEXT PRIMARY KEY NOT NULL,
    did         TEXT,
    updated_at  INTEGER NOT NULL
) STRICT;

-- Documents for did:web accounts hosted here. A row outlives its account:
-- the host stays tied to the DID forever, so no other account can take it.
CREATE TABLE hosted_did_web (
    did         TEXT PRIMARY KEY NOT NULL,
    host        TEXT NOT NULL UNIQUE,
    doc         TEXT,
    updated_at  INTEGER NOT NULL
) STRICT;

-- Identity changes that must converge after a partial failure: a handle
-- change whose PLC op may have applied (`value` is the new handle), or a
-- tombstone for a DID locus created but couldn't use. The repair job
-- retries each until it's done.
CREATE TABLE identity_repair (
    did         TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('handle', 'tombstone')),
    value       TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    attempts    INTEGER NOT NULL DEFAULT 0,
    next_at     INTEGER NOT NULL,
    last_error  TEXT,
    PRIMARY KEY (did, kind)
) STRICT;
CREATE INDEX identity_repair_due ON identity_repair (next_at);
