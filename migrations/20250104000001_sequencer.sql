-- Sequencer event log
-- Globally ordered event stream for federation and synchronization

CREATE TABLE IF NOT EXISTS repo_seq (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    did TEXT NOT NULL,
    event_type TEXT NOT NULL,  -- 'commit', 'identity', 'account'
    event BLOB NOT NULL,        -- CBOR-encoded event data
    invalidated INTEGER NOT NULL DEFAULT 0,  -- 0 = active, 1 = invalidated
    sequenced_at TEXT NOT NULL  -- ISO 8601 timestamp
);

-- Index for filtering by DID
CREATE INDEX IF NOT EXISTS idx_repo_seq_did ON repo_seq(did);

-- Index for filtering by event type
CREATE INDEX IF NOT EXISTS idx_repo_seq_event_type ON repo_seq(event_type);

-- Index for time-based queries
CREATE INDEX IF NOT EXISTS idx_repo_seq_sequenced_at ON repo_seq(sequenced_at);

-- Index for efficient cursor queries (seq > cursor)
CREATE INDEX IF NOT EXISTS idx_repo_seq_seq_invalidated ON repo_seq(seq, invalidated);
