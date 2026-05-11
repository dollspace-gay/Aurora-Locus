-- Postgres variant of migrations/0007_distributed_state.sql.
-- See that file for the Arc 7 / V04_DESIGN.md §6.2.1 + §6.4.0.6
-- motivation.
--
-- Schema is identical to the SQLite variant — the column types
-- chosen (TEXT, BIGINT) are within sqlx::Any's portable subset
-- and require no Postgres-side type translation. No BIGSERIAL,
-- BOOLEAN, or TIMESTAMPTZ here because the substrate's hot-path
-- arithmetic depends on integer epoch-milliseconds being directly
-- subtractable on both backends.

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
