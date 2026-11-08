-- Blobs: the active storage backend, blob rows, the delete queue, and the
-- record → blob and record → subject indexes.

-- The one active backend and its epoch (bumped when blobs move to another
-- backend). Every transaction that names a storage key reads it first.
CREATE TABLE blob_backend (
    id               INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    active           TEXT NOT NULL DEFAULT 'disk',
    epoch            INTEGER NOT NULL DEFAULT 1,
    paused           INTEGER NOT NULL DEFAULT 0,
    migration_job    TEXT,
    disk_root        TEXT,
    import_prefixes  TEXT NOT NULL DEFAULT '[]'
) STRICT;
INSERT INTO blob_backend (id) VALUES (1);

-- SQLite's stand-in for a sequence: writers are serialized.
CREATE TABLE key_seq_counter (
    id     INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    value  INTEGER NOT NULL
) STRICT;
INSERT INTO key_seq_counter (id, value) VALUES (1, 0);

-- A row per (account, blob). storage_key is unique per copy (a fresh
-- generation for every write or move); temp blobs live under tmp/.
CREATE TABLE blob (
    did                 TEXT NOT NULL,
    cid                 TEXT NOT NULL,
    mime_type           TEXT NOT NULL,
    size                INTEGER NOT NULL,
    storage_key         TEXT NOT NULL,
    state               TEXT NOT NULL CHECK (state IN ('temp', 'permanent', 'quarantined')),
    created_at          INTEGER NOT NULL,
    takedown_ref        TEXT,
    store               TEXT NOT NULL DEFAULT 'normal' CHECK (store IN ('normal', 'gated')),
    gated               INTEGER NOT NULL DEFAULT 0,
    pending_move        TEXT,
    move_epoch          INTEGER,
    unreferenced_since  INTEGER,
    review_hold         INTEGER NOT NULL DEFAULT 0,
    scan_status         TEXT NOT NULL DEFAULT 'none',
    key_seq             INTEGER NOT NULL,
    PRIMARY KEY (did, cid)
) STRICT;
CREATE INDEX blob_key_seq ON blob (key_seq);
CREATE INDEX blob_temp ON blob (created_at) WHERE state = 'temp';
CREATE INDEX blob_unreferenced ON blob (unreferenced_since) WHERE unreferenced_since IS NOT NULL;
CREATE INDEX blob_pending_move ON blob (did) WHERE pending_move IS NOT NULL;
CREATE INDEX blob_storage_key ON blob (storage_key);

-- Bytes to delete, queued by the transaction that orphans them.
CREATE TABLE blob_delete (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    storage_key    TEXT NOT NULL,
    backend_epoch  INTEGER NOT NULL,
    enqueued_at    INTEGER NOT NULL,
    attempts       INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX blob_delete_enqueued ON blob_delete (enqueued_at);

CREATE TABLE record_blob (
    did         TEXT NOT NULL,
    blob_cid    TEXT NOT NULL,
    collection  TEXT NOT NULL,
    rkey        TEXT NOT NULL,
    PRIMARY KEY (did, blob_cid, collection, rkey)
) STRICT;
CREATE INDEX record_blob_record ON record_blob (did, collection, rkey);

-- What each record points at (`subject`, `subject.uri`), for the
-- reference's duplicate follow/like/repost/block cleanup.
CREATE TABLE backlink (
    did         TEXT NOT NULL,
    collection  TEXT NOT NULL,
    rkey        TEXT NOT NULL,
    path        TEXT NOT NULL,
    link_to     TEXT NOT NULL,
    PRIMARY KEY (did, collection, rkey, path)
) STRICT;
CREATE INDEX backlink_target ON backlink (did, collection, path, link_to);
