-- Holder UI Phase 2.b (chainlink #427): reshape holder_auth_method passkey
-- columns to webauthn-rs's whole-serialized-`Passkey` model. Postgres
-- counterpart of sqlite 0034. See that file for the full rationale (lossless —
-- no passkey rows were ever written). Postgres drops columns freely.

ALTER TABLE holder_auth_method DROP COLUMN passkey_public_key;
ALTER TABLE holder_auth_method DROP COLUMN passkey_sign_count;
ALTER TABLE holder_auth_method DROP COLUMN passkey_backup_eligible;
ALTER TABLE holder_auth_method DROP COLUMN passkey_backup_state;

ALTER TABLE holder_auth_method ADD COLUMN passkey_data TEXT;
