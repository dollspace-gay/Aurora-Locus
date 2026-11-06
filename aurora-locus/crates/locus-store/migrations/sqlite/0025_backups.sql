-- Backups: one row per `backup create`. While one runs (its `backup` lease
-- live), the delete job deletes nothing until its delete mark is recorded,
-- then only queue rows at or below it.
CREATE TABLE backup_run (
    id           TEXT PRIMARY KEY NOT NULL,
    started_at   INTEGER NOT NULL,
    finished_at  INTEGER,
    status       TEXT NOT NULL CHECK (status IN ('running', 'done', 'failed', 'ended')),
    dir          TEXT NOT NULL,
    delete_mark  INTEGER,
    objects      INTEGER NOT NULL DEFAULT 0,
    missing      INTEGER NOT NULL DEFAULT 0,
    with_keys    INTEGER NOT NULL DEFAULT 0,
    started_by   TEXT NOT NULL,
    error        TEXT
) STRICT;
CREATE INDEX backup_run_started ON backup_run (started_at);
