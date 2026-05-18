-- Postgres variant of migrations/0008_plc_keys_atproto_signing_key.sql.
-- See that file for the Arc 12 Step 1.5 substrate-gap-closure
-- motivation + Arc 13 scope split. chainlink #68.
--
-- Identical statement: the TEXT column type + DEFAULT '' clause
-- is portable across SQLite and Postgres without translation.

ALTER TABLE plc_keys
    ADD COLUMN atproto_signing_key TEXT NOT NULL DEFAULT '';
