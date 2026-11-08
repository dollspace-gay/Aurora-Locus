-- Repositories: current root per account, reference-counted blocks, the
-- record index, and the event outbox written in each commit's transaction.

CREATE TABLE repo_root (
    did         TEXT PRIMARY KEY NOT NULL,
    commit_cid  TEXT NOT NULL,
    rev         TEXT NOT NULL,
    data_cid    TEXT NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;

-- MST nodes, record blocks and commit blocks. A block lives while
-- ref_count > 0; GC removes it some time after it reaches 0.
CREATE TABLE repo_block (
    did             TEXT NOT NULL,
    cid             TEXT NOT NULL,
    rev_introduced  TEXT NOT NULL,
    size            INTEGER NOT NULL,
    content         BLOB NOT NULL,
    ref_count       INTEGER NOT NULL,
    zero_since      INTEGER,
    PRIMARY KEY (did, cid)
) STRICT;
CREATE INDEX repo_block_zero ON repo_block (zero_since) WHERE ref_count = 0;
CREATE INDEX repo_block_rev ON repo_block (did, rev_introduced);

CREATE TABLE record (
    did           TEXT NOT NULL,
    collection    TEXT NOT NULL,
    rkey          TEXT NOT NULL,
    cid           TEXT NOT NULL,
    rev           TEXT NOT NULL,
    indexed_at    INTEGER NOT NULL,
    takedown_ref  TEXT,
    PRIMARY KEY (did, collection, rkey)
) STRICT;

-- Events written in the same transaction as the change they describe. The
-- sequencer moves them, in commit order, into the numbered event log.
CREATE TABLE outbox (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    did         TEXT NOT NULL,
    kind        TEXT NOT NULL,
    payload     BLOB NOT NULL,
    created_at  INTEGER NOT NULL
) STRICT;
