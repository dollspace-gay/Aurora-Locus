-- Signing keys reserved by `reserveSigningKey` for an account not yet
-- created (keyed by the DID given, or the key's did:key), sealed, and
-- dropped once used or expired.
CREATE TABLE reserved_key (
    id          TEXT PRIMARY KEY NOT NULL,
    did_key     TEXT NOT NULL,
    sealed      BLOB NOT NULL,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL
) STRICT;
CREATE INDEX reserved_key_did_key ON reserved_key (did_key);
CREATE INDEX reserved_key_expires ON reserved_key (expires_at);

-- Shard role: the entryway's token verify keys accepted, each until
-- expires_at (NULL: no end).
CREATE TABLE shard_verify_key (
    did_key     TEXT PRIMARY KEY NOT NULL,
    added_at    INTEGER NOT NULL,
    expires_at  INTEGER
) STRICT;
