-- Arc 13 §6.3.2 / §6.4 Step 0.7.1 — key separation completion.
--
-- After Arc 13 the PDS-wide rotation key lives only in
-- `config.authentication.plc_rotation_key` (env-loaded once at
-- startup), not in per-account `plc_keys` rows. The per-actor
-- atproto signing key (Arc 12 Step 1.5's `atproto_signing_key`
-- column) is the only crypto material `plc_keys` carries.
--
-- DESTRUCTIVE per §6.1 clean-slate: any account whose
-- `atproto_signing_key` is still empty (pre-Arc-12-Step-1.5 rows)
-- becomes unusable for service-auth signing after this migration.
-- Arc 13 explicitly opts out of forward-population per §6.5.1
-- ("no production migration story"); operators wipe test
-- accounts before running Arc 13 per §6.4 Step 0.1.
--
-- Cross-arc handoff to Arc 12 v4.1: this also closes the residual
-- gap noted at §5.4 Step 1.5's report (Arc 12 added the new
-- column but couldn't remove the old one yet because downstream
-- read paths still referenced it; Arc 13 owns the removal +
-- read-path updates in the same atomic commit per §6.4 Step 0.7).

ALTER TABLE plc_keys DROP COLUMN rotation_key;
ALTER TABLE plc_keys DROP COLUMN rotation_key_public;
