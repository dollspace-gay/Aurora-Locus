-- Shard role, behind a locus entryway: revoked sessions (`sid`), or an
-- account's `not_before` (sid = ''), kept until every token they cover
-- has expired; and the hash of the shard-callback token.
CREATE TABLE shard_revocation (
    did         TEXT NOT NULL,
    sid         TEXT NOT NULL,
    not_before  BIGINT,
    expires_at  BIGINT NOT NULL,
    PRIMARY KEY (did, sid)
);
CREATE INDEX shard_revocation_expires ON shard_revocation (expires_at);

CREATE TABLE shard_callback_token (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    token_hash    TEXT NOT NULL,
    installed_at  BIGINT NOT NULL
);
