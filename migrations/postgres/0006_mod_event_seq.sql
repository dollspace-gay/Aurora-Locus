-- Postgres variant of migrations/0006_mod_event_seq.sql.
-- See that file for chainlink #115 motivation.

CREATE TABLE mod_event_seq (
    seq                  BIGSERIAL PRIMARY KEY,
    moderation_event_id  BIGINT NOT NULL,
    actor_did            TEXT NOT NULL,
    action               TEXT NOT NULL,
    subject_did          TEXT,
    subject_uri          TEXT,
    subject_cid          TEXT,
    detail               TEXT,
    created_at           TEXT NOT NULL
);

CREATE INDEX idx_mod_event_seq_created_at ON mod_event_seq(created_at);
CREATE INDEX idx_mod_event_seq_seq ON mod_event_seq(seq);
CREATE INDEX idx_mod_event_seq_subject_did ON mod_event_seq(subject_did);
CREATE INDEX idx_mod_event_seq_subject_uri ON mod_event_seq(subject_uri);
