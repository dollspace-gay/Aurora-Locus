-- v0.9 Federation runtime-mutability arc §3.6 (#394) — per-run, per-account
-- result tracking for the post-restart bulk did:plc DID-doc update (§2.3, run by
-- Phase E2; surfaced by E4). D2's save handler writes the initial `pending` rows
-- in the same outer tx as the bulk-diddoc-update marker; E2 updates each to a
-- terminal status.
--
-- `started_at` is the recency key for "most recent run" queries
-- (ORDER BY started_at DESC LIMIT 1) — `run_id` is a UUID and NOT chronologically
-- ordered, so MAX(run_id) would surface the wrong run (R3 H-2).
-- `skipped_did_web` is forward-compatibility for v0.10 did:web accounts; never
-- produced in v0.9 (all accounts are did:plc per #381 Outcome A).
CREATE TABLE bulk_diddoc_update_result (
    did        TEXT NOT NULL,
    run_id     TEXT NOT NULL,
    started_at TEXT NOT NULL,
    status     TEXT NOT NULL CHECK(status IN (
                   'pending', 'aligned', 'failed', 'unresolvable', 'skipped_did_web'
               )),
    reason     TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (did, run_id)
);

CREATE INDEX idx_bulk_diddoc_update_result_run_status
    ON bulk_diddoc_update_result (run_id, status);

CREATE INDEX idx_bulk_diddoc_update_result_started_at
    ON bulk_diddoc_update_result (started_at DESC);
