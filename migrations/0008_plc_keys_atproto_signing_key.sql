-- Arc 12 Step 1.5 — substrate-gap closure for `entryway_auth_headers`
-- (V05_DESIGN.md §5.4 Step 2.1). Adds the per-actor atproto signing
-- key column read by Arc 12's service-auth-JWT mint surface.
--
-- Scope-split with Arc 13 v4.1 §6.3.2 (preserved):
--   - Arc 12 (this migration): add the column; new accounts
--     populate it at create-time with a fresh ES256K key distinct
--     from `rotation_key`.
--   - Arc 13: forward-population of legacy `plc_keys` rows whose
--     `atproto_signing_key` defaults to empty; the semantic-
--     separation rename + codebase split (PLC ops use rotation
--     key, service-auth signing uses atproto signing key).
--
-- Storage format: hex-encoded 32-byte k256 private key, matching
-- the existing `rotation_key` column's encoding. Default `''`
-- empty-string for legacy rows; `entryway_auth_headers` surfaces
-- `KeyNotFound` per §5.4 Step 2.1 failure modes when the column
-- is empty, so legacy rows remain readable by every other
-- account-path query without breakage.

ALTER TABLE plc_keys
    ADD COLUMN atproto_signing_key TEXT NOT NULL DEFAULT '';
