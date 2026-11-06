-- Hostname migrations: prepared (the candidate reserved, nothing else
-- changed), running (the new hostname current; the job moves every
-- account's PDS endpoint), done, or cancelled. Old hosts stay reserved.
CREATE TABLE hostname_migration (
    id              TEXT PRIMARY KEY,
    old_host        TEXT NOT NULL,
    new_host        TEXT NOT NULL,
    state           TEXT NOT NULL CHECK (state IN ('prepared', 'running', 'done', 'cancelled')),
    created_at      BIGINT NOT NULL,
    created_by      TEXT NOT NULL,
    prechecks       TEXT,
    confirmed_at    BIGINT,
    cursor          TEXT,
    job_done_at     BIGINT,
    window_ends_at  BIGINT,
    rechecked_at    BIGINT,
    closed_at       BIGINT,
    former_at       BIGINT
);
CREATE INDEX hostname_migration_hosts ON hostname_migration (old_host, new_host);

-- What the job did per account. `cannot`: locus can't move it (no held
-- rotation key, or the user hosts its did:web document); `failed`: it
-- tried and will recheck.
CREATE TABLE hostname_move (
    migration_id  TEXT NOT NULL,
    did           TEXT NOT NULL,
    state         TEXT NOT NULL CHECK (state IN ('moved', 'cannot', 'failed')),
    reason        TEXT,
    checked_at    BIGINT NOT NULL,
    PRIMARY KEY (migration_id, did)
);
CREATE INDEX hostname_move_left ON hostname_move (migration_id, state) WHERE state <> 'moved';
