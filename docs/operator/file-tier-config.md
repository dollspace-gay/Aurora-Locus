# File-tier runtime configuration

Aurora-Locus resolves runtime settings through a four-tier
hierarchy. From highest to lowest precedence:

1. **Recovery-mode env-var override** — `AURORA_RECOVERY_MODE=true`
   forces `moderation-mode` to `"full"` at the read boundary,
   bypassing tiers 2-4. Recovery-only; do not leave set in
   production.
2. **Runtime row** in the `runtime_settings` table. Set via
   `tools.aurora.admin.setRuntimeSetting`; ephemeral; per-key;
   audit-chained.
3. **File-tier YAML** at `<data_directory>/runtime.yaml` (the
   subject of this document). Loaded once at startup; cached
   for the process lifetime; deployment-stable.
4. **Compiled-in default** from `default_for_key`. Cycle-stable
   fallback shipped with the binary.

The file tier sits between operator runtime control (admin
endpoints) and the compiled-in defaults. Use it for
deployment-stable values that don't need the runtime API
surface — settings the operator wants survived across server
restarts but doesn't need to change on the hot path.

## Location

**Default**: `<data_directory>/runtime.yaml`, where
`<data_directory>` is `PDS_DATA_DIRECTORY` (the same root
the SQLite databases and blob store sit under).

**Override**: set `PDS_RUNTIME_FILE` to any absolute path:

```bash
PDS_RUNTIME_FILE=/etc/aurora-locus/runtime.yaml
```

If the file does not exist, the file tier is empty. There is
no error — file-tier configuration is optional.

## Format

Top-level YAML mapping. One key per `runtime_settings` key. Values
follow the same shape `setRuntimeSetting` accepts.

```yaml
moderation-mode: reduced
moderation-mode-redirect-url: https://example.org/maintenance
```

The set of accepted keys is the `KNOWN_RUNTIME_KEYS` allowlist
from `src/api/aurora_admin.rs`. New keys ship as one append to
the constant plus a corresponding `default_for_key` arm.

## Validation

**Unknown keys** (not in `KNOWN_RUNTIME_KEYS`): logged at WARN
level and skipped at load time. The cache does not hold them;
lookups for those keys fall through to the compiled-in default.
This catches operator typos without bringing the deployment
down — a misspelled key produces a visible warning in the
startup log instead of a silent partial-config.

**Invalid per-key values** (e.g., `moderation-mode: nonsense`
when the allowed set is `full | reduced | disabled`): same
treatment — logged at WARN and skipped. The compiled-in default
applies for that key. Per-key validation mirrors the rules
`setRuntimeSetting` enforces at the API boundary.

**Malformed YAML**: produces a **startup error** with the file
path in the message. There is no silent fallback to defaults —
operators expect the file to be authoritative when present.

**Top-level non-mapping** (e.g., a YAML scalar or sequence at
the root): produces a startup error.

## Lookup precedence in detail

For each key, `getRuntimeSetting` walks:

| Tier | Source | When applies |
|---|---|---|
| Recovery override | env var | `moderation-mode` reads only, when `AURORA_RECOVERY_MODE=true` |
| Runtime row | `runtime_settings` table | A row exists for the key |
| File tier | `runtime.yaml` cache | Key is in the cache (loaded at startup) |
| Default | `default_for_key` | None of the above |

The response's `source` field reflects which tier resolved the
read: `"Runtime"`, `"File"`, `"Default"`, or `"RecoveryMode"`.
External tooling reading the field can rely on this value set.
The field shape is open — new tier labels may be added additively
in future releases.

## Operator workflow

1. **Add a key**: edit `runtime.yaml` to include the key and
   value. Restart the server. Verify with:

   ```bash
   curl -H "Authorization: Bearer $TOKEN" \
        "https://$HOST/xrpc/tools.aurora.admin.getRuntimeSetting?key=moderation-mode"
   ```

   Confirm `"source": "File"` in the response.

2. **Override file-tier with a runtime row**: call
   `tools.aurora.admin.setRuntimeSetting` with the key and the
   new value. The runtime row takes precedence over file-tier
   on subsequent reads (`"source": "Runtime"`). The file-tier
   value is unchanged but shadowed.

3. **Remove a key**: delete the key from `runtime.yaml` and
   restart the server. If a runtime row exists, it remains
   effective; otherwise the lookup falls through to the
   compiled-in default.

4. **Verify which tier is in effect**: every `getRuntimeSetting`
   response includes the `source` field. The four documented
   values disambiguate.

## Reload semantics

The file is read **once at startup**. Changes to `runtime.yaml`
during runtime are not picked up until the server restarts.
For values that need to change on the hot path, use
`tools.aurora.admin.setRuntimeSetting` (writes a runtime row
which takes precedence over file tier).

Reload-on-`SIGHUP` is future work. The current design treats the
runtime_settings table as the in-process change mechanism and the
file tier as the deployment-stable layer.

## Security notes

- The file is read with the running process's filesystem
  permissions. Operators should restrict read access on
  deployments where the file may carry sensitive values.
- File-tier writes are not audit-chained. Changes to
  `runtime.yaml` happen out-of-band relative to the
  `audit_chain_entry` ledger — the file is operator
  configuration, not operator action. Runtime-row writes
  through `setRuntimeSetting` ARE audit-chained.

## Per-key value formats

Runtime settings are validated per-key at the API boundary
(`setRuntimeSetting`) and warned-and-skipped at file-tier
load time. The set of accepted keys is the `KNOWN_RUNTIME_KEYS`
allowlist in `src/api/aurora_admin.rs`; the per-key validation
rules live alongside the allowlist in `validate_runtime_value`.

| Key | JSON type | Allowed values | Default | Notes |
|-----|-----------|----------------|---------|-------|
| `moderation-mode` | String | `"full"` \| `"reduced"` \| `"disabled"` | `"full"` | Controls moderation surface visibility. `full` keeps the moderation domain visible to moderator-role operators; `reduced` collapses the moderation surface (operations-domain only); `disabled` hides moderation entirely. The `AURORA_RECOVERY_MODE=true` env-var override forces this back to `"full"` regardless of runtime / file tier. |
| `moderation-mode-redirect-url` | String | Any non-null string (including empty) | `""` | Where to redirect callers when `moderation-mode` is `reduced` or `disabled`. Empty string means no redirect (status-only response). The validator accepts any string shape; format checking (URL parse) is not enforced. |

### Adding a new runtime setting

Adding a new key in a future cycle is a four-step procedure:

1. Append the key constant + add it to `KNOWN_RUNTIME_KEYS` in
   `src/api/aurora_admin.rs`.
2. Add the per-key validation arm to `validate_runtime_value`
   in the same file.
3. Add a default in `default_for_key`.
4. Append a row to the table above documenting the value
   format, allowed values, default, and any semantic notes.

The table here is the operator-facing reference; the source
allowlist is the runtime enforcement. The two must stay in
sync — new keys must be documented here before the cycle that
ships them closes.
