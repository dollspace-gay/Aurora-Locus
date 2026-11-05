-- Blob placement: finding rows whose bytes belong in another store (into
-- or out of gated/ and quarantine/) without scanning every blob.
CREATE INDEX blob_gated_flag ON blob (did, cid) WHERE gated;
CREATE INDEX blob_in_gated ON blob (did, cid) WHERE storage_key LIKE 'gated/%';
CREATE INDEX blob_held ON blob (did, cid) WHERE takedown_ref IS NOT NULL OR state = 'quarantined';
CREATE INDEX blob_in_quarantine ON blob (did, cid) WHERE storage_key LIKE 'quarantine/%';

-- Branding uploads draw a key sequence value like blob rows, so backups
-- can tell which objects are newer than a snapshot.
ALTER TABLE branding_asset ADD COLUMN key_seq BIGINT NOT NULL DEFAULT 0;
UPDATE branding_asset SET key_seq = (SELECT last_value FROM blob_key_sequence);
CREATE INDEX branding_asset_key_seq ON branding_asset (key_seq);
CREATE INDEX branding_asset_storage_key ON branding_asset (storage_key);
