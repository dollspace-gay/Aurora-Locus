-- Blobs: the active storage backend, blob rows, the delete queue, and the
-- record → blob and record → subject indexes.

-- The one active backend and its epoch (bumped when blobs move to another
-- backend). Every transaction that names a storage key reads it first
-- (FOR SHARE).
CREATE TABLE blob_backend (
    id               INTEGER PRIMARY KEY CHECK (id = 1),
    active           TEXT NOT NULL DEFAULT 'disk',
    epoch            BIGINT NOT NULL DEFAULT 1,
    paused           BOOLEAN NOT NULL DEFAULT FALSE,
    migration_job    TEXT,
    disk_root        TEXT,
    import_prefixes  TEXT NOT NULL DEFAULT '[]'
);
INSERT INTO blob_backend (id) VALUES (1);

-- CACHE 1, so last_value is the highest value any session has drawn.
CREATE SEQUENCE blob_key_sequence CACHE 1;

-- A row per (account, blob). storage_key is unique per copy (a fresh
-- generation for every write or move); temp blobs live under tmp/.
CREATE TABLE blob (
    did                 TEXT NOT NULL,
    cid                 TEXT NOT NULL,
    mime_type           TEXT NOT NULL,
    size                BIGINT NOT NULL,
    storage_key         TEXT NOT NULL,
    state               TEXT NOT NULL CHECK (state IN ('temp', 'permanent', 'quarantined')),
    created_at          BIGINT NOT NULL,
    takedown_ref        TEXT,
    store               TEXT NOT NULL DEFAULT 'normal' CHECK (store IN ('normal', 'gated')),
    gated               BOOLEAN NOT NULL DEFAULT FALSE,
    pending_move        TEXT,
    move_epoch          BIGINT,
    unreferenced_since  BIGINT,
    review_hold         BOOLEAN NOT NULL DEFAULT FALSE,
    scan_status         TEXT NOT NULL DEFAULT 'none',
    key_seq             BIGINT NOT NULL,
    PRIMARY KEY (did, cid)
);
CREATE INDEX blob_key_seq ON blob (key_seq);
CREATE INDEX blob_temp ON blob (created_at) WHERE state = 'temp';
CREATE INDEX blob_unreferenced ON blob (unreferenced_since) WHERE unreferenced_since IS NOT NULL;
CREATE INDEX blob_pending_move ON blob (did) WHERE pending_move IS NOT NULL;
CREATE INDEX blob_storage_key ON blob (storage_key);

-- Bytes to delete, queued by the transaction that orphans them.
CREATE TABLE blob_delete (
    id             BIGSERIAL PRIMARY KEY,
    storage_key    TEXT NOT NULL,
    backend_epoch  BIGINT NOT NULL,
    enqueued_at    BIGINT NOT NULL,
    attempts       INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX blob_delete_enqueued ON blob_delete (enqueued_at);

CREATE TABLE record_blob (
    did         TEXT NOT NULL,
    blob_cid    TEXT NOT NULL,
    collection  TEXT NOT NULL,
    rkey        TEXT NOT NULL,
    PRIMARY KEY (did, blob_cid, collection, rkey)
);
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
);
CREATE INDEX backlink_target ON backlink (did, collection, path, link_to);
