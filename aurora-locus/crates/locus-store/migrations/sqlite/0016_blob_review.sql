-- Blobs a scanner matched, for operators to review (matches can be false
-- positives). The bytes are kept (`review_hold`) until resolved.
CREATE TABLE blob_review (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    did          TEXT NOT NULL,
    cid          TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    source       TEXT NOT NULL,
    result       TEXT NOT NULL,
    reason       TEXT,
    status       TEXT NOT NULL DEFAULT 'open',
    resolved_at  INTEGER,
    resolved_by  TEXT,
    note         TEXT
) STRICT;
CREATE INDEX blob_review_status ON blob_review (status, created_at);
CREATE INDEX blob_scan_status ON blob (scan_status) WHERE scan_status = 'unscanned';
