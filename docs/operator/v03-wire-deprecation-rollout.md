# v0.3 wire-shape deprecation rollout

## Summary

Aurora-Locus v0.3 introduced canonical wire shapes for two
admin-tier endpoints that previously shipped v0.2 shapes. v0.4
ships dual-shape acceptance: the PDS accepts both canonical
(v0.3) and legacy (v0.2) shapes during the migration window, with
observability so operators can track who's still on the legacy
form and decide when to commit to a removal.

This document is the operator-facing reference for the
deprecation rollout. Read it if you run:

- Custom admin-tier client tooling (scripts, custom UIs).
- Third-party admin services that call Aurora-Locus directly.
- A non-default Aurora Admin UI build (everything in
  `static/admin/scripts/` is the canonical UI; customized
  builds may have drifted).

If you exclusively use the stock Aurora Admin UI shipped with
this PDS, your client is already on canonical shapes — Arc 6
(v0.4 cycle) migrated it. The deprecation signals exist for the
benefit of out-of-tree clients.

## Affected endpoints

| Endpoint | Canonical field | Legacy field | Shape label |
|---|---|---|---|
| `tools.aurora.admin.emitEvent` | `subjects` (array of Subject) | `subject` (single Subject) | `v0.2_single_subject` |
| `com.atproto.admin.updateSubjectStatus` (RepoBlobRef subject variant) | `record_uri` (snake_case) | `recordUri` (camelCase) | `v0.2_camelCase_record_uri` |

Both endpoints accept either shape during the deprecation
window. Legacy shapes are normalized to canonical form before
the handler runs; the response shape is unchanged from v0.3.

## Migration window

**No committed sunset date.** Operators with legacy clients have
indefinite time to migrate. The deprecation observability is
already live; the removal of legacy-shape support waits on
operator readiness — see [Sunset configuration](#sunset-configuration)
for committing to a date when you're ready.

## Deprecation signals

### Metrics counter

The PDS exposes a Prometheus counter at `/metrics`:

```
aurora_legacy_wire_ingest_total{endpoint, shape, field}
```

Labels:
- `endpoint`: NSID of the receiving handler.
- `shape`: high-level legacy shape identifier (one of
  `v0.2_single_subject`, `v0.2_camelCase_record_uri`).
- `field`: specific legacy field name (`subject`, `recordUri`).
  Today every (endpoint, shape) pair has exactly one field; the
  label slot is forward-looking for future deprecations that
  may bundle multiple fields under one shape label.

Incremented once per legacy-shape request received. Useful
PromQL queries:

```promql
# Total legacy ingest count per endpoint:
sum by (endpoint) (aurora_legacy_wire_ingest_total)

# Per-field migration progress (granular):
sum by (endpoint, field) (aurora_legacy_wire_ingest_total)

# Rate of legacy ingest over the last hour:
rate(aurora_legacy_wire_ingest_total[1h])

# Migration progress over a week (lower-right corner means
# done — no legacy ingest in the recent window):
sum by (endpoint) (
  increase(aurora_legacy_wire_ingest_total[7d])
)
```

### Structured tracing

Every legacy-shape request also logs an `info!`-level event:

```
INFO legacy_wire_shape_ingested
  endpoint="<endpoint NSID>"
  shape="<shape label>"
  field="<field name>"
```

Cross-reference with the per-request access logs (by trace
context or request ID, depending on your log shipping config)
to identify specific clients.

### Response headers — NOT emitted

The kickoff design for Step 7 specified four response headers on
legacy-shape responses: `Deprecation: true`, `Sunset: <date>`,
`Warning: 299 - "…"`, and `X-Wire-Migration-Guide` (pointing at
this doc). **These headers are not currently emitted.** Implementing
them would require changes to the handler return types that ripple
through ~30 internal test call sites; the metrics counter +
structured log alone meet the observability goal for the
operator-facing side of §5.3.6.

If your client tooling needs per-response self-discovery of
deprecation (vs. operator-side polling via Prometheus), file an
issue — a follow-up cycle can add the headers without disturbing
the existing wire contract.

## Sunset configuration

When your migration is complete and you want the PDS to commit
to a removal date for legacy-shape acceptance, configure the
sunset via env var:

```
PDS_V03_WIRE_SUNSET_DATE="Wed, 01 Jan 2027 00:00:00 GMT"
```

Format: HTTP-date (RFC 7231 IMF-fixdate). The value is parsed
by the `emit_legacy_wire_headers` helper in
`src/api/middleware.rs` and emitted as a `Sunset` response
header on legacy-shape responses, alongside `Deprecation: true`.

When the env var is unset (or set to the literal string
`"deprecated"`), no `Sunset` header is emitted. The PDS still
accepts legacy shapes; only the operator-driven commitment is
absent.

(Note: the header helper exists today but is not currently
wired into the handler-response path — see "Response headers
— NOT emitted" above. Setting `PDS_V03_WIRE_SUNSET_DATE` is a
no-op until the headers ship. The env var is documented here
in advance so operators can plan rollouts; when the headers
land, this section becomes immediately operational.)

## Migration checklist

For each legacy client identified via the counter / log:

1. **Identify the client**. Use the counter labels +
   `legacy_wire_shape_ingested` log events to pin down which
   client tools / IPs still send legacy shapes.

2. **Update the client's request construction**:
   - **`emitEvent`**: wrap the single subject in a one-element
     array.
     ```
     // Before (legacy v0.2):
     POST /xrpc/tools.aurora.admin.emitEvent
     { "action": {...}, "subject": {...}, "rationale": "..." }

     // After (canonical v0.3):
     POST /xrpc/tools.aurora.admin.emitEvent
     { "action": {...}, "subjects": [{...}], "rationale": "..." }
     ```
   - **`updateSubjectStatus` blob variant**: rename `recordUri`
     to `record_uri` in the RepoBlobRef subject.
     ```
     // Before (legacy v0.2):
     "subject": {
       "$type": "com.atproto.admin.defs#repoBlobRef",
       "did": "did:plc:...",
       "cid": "bafy...",
       "recordUri": "at://..."   // camelCase
     }

     // After (canonical v0.3):
     "subject": {
       "$type": "com.atproto.admin.defs#repoBlobRef",
       "did": "did:plc:...",
       "cid": "bafy...",
       "record_uri": "at://..."  // snake_case
     }
     ```

3. **Verify the counter no longer increments** for that
   client. Wait at least one full traffic cycle (often
   24 hours for human-operator clients, less for automated
   ones) and re-check.

4. **Repeat** for every client surfaced by the counter.

5. **After all clients have migrated**: optionally set
   `PDS_V03_WIRE_SUNSET_DATE` to commit to a date. Document
   the commitment to your downstream consumers.

6. **After the sunset date**: legacy-shape support can be
   removed from the PDS in a future release. Coordinate the
   removal with downstream operators ahead of the date.

## Sending both shapes simultaneously — forbidden

Both dual-shape implementations reject requests that include
both the canonical AND the legacy field name simultaneously.
This catches accidental client behavior where a migration is
mid-flight and both forms are populated. The PDS returns a
`400 Bad Request` with an error message naming the conflict:

```
"emitEvent accepts either canonical 'subjects' (array of Subject)
 or legacy 'subject' (single Subject), not both; pick exactly one
 shape per request"
```

```
"RepoBlobRef subject accepts either canonical 'record_uri'
 (snake_case) or legacy 'recordUri' (camelCase), not both;
 pick exactly one shape per request"
```

If your client's request-builder produces both forms during a
migration, fix it to emit one canonically; the PDS won't
silently pick one and the operator wouldn't catch the ambiguity
without this guard.

## Related documentation

- `docs/V04_DESIGN.md` §5.3.6 — original design rationale.
- `docs/operator/contract-stability.md` — surrounding
  framework for wire-shape commitments.
- `docs/operator/running-ui-tests.md` — Arc 6 Step 5's
  test-harness recon, useful when validating client UI
  changes after migration.
