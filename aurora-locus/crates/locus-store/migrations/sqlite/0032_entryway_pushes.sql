-- Entryway role: the durable per-shard outbox of locus callbacks
-- (revokeSessions, setPrivateTierBlock, setVerifyKeys), delivered per shard
-- in id order and kept until the shard acknowledges; the token-key
-- rotation. (Shards confirmed for a pending PLC rotation key are in
-- plc_rotation_key.confirmed_shards.)
CREATE TABLE shard_outbox (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    shard       TEXT NOT NULL,
    kind        TEXT NOT NULL,
    subject     TEXT NOT NULL,
    body        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    attempts    INTEGER NOT NULL DEFAULT 0,
    next_at     INTEGER NOT NULL,
    last_error  TEXT
) STRICT;
CREATE INDEX shard_outbox_shard ON shard_outbox (shard, id);

CREATE TABLE entryway_key_rotation (
    id            INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    old_key       TEXT NOT NULL,
    new_key       TEXT NOT NULL,
    started_at    INTEGER NOT NULL,
    acknowledged  TEXT NOT NULL DEFAULT '[]',
    switched_at   INTEGER,
    finished_at   INTEGER
) STRICT;
