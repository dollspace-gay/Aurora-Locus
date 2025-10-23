-- Add PLC rotation key storage to account table
-- Migration for PLC DID integration

-- Add columns for PLC key management
ALTER TABLE account ADD COLUMN plc_rotation_key TEXT;
ALTER TABLE account ADD COLUMN plc_rotation_key_public TEXT;
ALTER TABLE account ADD COLUMN plc_last_operation_cid TEXT;

-- Create index for efficient PLC key lookups
CREATE INDEX IF NOT EXISTS idx_account_plc_rotation_key ON account(plc_rotation_key_public);

-- Comments:
-- plc_rotation_key: Hex-encoded private key (32 bytes) for signing PLC operations
-- plc_rotation_key_public: Hex-encoded compressed public key (33 bytes) for verification
-- plc_last_operation_cid: CID of the last PLC operation submitted (for tracking history)
