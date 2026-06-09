-- v0.8 Arc 1 / chainlink #180 — persistent bind-audit orphan marker.
-- See docs/internal/design/v08_arc1.md §2 for the full schema rationale.
--
-- Closes the v0.7 Step 3.5 deferral: today `commit_with_orphan_recovery`
-- emits a `tracing::error!` orphan marker when the per-actor record
-- commit fails AFTER the shared audit tx already committed (the
-- audit-first relay-race ordering). That tracing event is forensic-only
-- — no DB row, nothing for a reconciliation sweep to act on. This table
-- is the persistent sibling: one row per orphaned `moderation_event`,
-- joined back via `moderation_event_id`, walked by the reconciliation
-- sweep (§4) which verifies whether the actor's record eventually landed.
--
-- Dual-backend type discipline (§2.4 / §7, #130 invariant):
--   * Every timestamp column is TEXT (RFC3339), never TIMESTAMPTZ — the
--     read path goes through sqlx::Any, whose type-compat set deliberately
--     excludes chrono::DateTime<Utc>; TIMESTAMPTZ silently breaks reads
--     on PG (the migration 0012 lesson).
--   * `state` is TEXT, never a PG ENUM — ENUM-via-sqlx::Any is the same
--     class of silent-decode trap. Parsed in Rust.
--   * `id` / `moderation_event_id` mirror moderation_event.id exactly so
--     the i64 read pattern (try_get::<i64, _>) carries over (mirrors the
--     mod_event_seq.moderation_event_id pairing precedent, 0006).
--
-- `subject_uri` is NOT NULL (§2.1 / round-1 L2): the only v0.7 orphan-able
-- emit, KryphocronAudienceUpdated, unconditionally populates it with the
-- actor's `at://{did}/{collection}/{rkey}` record URI. Future orphan-able
-- emits that legitimately can't supply one migrate the column then.

CREATE TABLE bind_audit_orphan_marker (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    moderation_event_id   INTEGER NOT NULL,
    actor_did             TEXT NOT NULL,
    subject_uri           TEXT NOT NULL,
    actor_commit_error    TEXT NOT NULL,
    state                 TEXT NOT NULL DEFAULT 'unresolved',
    created_at            TEXT NOT NULL,
    resolved_at           TEXT,
    resolution_detail     TEXT,
    UNIQUE (moderation_event_id)
);

-- Serves the reconciliation sweep's keyset walk directly:
--   WHERE state = 'unresolved' AND id > $cursor ORDER BY id ASC LIMIT N
-- `id` is strictly monotonic via AUTOINCREMENT, so a single-column cursor
-- needs no tie-break (unlike created_at, which ties within an RFC3339
-- second across concurrent inserts).
CREATE INDEX idx_bind_audit_orphan_marker_state_id
    ON bind_audit_orphan_marker (state, id);
