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
The field shape is open per Arc 2's contract framing — new tier
labels may be added additively in future cycles.

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

Reload-on-`SIGHUP` is a v0.4 follow-up. The current design
treats the runtime_settings table as the in-process change
mechanism and the file tier as the deployment-stable layer.

## Security notes

- The file is read with the running process's filesystem
  permissions. Operators should restrict read access on
  deployments where the file may carry sensitive values.
- File-tier writes are not audit-chained. Changes to
  `runtime.yaml` happen out-of-band relative to the
  `audit_chain_entry` ledger — the file is operator
  configuration, not operator action. Runtime-row writes
  through `setRuntimeSetting` ARE audit-chained.
