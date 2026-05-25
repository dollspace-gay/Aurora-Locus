-- Postgres variant of migrations/0009_drop_plc_keys_rotation_key.sql.
-- See that file for the Arc 13 Step 0.7.1 motivation + Arc 12
-- cross-arc handoff. chainlink #70.
--
-- Identical statements: both SQLite (>= 3.35) and PostgreSQL
-- support ALTER TABLE ... DROP COLUMN.

ALTER TABLE plc_keys DROP COLUMN rotation_key;
ALTER TABLE plc_keys DROP COLUMN rotation_key_public;
