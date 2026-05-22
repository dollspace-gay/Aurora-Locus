-- #130 — restore the all-timestamp-columns-are-TEXT invariant on PG.
--
-- Migration 0010 declared actor.suspended_at and actor.desynchronized_at
-- as TIMESTAMPTZ; the matching SQLite migration declared them TEXT. Every
-- other timestamp column in the schema (created_at, takedown_ref,
-- deactivated_at, delete_after, email_confirmed_at, ...) is TEXT on both
-- backends because the read path goes through sqlx::Any, whose type-compat
-- set deliberately excludes chrono::DateTime<Utc>. The Rust read path at
-- src/account/manager.rs:603-604 reads both columns as Option<String>; on
-- PG that decode fails against TIMESTAMPTZ, breaking createSession (and
-- every other actor-row read) for any PG-backed instance.
--
-- This forward migration realigns PG to the established invariant by
-- ALTERing both columns TYPE TEXT with an explicit ::text USING clause.
-- The cast produces PG's default RFC3339-ish ISO 8601 rendering for any
-- populated values (these columns are admin-moderation / desync-detection
-- writes, currently test-affordance only per 0010's comment, so populated
-- values are rare on real deploys). NULL rows pass through as NULL::text.

ALTER TABLE actor
    ALTER COLUMN suspended_at TYPE TEXT USING suspended_at::text,
    ALTER COLUMN desynchronized_at TYPE TEXT USING desynchronized_at::text;
