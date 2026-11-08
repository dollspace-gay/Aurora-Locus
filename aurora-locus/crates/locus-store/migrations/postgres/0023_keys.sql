-- Install-wide key state: a generation every node watches (bumped when a
-- key nodes hold in memory changes, so they reload), the last key backup,
-- and the PLC key scan's progress.
CREATE TABLE key_state (
    id                 INTEGER PRIMARY KEY CHECK (id = 1),
    generation         BIGINT NOT NULL DEFAULT 0,
    last_backup_at     BIGINT,
    last_backup_by     TEXT,
    scan_cursor        TEXT,
    scan_started_at    BIGINT,
    scan_completed_at  BIGINT
);
INSERT INTO key_state (id) VALUES (1);
