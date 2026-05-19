-- Arc 14 §7.3.6 / §7.4 Step 6.1: add suspended_at + desynchronized_at
-- columns to actor; TRUNCATE repo_seq in the same migration
-- commit (Sub-step 0.A: Arc 14's own bookkeeping per round-1 F2 closure).
--
-- Postgres variant of migrations/0010_arc14_suspended_desynchronized.sql.

ALTER TABLE actor ADD COLUMN suspended_at TIMESTAMPTZ;
ALTER TABLE actor ADD COLUMN desynchronized_at TIMESTAMPTZ;

TRUNCATE TABLE repo_seq RESTART IDENTITY CASCADE;
