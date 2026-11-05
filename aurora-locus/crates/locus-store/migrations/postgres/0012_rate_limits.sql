-- Rate limit counters shared by every node (fixed windows).
CREATE TABLE rate_limit (
    key         TEXT PRIMARY KEY,
    consumed    BIGINT NOT NULL,
    window_end  BIGINT NOT NULL
);
CREATE INDEX rate_limit_end ON rate_limit (window_end);
