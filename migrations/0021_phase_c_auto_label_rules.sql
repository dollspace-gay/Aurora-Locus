-- §5.5.4 Phase C / chainlink #347 — auto-label rules (§3).
--
-- Operator-defined rules that auto-apply labels when substrate-observable
-- conditions are met (report-count / operator-action / account-age-activity
-- triggers). SuperAdmin CRUD; soft-delete via deleted_at; capped at 100
-- active rules. enabled is a nullable-INTEGER bool (sqlx::Any convention).
CREATE TABLE moderation_auto_label_rule (
    id                   TEXT PRIMARY KEY,
    trigger_type         TEXT NOT NULL,
    trigger_params       TEXT NOT NULL,           -- JSON, per-type schema (§3.4)
    label_value          TEXT NOT NULL,
    subject_scope        TEXT NOT NULL,           -- post | account | both
    enabled              INTEGER NOT NULL,
    created_at           TEXT NOT NULL,
    created_by_did       TEXT NOT NULL,
    last_modified_at     TEXT NOT NULL,
    last_modified_by_did TEXT NOT NULL,
    rationale            TEXT,
    deleted_at           TEXT
);

CREATE INDEX idx_auto_label_enabled ON moderation_auto_label_rule(enabled);
-- Active-rule partial index (§3.5 / MD-39). Both SQLite and Postgres support
-- the partial WHERE; the enabled flag + 100-rule cap bound scan cost if a
-- future backend ever lacked it.
CREATE INDEX idx_auto_label_active ON moderation_auto_label_rule(deleted_at) WHERE deleted_at IS NULL;

-- Pipeline B (operator-action) window count: subject account + action within
-- a window, excluding substrate-emitted (did:system) rows. (§3.5)
CREATE INDEX idx_audit_subject_created_action ON audit_chain_entry(subject_did, created_at, action);

-- §3.8 label provenance — which rule applied a label and from what source.
-- Both nullable: pre-Phase-C rows (Phase A default-action holds, manual labels)
-- read back NULL, which the dedup contract resolves as existing_source=None.
-- The `src` column already exists (it is the labeling-AUTHORITY DID, not the
-- decision provenance — distinct concept).
ALTER TABLE label ADD COLUMN rule_id TEXT;
ALTER TABLE label ADD COLUMN source TEXT;

-- NOTE: the design's idx_records_author_created is intentionally omitted —
-- Aurora-Locus has no global records table (records live in per-actor SQLite
-- stores), so Pipeline C counts posts against the author's own store at
-- create_record time. (Local-idiom translation, recorded per Nova's Decision 2.)
