-- KMS: a PLC rotation key can live in KMS (its private half is never
-- here), and the master key can be a KMS data key, kept wrapped by KMS so
-- every node can unwrap it.
ALTER TABLE plc_rotation_key ADD COLUMN kms_key_id TEXT;

CREATE TABLE master_key_wrap (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    kms_key_id  TEXT NOT NULL,
    wrapped     BYTEA NOT NULL,
    created_at  BIGINT NOT NULL
);
