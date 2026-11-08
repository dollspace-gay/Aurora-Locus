-- The numbered event log the firehose serves, filled from the outbox by
-- the sequencer (one at a time, leased), and the sequencer's state.

CREATE TABLE seq_event (
    seq           INTEGER PRIMARY KEY NOT NULL,
    did           TEXT NOT NULL,
    kind          TEXT NOT NULL,
    payload       BLOB NOT NULL,
    sequenced_at  INTEGER NOT NULL
) STRICT;
CREATE INDEX seq_event_did ON seq_event (did, seq);
CREATE INDEX seq_event_time ON seq_event (sequenced_at);

-- last_seq: the highest seq assigned. pruned_through: the highest seq
-- retention has removed (a cursor below it gets OutdatedCursor).
CREATE TABLE seq_state (
    id              INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    last_seq        INTEGER NOT NULL DEFAULT 0,
    pruned_through  INTEGER NOT NULL DEFAULT 0
) STRICT;
INSERT INTO seq_state (id) VALUES (1);

-- A deleted account's earlier events, removed once due (the preservation
-- window, default immediately).
CREATE TABLE seq_purge (
    did         TEXT PRIMARY KEY NOT NULL,
    before_seq  INTEGER NOT NULL,
    due_at      INTEGER NOT NULL
) STRICT;
