-- Rate limit counters shared by every node (fixed windows).
CREATE TABLE rate_limit (
    key         TEXT PRIMARY KEY NOT NULL,
    consumed    INTEGER NOT NULL,
    window_end  INTEGER NOT NULL
) STRICT;
CREATE INDEX rate_limit_end ON rate_limit (window_end);
