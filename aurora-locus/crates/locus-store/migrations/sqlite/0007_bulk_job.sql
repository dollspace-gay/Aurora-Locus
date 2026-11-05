-- Bulk work on one account (deletion; later imports and rebuilds) as a
-- persisted, resumable job. The job holds itself like a lease (holder,
-- epoch, expires_at); every batch checks it in its own transaction, and
-- a resumer takes over jobs whose lease ran out.
CREATE TABLE bulk_job (
    id          TEXT PRIMARY KEY NOT NULL,
    did         TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('import', 'delete', 'rebuild')),
    phase       TEXT NOT NULL,
    cursor      TEXT,
    holder      TEXT NOT NULL,
    epoch       INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    new_rev     TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    error       TEXT
) STRICT;
CREATE INDEX bulk_job_open ON bulk_job (expires_at) WHERE phase NOT IN ('done', 'failed');
CREATE INDEX bulk_job_did ON bulk_job (did);
