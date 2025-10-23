-- Actor Store Schema (Per-User Database)
-- Each user gets their own SQLite database with this schema
-- Based on TypeScript PDS actor-store/db/migrations/001-init.ts

-- Repository root metadata (single row per database)
CREATE TABLE IF NOT EXISTS repo_root (
    did TEXT PRIMARY KEY NOT NULL,
    cid TEXT NOT NULL,              -- Current root CID of the MST
    rev TEXT NOT NULL,              -- Repository revision (TID)
    indexed_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- MST blocks (content-addressed storage)
CREATE TABLE IF NOT EXISTS repo_block (
    cid TEXT PRIMARY KEY NOT NULL,
    repo_rev TEXT NOT NULL,         -- Which revision this block belongs to
    size INTEGER NOT NULL,          -- Size in bytes
    content BLOB NOT NULL           -- CBOR-encoded block data
);

CREATE INDEX IF NOT EXISTS idx_repo_block_rev ON repo_block(repo_rev);
CREATE INDEX IF NOT EXISTS idx_repo_block_rev_cid ON repo_block(repo_rev, cid);

-- Records index (fast lookups and metadata)
CREATE TABLE IF NOT EXISTS record (
    uri TEXT PRIMARY KEY NOT NULL,  -- at://did/collection/rkey
    cid TEXT NOT NULL,              -- CID of this record
    collection TEXT NOT NULL,       -- Collection name
    rkey TEXT NOT NULL,             -- Record key
    repo_rev TEXT NOT NULL,         -- Repository revision
    indexed_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    takedown_ref TEXT               -- Moderation takedown reference
);

CREATE INDEX IF NOT EXISTS idx_record_cid ON record(cid);
CREATE INDEX IF NOT EXISTS idx_record_collection ON record(collection);
CREATE INDEX IF NOT EXISTS idx_record_repo_rev ON record(repo_rev);
CREATE INDEX IF NOT EXISTS idx_record_collection_rkey ON record(collection, rkey);

-- Blob metadata
CREATE TABLE IF NOT EXISTS blob (
    cid TEXT PRIMARY KEY NOT NULL,
    mime_type TEXT NOT NULL,
    size INTEGER NOT NULL,
    temp_key TEXT,                  -- Temporary key in blobstore (null when permanent)
    width INTEGER,                  -- Image width (if applicable)
    height INTEGER,                 -- Image height (if applicable)
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    takedown_ref TEXT               -- Moderation takedown reference
);

CREATE INDEX IF NOT EXISTS idx_blob_temp_key ON blob(temp_key) WHERE temp_key IS NOT NULL;

-- Record-blob associations (many-to-many)
CREATE TABLE IF NOT EXISTS record_blob (
    blob_cid TEXT NOT NULL,
    record_uri TEXT NOT NULL,
    PRIMARY KEY (blob_cid, record_uri),
    FOREIGN KEY (blob_cid) REFERENCES blob(cid) ON DELETE CASCADE,
    FOREIGN KEY (record_uri) REFERENCES record(uri) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_record_blob_uri ON record_blob(record_uri);

-- Backlinks (record link tracking)
CREATE TABLE IF NOT EXISTS backlink (
    uri TEXT NOT NULL,              -- URI of the record making the link
    path TEXT NOT NULL,             -- JSON path to the link
    link_to TEXT NOT NULL,          -- What the record links to (DID or AtUri)
    PRIMARY KEY (uri, path),
    FOREIGN KEY (uri) REFERENCES record(uri) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_backlink_path_link ON backlink(path, link_to);

-- User preferences
CREATE TABLE IF NOT EXISTS account_pref (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,             -- Preference name (namespace)
    value_json TEXT NOT NULL        -- JSON-serialized preference value
);

CREATE INDEX IF NOT EXISTS idx_account_pref_name ON account_pref(name);
