-- What `sequencer recover` did per account: recovered from the event log,
-- or failed (left for `repair repos --from-sequencer`, which marks it fixed).
CREATE TABLE sequencer_recovery (
    did           TEXT PRIMARY KEY NOT NULL,
    recovered_at  INTEGER NOT NULL,
    error         TEXT,
    fixed         INTEGER NOT NULL DEFAULT 0
) STRICT;
