-- Install-wide key state: a generation every node watches (bumped when a
-- key nodes hold in memory changes, so they reload), the last key backup,
-- and the PLC key scan's progress.
CREATE TABLE key_state (
    id                 INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    generation         INTEGER NOT NULL DEFAULT 0,
    last_backup_at     INTEGER,
    last_backup_by     TEXT,
    scan_cursor        TEXT,
    scan_started_at    INTEGER,
    scan_completed_at  INTEGER
) STRICT;
INSERT INTO key_state (id) VALUES (1);
