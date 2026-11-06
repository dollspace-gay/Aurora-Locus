-- `migrate blobs`: one job at a time (named in blob_backend.migration_job),
-- the marks its passes have copied and verified through, and what each
-- key's copy came to.
CREATE TABLE blob_migration (
    id                TEXT PRIMARY KEY,
    target            TEXT NOT NULL CHECK (target IN ('disk', 's3')),
    disk_root         TEXT,
    phase             TEXT NOT NULL CHECK (phase IN ('copying', 'post_flip', 'done', 'cancelled')),
    copied_through    BIGINT NOT NULL DEFAULT 0,
    verified_through  BIGINT NOT NULL DEFAULT 0,
    passes            BIGINT NOT NULL DEFAULT 0,
    started_at        BIGINT NOT NULL,
    started_by        TEXT NOT NULL,
    finished_at       BIGINT,
    note              TEXT
);

CREATE TABLE blob_copy (
    storage_key   TEXT PRIMARY KEY,
    outcome       TEXT NOT NULL CHECK (outcome IN ('copied', 'missing_source')),
    acknowledged  BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE INDEX blob_copy_missing ON blob_copy (storage_key) WHERE outcome = 'missing_source';
