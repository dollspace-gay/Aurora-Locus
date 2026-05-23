-- Arc 17 §17.4 Step 1.3 — on-disk lexicon cache. SQLite variant of
-- migrations/postgres/0013_arc17_lexicon_cache.sql. Schema pinned by #132.
--
-- All timestamp columns are TEXT (ISO-8601 UTC). #130 invariant: sqlx::Any
-- deliberately excludes chrono::DateTime<Utc> from its type-compat set;
-- TIMESTAMPTZ would silently break reads on PG. Every timestamp column in
-- the schema follows this rule.
--
-- Conflict handling (§17.3.2 round-1 F6 closure): SQLite writers use
-- `INSERT OR REPLACE` against this table; the PG variant uses
-- `INSERT ... ON CONFLICT (nsid) DO UPDATE`.

CREATE TABLE lexicon_cache (
    nsid           TEXT PRIMARY KEY,
    authority_did  TEXT NOT NULL,
    lexicon_json   TEXT NOT NULL,
    fetched_at     TEXT NOT NULL,
    last_used_at   TEXT NOT NULL,
    expires_at     TEXT NOT NULL
);

CREATE INDEX idx_lexicon_cache_last_used_at ON lexicon_cache (last_used_at);
