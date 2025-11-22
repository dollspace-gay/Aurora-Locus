-- DID Cache Tables Migration
-- Creates tables for caching DID documents and handle mappings
--
-- Migration created: 2025-11-22

-- ====================================================================
-- DID Document Cache
-- ====================================================================
CREATE TABLE IF NOT EXISTS did_doc (
    did TEXT PRIMARY KEY,
    doc TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    cached_at TEXT NOT NULL
);

-- ====================================================================
-- Handle to DID Mapping Cache
-- ====================================================================
CREATE TABLE IF NOT EXISTS did_handle (
    handle TEXT PRIMARY KEY,
    did TEXT NOT NULL,
    declared_at TEXT,
    updated_at TEXT NOT NULL
);

-- ====================================================================
-- Migration Complete
-- ====================================================================
