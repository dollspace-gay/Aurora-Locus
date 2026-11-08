-- Repository imports: per-job staging tables, filled while the import is
-- checked and read back by its later phases, so each phase can resume from
-- them after a crash.

-- Every block the import changes the count of: delta is (references in the
-- new tree) - (references in the old tree). content is null for blocks
-- only the old tree has.
CREATE TABLE import_block (
    job_id   TEXT NOT NULL,
    cid      TEXT NOT NULL,
    content  BLOB,
    delta    INTEGER NOT NULL DEFAULT 0,
    applied  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (job_id, cid)
) STRICT;

-- The record diff: one row per key that changes.
CREATE TABLE import_record (
    job_id      TEXT NOT NULL,
    collection  TEXT NOT NULL,
    rkey        TEXT NOT NULL,
    change      TEXT NOT NULL CHECK (change IN ('create', 'update', 'delete')),
    new_cid     TEXT,
    old_cid     TEXT,
    applied     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (job_id, collection, rkey)
) STRICT;

-- Blob CIDs the new tree references, and those the old tree's changed or
-- deleted records referenced.
CREATE TABLE import_blob (
    job_id        TEXT NOT NULL,
    cid           TEXT NOT NULL,
    in_new_tree   INTEGER NOT NULL,
    private_only  INTEGER NOT NULL DEFAULT 0,
    applied       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (job_id, cid)
) STRICT;
CREATE INDEX import_blob_cid ON import_blob (cid);

-- Progress and notes for the job's status.
ALTER TABLE bulk_job ADD COLUMN progress_done INTEGER NOT NULL DEFAULT 0;
ALTER TABLE bulk_job ADD COLUMN progress_total INTEGER NOT NULL DEFAULT 0;
ALTER TABLE bulk_job ADD COLUMN notes TEXT;
