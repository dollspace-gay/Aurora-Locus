-- The master key check, named secrets and per-account signing keys.
-- Secrets are sealed with the master key; the database never holds one in
-- the clear.

CREATE TABLE master_key_check (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    key_id       TEXT NOT NULL,
    source_kind  TEXT NOT NULL,
    mac          BYTEA NOT NULL,
    created_at   BIGINT NOT NULL
);

CREATE TABLE secret (
    name        TEXT PRIMARY KEY,
    sealed      BYTEA NOT NULL,
    updated_at  BIGINT NOT NULL
);

CREATE TABLE signing_key (
    did         TEXT PRIMARY KEY,
    key_type    TEXT NOT NULL,
    sealed      BYTEA NOT NULL,
    created_at  BIGINT NOT NULL
);
