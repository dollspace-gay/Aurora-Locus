-- §5.5.4 Phase D / chainlink #348 — escalation rules (§5).
--
-- Operator-defined rules that auto-escalate queue items (status='escalated')
-- on severity signals. SuperAdmin CRUD; soft-delete; capped at 100 active.
-- The only path into status='escalated' in v0.9 is rule firing (no manual
-- escalation, per §5.1 HM-CC).
CREATE TABLE moderation_escalation_rule (
    id                   TEXT PRIMARY KEY,
    trigger_type         TEXT NOT NULL,           -- report-count | operator-action | category-match
    trigger_params       TEXT NOT NULL,           -- JSON, per-type schema (§5.7)
    action_type          TEXT NOT NULL,           -- mark | reassign-to-superadmin
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

-- §5.6 consumed-row mechanism: a (rule, item) pair that was de-escalated is
-- recorded so the rule does not immediately re-fire on the same item.
CREATE TABLE moderation_escalation_consumed (
    rule_id          TEXT NOT NULL,
    item_id          TEXT NOT NULL,
    deescalated_at   TEXT NOT NULL,
    deescalated_by   TEXT NOT NULL,
    PRIMARY KEY (rule_id, item_id)
);

CREATE INDEX idx_escalation_consumed_item ON moderation_escalation_consumed(item_id);

-- §5.9 row-level serialization advisory rows. Ships unconditionally; used by
-- the SQLite path (INSERT unique-key = contention signal, DELETE on release).
-- The Postgres path additionally takes SELECT ... FOR UPDATE on the report row
-- inside the mutation transaction. `acquired_at` enables stale-lock stealing
-- after a process crash mid-evaluation.
CREATE TABLE escalation_eval_lock (
    lock_key    TEXT PRIMARY KEY,
    acquired_at TEXT NOT NULL
);
