-- Arc 14 §7.3.6 / §7.4 Step 6.1: add suspended_at + desynchronized_at
-- columns to actor; TRUNCATE repo_seq in the same migration
-- commit (Sub-step 0.A: Arc 14's own bookkeeping per round-1 F2 closure).
--
-- These columns are populated:
--   * suspended_at      — admin moderation action (currently
--                         test-affordance direct DB writes only;
--                         production setter is v0.6+).
--   * desynchronized_at — desync detection (currently
--                         test-affordance direct DB writes only;
--                         production setter is v0.6+).
--
-- Per V05_DESIGN.md §3.3 clean-slate destructive migration policy:
-- repo_seq is wiped (firehose is consumer-recoverable from current
-- repo state; no need to preserve a backfill window across schema
-- changes).

ALTER TABLE actor ADD COLUMN suspended_at TEXT;
ALTER TABLE actor ADD COLUMN desynchronized_at TEXT;

DELETE FROM repo_seq;
