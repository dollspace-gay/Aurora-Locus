-- The numbered event log the firehose serves, filled from the outbox by
-- the sequencer (one at a time, leased), and the sequencer's state.

CREATE TABLE seq_event (
    seq           BIGINT PRIMARY KEY,
    did           TEXT NOT NULL,
    kind          TEXT NOT NULL,
    payload       BYTEA NOT NULL,
    sequenced_at  BIGINT NOT NULL
);
CREATE INDEX seq_event_did ON seq_event (did, seq);
CREATE INDEX seq_event_time ON seq_event (sequenced_at);

-- last_seq: the highest seq assigned. pruned_through: the highest seq
-- retention has removed (a cursor below it gets OutdatedCursor).
CREATE TABLE seq_state (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    last_seq        BIGINT NOT NULL DEFAULT 0,
    pruned_through  BIGINT NOT NULL DEFAULT 0
);
INSERT INTO seq_state (id) VALUES (1);

-- A deleted account's earlier events, removed once due (the preservation
-- window, default immediately).
CREATE TABLE seq_purge (
    did         TEXT PRIMARY KEY,
    before_seq  BIGINT NOT NULL,
    due_at      BIGINT NOT NULL
);
