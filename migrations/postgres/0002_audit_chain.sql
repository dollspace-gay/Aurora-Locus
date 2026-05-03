-- Phase 3.8 — hash-chained audit log + snapshot infrastructure
-- (chainlink #105 / docs/AURORA_ADMIN_UI_DESIGN.md §3.4, §8.4, §8.7).
-- Postgres counterpart of migrations/0002_audit_chain.sql.

CREATE TABLE audit_chain_entry (
    id              BIGSERIAL PRIMARY KEY,
    sequence        BIGINT NOT NULL,
    created_at      TEXT NOT NULL,
    actor_did       TEXT NOT NULL,
    action          TEXT NOT NULL,
    subject_did     TEXT,
    subject_uri     TEXT,
    subject_cid     TEXT,
    rationale       TEXT NOT NULL,
    snapshot_id     BIGINT,
    event_id        BIGINT,
    current_hash    TEXT NOT NULL,
    previous_hash   TEXT,
    cascade_subjects TEXT,
    UNIQUE(sequence)
);

CREATE INDEX idx_audit_chain_seq ON audit_chain_entry(sequence);
CREATE INDEX idx_audit_chain_created ON audit_chain_entry(created_at);
CREATE INDEX idx_audit_chain_actor ON audit_chain_entry(actor_did);
CREATE INDEX idx_audit_chain_subject_did ON audit_chain_entry(subject_did);

CREATE TABLE audit_snapshot (
    id              BIGSERIAL PRIMARY KEY,
    captured_at     TEXT NOT NULL,
    subject_did     TEXT,
    subject_uri     TEXT,
    subject_cid     TEXT,
    content         TEXT NOT NULL,
    content_hash    TEXT NOT NULL
);

CREATE INDEX idx_audit_snapshot_subject_did ON audit_snapshot(subject_did);
CREATE INDEX idx_audit_snapshot_captured ON audit_snapshot(captured_at);
