-- Operator control of the leased jobs: paused (the job stops and isn't
-- taken up), and a run asked for (the holder restarts it now).
CREATE TABLE job_control (
    name           TEXT PRIMARY KEY,
    paused         BOOLEAN NOT NULL DEFAULT FALSE,
    paused_by      TEXT,
    run_requested  BIGINT
);
