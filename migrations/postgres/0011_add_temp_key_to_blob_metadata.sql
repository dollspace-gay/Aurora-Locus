-- Arc 16b §9.2.3.1 / Step 1.1 — Postgres variant of
-- migrations/0011_add_temp_key_to_blob_metadata.sql.

ALTER TABLE blob_metadata ADD COLUMN temp_key TEXT NULL
    CHECK (temp_key IS NULL OR temp_key = '1');

CREATE INDEX idx_blob_metadata_untethered
    ON blob_metadata (created_at)
    WHERE temp_key IS NOT NULL;
