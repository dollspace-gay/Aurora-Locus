-- DID Cache - Stores resolved DID documents and handle mappings
-- Enables efficient handle resolution and cross-server identity lookups

-- DID document cache
CREATE TABLE IF NOT EXISTS did_doc (
    did TEXT PRIMARY KEY,
    doc TEXT NOT NULL,          -- JSON-encoded DID document
    updated_at TEXT NOT NULL,   -- ISO 8601 timestamp
    cached_at TEXT NOT NULL     -- ISO 8601 timestamp
);

-- Handle to DID mapping cache
CREATE TABLE IF NOT EXISTS did_handle (
    handle TEXT PRIMARY KEY,    -- Normalized handle (lowercase)
    did TEXT NOT NULL,
    declared_at TEXT,           -- ISO 8601 timestamp of last declaration
    updated_at TEXT NOT NULL,   -- ISO 8601 timestamp
    FOREIGN KEY (did) REFERENCES did_doc(did) ON DELETE CASCADE
);

-- Index for reverse lookup (DID to handle)
CREATE INDEX IF NOT EXISTS idx_did_handle_did ON did_handle(did);

-- Index for cache invalidation (find stale entries)
CREATE INDEX IF NOT EXISTS idx_did_doc_updated_at ON did_doc(updated_at);
CREATE INDEX IF NOT EXISTS idx_did_handle_updated_at ON did_handle(updated_at);
