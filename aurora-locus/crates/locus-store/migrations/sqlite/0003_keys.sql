-- The master key check, named secrets and per-account signing keys.
-- Secrets are sealed with the master key; the database never holds one in
-- the clear.

CREATE TABLE master_key_check (
    id           INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    key_id       TEXT NOT NULL,
    source_kind  TEXT NOT NULL,
    mac          BLOB NOT NULL,
    created_at   INTEGER NOT NULL
) STRICT;

CREATE TABLE secret (
    name        TEXT PRIMARY KEY NOT NULL,
    sealed      BLOB NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;

CREATE TABLE signing_key (
    did         TEXT PRIMARY KEY NOT NULL,
    key_type    TEXT NOT NULL,
    sealed      BLOB NOT NULL,
    created_at  INTEGER NOT NULL
) STRICT;
