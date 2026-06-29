-- Postgres variant of migrations/0015_arc_e_operator_session_rotation.sql.
-- See that file (and design §8.1.7) for the chainlink #272 rotation-on-use
-- motivation. PG stays one migration number ahead of SQLite (the 0012
-- TIMESTAMPTZ→TEXT fix); each backend tracks its own _sqlx_migrations.
--
-- TEXT (RFC3339), nullable: NULL until the session's first rotation.

ALTER TABLE operator_session ADD COLUMN refresh_rotated_at TEXT;
