-- v0.9 close cohort (#406) — rename the bundled theme `stack-classic` to
-- `aurora-classic`. Operators who set the deployment-default theme to the old
-- id carry a stale runtime-settings row; migrate it forward so the
-- `theme.deployment-default` setting resolves to a real theme after the rename.
--
-- The value is stored JSON-encoded (e.g. '"stack-classic"'); the reader also
-- tolerates a raw un-quoted form, so migrate both encodings. Personal per-
-- operator preferences live in browser localStorage and are migrated client-
-- side by the settings.js LEGACY_THEME map. No-op on deployments that never
-- set the runtime row (the compiled default already resolves to aurora-classic).
UPDATE runtime_settings
   SET value = '"aurora-classic"'
 WHERE key = 'theme.deployment-default' AND value = '"stack-classic"';

UPDATE runtime_settings
   SET value = 'aurora-classic'
 WHERE key = 'theme.deployment-default' AND value = 'stack-classic';
