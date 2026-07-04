-- Holder UI Phase 2.b (chainlink #427): reshape the holder_auth_method passkey
-- columns to match webauthn-rs's model.
--
-- webauthn-rs's `Passkey` type is serde-serializable and is meant to be stored
-- and reloaded whole ("can be safely serialised and deserialised from a database
-- for persistence") — the decomposed columns added in migration 0033 (public
-- key / sign-count / backup flags as separate fields) do not reconstruct a
-- `Passkey`. So Phase 2.b stores the serialized `Passkey` in one column and
-- keeps only what needs its own column:
--
--   passkey_data           the serde_json-serialized `Passkey` (source of truth
--                          for finish_passkey_authentication). NULL for the
--                          password / login_alpha method types.
--   passkey_credential_id  KEPT — extracted from `Passkey::cred_id()` at insert
--                          time for the existing partial-unique index + fast
--                          lookup during authentication.
--   passkey_device_name    KEPT — holder-friendly label.
--
-- The four decomposed columns are dropped. This is lossless: passkey was
-- deferred through Phase 1 and 2.a, no `passkey` rows were ever written (grep:
-- no code reads these columns at HEAD), so the columns are uniformly NULL.
--
-- `ALTER TABLE ... DROP COLUMN` is precedented in this codebase (migration
-- 0009). SQLite supports DROP COLUMN since 3.35 for columns not referenced by an
-- index/constraint — the four dropped here are unindexed (only
-- passkey_credential_id is indexed, and it is kept).

ALTER TABLE holder_auth_method DROP COLUMN passkey_public_key;
ALTER TABLE holder_auth_method DROP COLUMN passkey_sign_count;
ALTER TABLE holder_auth_method DROP COLUMN passkey_backup_eligible;
ALTER TABLE holder_auth_method DROP COLUMN passkey_backup_state;

ALTER TABLE holder_auth_method ADD COLUMN passkey_data TEXT;
