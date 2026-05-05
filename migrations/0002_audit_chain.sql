-- Phase 3.8 — hash-chained audit log + snapshot infrastructure
-- (chainlink #105 / docs/AURORA_ADMIN_UI_DESIGN.md §3.4, §8.4, §8.7).
--
-- Two co-equal tables:
--   audit_chain_entry — append-only chain of operator decisions.
--                       Hash linkage via current_hash + previous_hash
--                       gives tamper-evident replay.
--   audit_snapshot   — content captured at decision time. Referenced
--                       by chain entries via snapshot_id.
--
-- Pre-Phase-3.8 events have neither — getAuditTrail emits a sentinel
-- row with current_hash="pre-chain" and verified=false for those.
-- See §8.4 behavior note.

CREATE TABLE audit_chain_entry (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    sequence        INTEGER NOT NULL,
    created_at      TEXT NOT NULL,
    actor_did       TEXT NOT NULL,
    action          TEXT NOT NULL,
    subject_did     TEXT,
    subject_uri     TEXT,
    subject_cid     TEXT,
    rationale       TEXT NOT NULL,
    snapshot_id     INTEGER,
    event_id        INTEGER,
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
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    captured_at     TEXT NOT NULL,
    subject_did     TEXT,
    subject_uri     TEXT,
    subject_cid     TEXT,
    -- Compact JSON capture of subject state at decision time. For
    -- account subjects: handle, takedown_ref, deactivated_at, active
    -- moderation action. For record/blob subjects: shape TBD per
    -- v0.3 work; v0.2 captures whatever fields are available.
    content         TEXT NOT NULL,
    content_hash    TEXT NOT NULL
);

CREATE INDEX idx_audit_snapshot_subject_did ON audit_snapshot(subject_did);
CREATE INDEX idx_audit_snapshot_captured ON audit_snapshot(captured_at);
