-- Operator control of the leased jobs: paused (the job stops and isn't
-- taken up), and a run asked for (the holder restarts it now).
CREATE TABLE job_control (
    name           TEXT PRIMARY KEY NOT NULL,
    paused         INTEGER NOT NULL DEFAULT 0,
    paused_by      TEXT,
    run_requested  INTEGER
) STRICT;
