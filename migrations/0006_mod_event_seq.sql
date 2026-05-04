-- Session 9 / chainlink #115 — retention-bounded subscription channel
-- per docs/AURORA_ADMIN_UI_DESIGN.md §3.5.
--
-- The `moderation_event` table grows unbounded as the historical
-- aggregate of administrative decisions. The live subscription channel
-- (`subscribeModEvents`) only needs a recent window — the design
-- commits to a separate retention-bounded table so storage on the
-- streaming surface is predictable and old detail rows can be pruned
-- without touching the historical record.
--
-- Dual-write: every successful `moderation_event` INSERT also writes
-- a `mod_event_seq` row inside the same transaction. The cleanup job
-- (chainlink #115 commit 2) deletes `mod_event_seq` rows older than
-- the retention window; `moderation_event` retains forever.
--
-- The `seq` column is autoincrementing and independent from
-- `moderation_event.id`. Two reasons:
--   1. Subscription cursors track this column; clients track `seq`,
--      not `moderation_event_id`.
--   2. Cleanup deletes rows from this table while `moderation_event`
--      retains forever. Sharing an ID space would leave gaps in the
--      historical sequence which would confuse history queries.
--
-- Columns mirror the subset of `moderation_event` that the
-- `Event` wire variant in `subscribeModEvents` actually emits.
-- The `meta` column is NOT mirrored — the wire format doesn't carry
-- it. Detail-rich queries continue to use `moderation_event` directly
-- via `tools.aurora.moderator.queryEvents` and the historical reads.

CREATE TABLE mod_event_seq (
    seq                  INTEGER PRIMARY KEY AUTOINCREMENT,
    moderation_event_id  INTEGER NOT NULL,
    actor_did            TEXT NOT NULL,
    action               TEXT NOT NULL,
    subject_did          TEXT,
    subject_uri          TEXT,
    subject_cid          TEXT,
    detail               TEXT,
    created_at           TEXT NOT NULL
);

-- Cleanup job deletes by created_at < (now - retention).
CREATE INDEX idx_mod_event_seq_created_at ON mod_event_seq(created_at);

-- Subscription handler: SELECT ... WHERE seq > ? plus optional
-- subject_did / subject_uri filters.
CREATE INDEX idx_mod_event_seq_seq ON mod_event_seq(seq);
CREATE INDEX idx_mod_event_seq_subject_did ON mod_event_seq(subject_did);
CREATE INDEX idx_mod_event_seq_subject_uri ON mod_event_seq(subject_uri);
