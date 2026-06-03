-- Postgres variant of migrations/0013_arc1_bind_audit_orphan_marker.sql.
-- See that file (and docs/internal/design/v08_arc1.md §2) for the v0.8
-- Arc 1 / chainlink #180 motivation and the dual-backend type discipline.
--
-- PG is one migration number ahead of SQLite (0012 was the PG-only
-- TIMESTAMPTZ→TEXT fix); sqlx tracks each backend in its own
-- _sqlx_migrations table, so the asymmetry is benign.
--
-- `id` is BIGSERIAL (vs SQLite AUTOINCREMENT), `moderation_event_id` is
-- BIGINT (vs INTEGER) — both read as i64 via try_get::<i64, _>, mirroring
-- the moderation_event.id / mod_event_seq pairing. Every timestamp is TEXT
-- (RFC3339), `state` is TEXT (not ENUM): sqlx::Any type-compat discipline.

CREATE TABLE bind_audit_orphan_marker (
    id                    BIGSERIAL PRIMARY KEY,
    moderation_event_id   BIGINT NOT NULL,
    actor_did             TEXT NOT NULL,
    subject_uri           TEXT NOT NULL,
    actor_commit_error    TEXT NOT NULL,
    state                 TEXT NOT NULL DEFAULT 'unresolved',
    created_at            TEXT NOT NULL,
    resolved_at           TEXT,
    resolution_detail     TEXT,
    UNIQUE (moderation_event_id)
);

CREATE INDEX idx_bind_audit_orphan_marker_state_id
    ON bind_audit_orphan_marker (state, id);
