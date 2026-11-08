-- Backups: one row per `backup create` (the small profile; a Postgres
-- install's backups are Postgres PITR and versioned buckets, so this stays
-- empty there, but the schemas match).
CREATE TABLE backup_run (
    id           TEXT PRIMARY KEY,
    started_at   BIGINT NOT NULL,
    finished_at  BIGINT,
    status       TEXT NOT NULL CHECK (status IN ('running', 'done', 'failed', 'ended')),
    dir          TEXT NOT NULL,
    delete_mark  BIGINT,
    objects      BIGINT NOT NULL DEFAULT 0,
    missing      BIGINT NOT NULL DEFAULT 0,
    with_keys    BOOLEAN NOT NULL DEFAULT FALSE,
    started_by   TEXT NOT NULL,
    error        TEXT
);
CREATE INDEX backup_run_started ON backup_run (started_at);
