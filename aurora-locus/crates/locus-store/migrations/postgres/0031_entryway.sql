-- Entryway role: the shards behind it (their admin password and, for a
-- locus shard, the shard-callback token are in the key store), and each
-- account's shard.
CREATE TABLE shard (
    name      TEXT PRIMARY KEY,
    url       TEXT NOT NULL UNIQUE,
    did       TEXT NOT NULL UNIQUE,
    locus     BOOLEAN NOT NULL,
    weight    BIGINT NOT NULL DEFAULT 1,
    draining  BOOLEAN NOT NULL DEFAULT FALSE,
    added_at  BIGINT NOT NULL
);

ALTER TABLE account ADD COLUMN shard TEXT;
CREATE INDEX account_shard ON account (shard);
