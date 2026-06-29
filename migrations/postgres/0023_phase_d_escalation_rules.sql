-- Postgres variant of migrations/0022_phase_d_escalation_rules.sql.
-- See that file for the chainlink #348 / §5.5.4 Phase D motivation.
CREATE TABLE moderation_escalation_rule (
    id                   TEXT PRIMARY KEY,
    trigger_type         TEXT NOT NULL,
    trigger_params       TEXT NOT NULL,
    action_type          TEXT NOT NULL,
    enabled              INTEGER NOT NULL,
    created_at           TEXT NOT NULL,
    created_by_did       TEXT NOT NULL,
    last_modified_at     TEXT NOT NULL,
    last_modified_by_did TEXT NOT NULL,
    rationale            TEXT,
    deleted_at           TEXT
);

CREATE INDEX idx_escalation_enabled ON moderation_escalation_rule(enabled);
CREATE INDEX idx_escalation_active ON moderation_escalation_rule(deleted_at) WHERE deleted_at IS NULL;

CREATE TABLE moderation_escalation_consumed (
    rule_id          TEXT NOT NULL,
    item_id          TEXT NOT NULL,
    deescalated_at   TEXT NOT NULL,
    deescalated_by   TEXT NOT NULL,
    PRIMARY KEY (rule_id, item_id)
);

CREATE INDEX idx_escalation_consumed_item ON moderation_escalation_consumed(item_id);

CREATE TABLE escalation_eval_lock (
    lock_key    TEXT PRIMARY KEY,
    acquired_at TEXT NOT NULL
);
