-- Temporary blob metadata table (in account database)
-- Tracks blobs in temporary storage during two-phase upload

CREATE TABLE IF NOT EXISTS temp_blob_metadata (
    cid TEXT PRIMARY KEY,
    mime_type TEXT NOT NULL,
    size INTEGER NOT NULL,
    creator_did TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    width INTEGER,
    height INTEGER,

    -- Foreign key to account
    FOREIGN KEY (creator_did) REFERENCES account(did) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_temp_blob_creator ON temp_blob_metadata(creator_did);
CREATE INDEX IF NOT EXISTS idx_temp_blob_created_at ON temp_blob_metadata(created_at);
