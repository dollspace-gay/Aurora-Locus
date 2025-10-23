-- Blob metadata table (in account database)
-- Tracks all blobs uploaded by users across the system

CREATE TABLE IF NOT EXISTS blob_metadata (
    cid TEXT PRIMARY KEY,
    mime_type TEXT NOT NULL,
    size INTEGER NOT NULL,
    creator_did TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- Foreign key to account (optional, for cleanup)
    FOREIGN KEY (creator_did) REFERENCES account(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_blob_creator ON blob_metadata(creator_did);
CREATE INDEX IF NOT EXISTS idx_blob_created_at ON blob_metadata(created_at);
