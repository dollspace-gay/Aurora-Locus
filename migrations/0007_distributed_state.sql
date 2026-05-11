-- Arc 7 / V04_DESIGN.md §6.2.1 + §6.4.0.6 — distributed-state substrate.
-- chainlink #53 — Arc 7 multi-instance auth state + rate limiting (v0.4).
--
-- These tables back the DistributedStore trait surface
-- (src/distributed/mod.rs, introduced in Step 1) for two
-- substrate consumers:
--
--   1. dpop_jti_replay — RFC 9449 JTI replay tracking for
--      client-issued DPoP proofs. Cross-instance correctness
--      concern: a JTI accepted by instance A must be rejected
--      by instance B. Single-use semantic via atomic
--      primary-key-conflict-on-insert; cleanup via reaper
--      sweep.
--
--   2. rate_limit_buckets — token-bucket counters for
--      distributed rate limiting. Cross-instance correctness
--      concern: rate limit exhausted on instance A must reject
--      on instance B. Refill arithmetic uses BIGINT epoch-ms
--      delta (cross-backend portable; sqlx::Any does not
--      expose Postgres's EXTRACT() or TIMESTAMPTZ subtraction
--      and SQLite has no equivalent functions).
--
-- OAuth flow state intentionally absent from this migration:
-- Aurora-Locus's authorization_request table (in
-- migrations/0001_initial.sql) is already the source of truth
-- for OAuth state. Step 2 adopts the DistributedStore trait
-- over that existing table via an adapter; no schema migration
-- of OAuth state.
--
-- Schema deliberately stays within sqlx::Any's portable subset:
-- - TEXT primary keys (no auto-increment, no enums).
-- - BIGINT for all numeric columns (no INT/INTEGER, no FLOAT,
--   no NUMERIC).
-- - BIGINT epoch-milliseconds for time-arithmetic columns
--   (deviation from the codebase's usual RFC3339 TEXT-timestamp
--   convention, documented above).
-- - No CHECK constraints, no triggers, no generated columns.
-- - PRIMARY KEY is the only column-level constraint.

CREATE TABLE dpop_jti_replay (
    jti                     TEXT PRIMARY KEY,
    jkt                     TEXT NOT NULL,
    exp_at_epoch_ms         BIGINT NOT NULL,
    created_at_epoch_ms     BIGINT NOT NULL
);

CREATE INDEX idx_dpop_jti_replay_exp
    ON dpop_jti_replay(exp_at_epoch_ms);

CREATE TABLE rate_limit_buckets (
    bucket_key                  TEXT PRIMARY KEY,
    tokens_remaining            BIGINT NOT NULL,
    max_tokens                  BIGINT NOT NULL,
    refill_rate                 BIGINT NOT NULL,
    window_start_at_epoch_ms    BIGINT NOT NULL,
    version                     BIGINT NOT NULL DEFAULT 0
);

CREATE INDEX idx_rate_limit_buckets_window
    ON rate_limit_buckets(window_start_at_epoch_ms);
