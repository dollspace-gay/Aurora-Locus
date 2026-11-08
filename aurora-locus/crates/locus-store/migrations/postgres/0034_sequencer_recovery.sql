-- What `sequencer recover` did per account: recovered from the event log,
-- or failed (left for `repair repos --from-sequencer`, which marks it fixed).
CREATE TABLE sequencer_recovery (
    did           TEXT PRIMARY KEY,
    recovered_at  BIGINT NOT NULL,
    error         TEXT,
    fixed         BOOLEAN NOT NULL DEFAULT FALSE
);
