-- Postgres variant of migrations/0014_arc_e_operator_session.sql.
-- See that file (and design §8.1.7) for the v0.9 Arc E 0.9.3 / chainlink
-- #271 per-operator session-store motivation and the dual-backend type
-- discipline.
--
-- PG stays one migration number ahead of SQLite (0012 was the PG-only
-- TIMESTAMPTZ→TEXT fix); sqlx tracks each backend in its own
-- _sqlx_migrations table, so the asymmetry is benign.
--
-- `revoked` is BOOLEAN (vs SQLite INTEGER), read via db::read_bool. Every
-- timestamp is TEXT (RFC3339), never TIMESTAMPTZ: sqlx::Any type-compat
-- discipline. No FK on `did` — AS-only operators have no actor row.

CREATE TABLE operator_session (
    id                    TEXT PRIMARY KEY,
    did                   TEXT NOT NULL,
    created_at            TEXT NOT NULL,
    last_active_at        TEXT NOT NULL,
    expires_at            TEXT NOT NULL,
    source_ip             TEXT,
    user_agent            TEXT,
    current_refresh_id    TEXT,
    prev_refresh_id       TEXT,
    revoked               BOOLEAN NOT NULL DEFAULT FALSE,
    revoked_at            TEXT,
    revoked_by            TEXT,
    revoke_reason         TEXT
);

CREATE INDEX idx_operator_session_did ON operator_session (did);
CREATE INDEX idx_operator_session_expires ON operator_session (expires_at);
