-- Postgres variant of migrations/0021_phase_c_auto_label_rules.sql.
-- See that file for the chainlink #347 / §5.5.4 Phase C motivation and the
-- per-actor records translation (idx_records_author_created omitted).
CREATE TABLE moderation_auto_label_rule (
    id                   TEXT PRIMARY KEY,
    trigger_type         TEXT NOT NULL,
    trigger_params       TEXT NOT NULL,
    label_value          TEXT NOT NULL,
    subject_scope        TEXT NOT NULL,
    enabled              INTEGER NOT NULL,
    created_at           TEXT NOT NULL,
    created_by_did       TEXT NOT NULL,
    last_modified_at     TEXT NOT NULL,
    last_modified_by_did TEXT NOT NULL,
    rationale            TEXT,
    deleted_at           TEXT
);

CREATE INDEX idx_auto_label_enabled ON moderation_auto_label_rule(enabled);
CREATE INDEX idx_auto_label_active ON moderation_auto_label_rule(deleted_at) WHERE deleted_at IS NULL;
CREATE INDEX idx_audit_subject_created_action ON audit_chain_entry(subject_did, created_at, action);

ALTER TABLE label ADD COLUMN rule_id TEXT;
ALTER TABLE label ADD COLUMN source TEXT;
