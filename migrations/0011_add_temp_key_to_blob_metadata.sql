-- Arc 16b §9.2.3.1 / Step 1.1 (chainlink #91, recon V05_ARC16B_RECON_R0.md
-- Step 0.1): add `temp_key` column + CHECK + partial index to
-- `blob_metadata`. Closes federation sub-feature #46 (jointly with
-- Arc 16c + Arc 16d).
--
-- temp_key sentinel discipline per §9.2.3.1:
--   NOT NULL (literal '1') = Untethered: row exists, no record refs.
--   NULL                   = Permanent: at least one record references.
-- TTL anchor for Arc 16d's sweep is `created_at`; refreshed on every
-- re-entry into untethered state per `track_untethered_blob` (§9.2.3.2).
--
-- Partial index services Arc 16d's anticipated TTL sweep query.
-- Leading column `created_at` matches that workload; forward-coupling
-- caveat noted in §9.2.5.6 #2.

ALTER TABLE blob_metadata ADD COLUMN temp_key TEXT NULL
    CHECK (temp_key IS NULL OR temp_key = '1');

CREATE INDEX idx_blob_metadata_untethered
    ON blob_metadata (created_at)
    WHERE temp_key IS NOT NULL;
