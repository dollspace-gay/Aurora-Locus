-- Entryway role: the shards behind it (their admin password and, for a
-- locus shard, the shard-callback token are in the key store), and each
-- account's shard.
CREATE TABLE shard (
    name      TEXT PRIMARY KEY NOT NULL,
    url       TEXT NOT NULL UNIQUE,
    did       TEXT NOT NULL UNIQUE,
    locus     INTEGER NOT NULL,
    weight    INTEGER NOT NULL DEFAULT 1,
    draining  INTEGER NOT NULL DEFAULT 0,
    added_at  INTEGER NOT NULL
) STRICT;

ALTER TABLE account ADD COLUMN shard TEXT;
CREATE INDEX account_shard ON account (shard);
