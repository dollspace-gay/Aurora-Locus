# Arc 8 Phase B exercise script

Localhost smoke-test script for skydeval's Phase B sweep of Arc 8
(`chainlink #54` — Runtime route enumeration). Mirrors the
Arc 7 convention at
[`arc7-phase-b-commands.md`](arc7-phase-b-commands.md): curl
against `localhost:2583`, no deployment framing.

Arc 8's surface is narrow. `tools.aurora.describeCapabilities`
now reads from `RouteRegistry` instead of hand-curated lists.
There's no env-var toggle (single mode of operation), no DB
substrate to inspect (`Arc<RouteRegistry>` lives in process
memory), no reapers. Phase B verifies the curl-visible wire
output matches the snapshot, the registry stays consistent
with what axum serves, and the regression-detection tests
fire.

## Prerequisites

- Working dir: `/mnt/d/- - CODING/RUST/aurora-locus`.
- Branch `skydeval/v0.4-cycle` at the Arc 8 Step 4 tip
  (`bb6ef1b`) or its descendants.
- Free port 2583.
- `curl`, `jq` on the dev machine.

---

## Setup (one-time per session)

### Start the PDS

```bash
cargo run --bin aurora-locus -- serve
```

Expected log lines (order may vary):

```
Distributed-state substrate initialized (Postgres-CAS) ...
🚀 Aurora Locus PDS listening on 0.0.0.0:2583
```

The `RouteRegistry` is populated inside `api::routes()` and
parked on `AppContext` before `serve` runs. No dedicated
startup log line; observable via the Section A curls.

### Health probe

```bash
curl -s http://localhost:2583/health | jq
```

Expected: `{"status":"ok",...}`.

### Test accounts (skip if local state already has these)

Admin path — create the account, grant SuperAdmin, mint JWT:

```bash
curl -s -X POST http://localhost:2583/xrpc/com.atproto.server.createAccount \
  -H 'Content-Type: application/json' \
  -d '{"email":"alice@localhost","handle":"alice.localhost","password":"TestPassword123!"}' \
  | jq

cargo run --bin aurora-locus -- grant-admin \
  --did did:plc:<from-above> \
  --role SuperAdmin \
  --notes "Arc 8 Phase B sweep"

SESSION=$(curl -s -X POST http://localhost:2583/xrpc/com.atproto.server.createSession \
  -H 'Content-Type: application/json' \
  -d '{"identifier":"alice.localhost","password":"TestPassword123!"}')
export ADMIN_TOKEN=$(echo "$SESSION" | jq -r '.accessJwt')
```

Non-admin path (for A5 only):

```bash
curl -s -X POST http://localhost:2583/xrpc/com.atproto.server.createAccount \
  -H 'Content-Type: application/json' \
  -d '{"email":"bob@localhost","handle":"bob.localhost","password":"TestPassword123!"}' \
  | jq

SESSION_BOB=$(curl -s -X POST http://localhost:2583/xrpc/com.atproto.server.createSession \
  -H 'Content-Type: application/json' \
  -d '{"identifier":"bob.localhost","password":"TestPassword123!"}')
export USER_TOKEN=$(echo "$SESSION_BOB" | jq -r '.accessJwt')
```

Tokens last ~1 hour; re-mint if a 401 appears mid-sweep.

---

## Section A — describeCapabilities wire output

The load-bearing surface for Arc 8.

### A1. Probe returns 200 with the expected top-level keys

```bash
curl -s http://localhost:2583/xrpc/tools.aurora.describeCapabilities \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  | jq 'keys'
```

Expected: `["extensions","families","implementation","version"]`
(alphabetical — `jq | keys` sorts).

### A2. Live wire output sanity vs. snapshot literal

The canonical byte-for-byte check is the snapshot test
`test_admin_route_registry_completeness` at
`src/api/admin.rs:7401-7551` (run as C1 below). A2 is the
paranoia path that confirms a running PDS emits the same shape
and content; ordering differs from the test literal
(canonical-sort vs. registration-order), so this isn't a literal
diff.

```bash
curl -s http://localhost:2583/xrpc/tools.aurora.describeCapabilities \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  | jq -cS . > /tmp/arc8_capabilities_live.json
wc -c /tmp/arc8_capabilities_live.json
```

Expected: a single line of canonical JSON, ~1.5 KB. An empty
or truncated file points at auth or transport, not Arc 8.

### A3. Structural invariants

```bash
curl -s http://localhost:2583/xrpc/tools.aurora.describeCapabilities \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  | jq '{
      family_keys: (.families | keys),
      ext_count: (.extensions | length),
      ext_names: (.extensions | map(.name)),
      impl: .implementation,
      version: .version
    }'
```

Expected:

```json
{
  "family_keys": [
    "tools.aurora.admin",
    "tools.aurora.moderator",
    "tools.aurora.ops",
    "tools.aurora.superadmin"
  ],
  "ext_count": 14,
  "ext_names": [
    "subject-context-v1",
    "moderator-activity-v1",
    "subject-history-v1",
    "appeals-v1",
    "instance-metrics-v1",
    "mod-events-emit-v1",
    "batch-takedown-v1",
    "trigger-password-reset-v1",
    "moderation-metrics-v1",
    "queue-stats-v1",
    "audit-trail-v1",
    "forensic-export-v1",
    "mod-events-stream-v1",
    "runtime-settings-v1"
  ],
  "impl": "aurora-locus",
  "version": "0.3.0"
}
```

`ext_names` order is `WIRE_EXTENSION_ORDER` element-for-element;
`version` is `CARGO_PKG_VERSION`.

### A4. Unauthenticated → 401

```bash
curl -s -o /dev/null -w "%{http_code}\n" \
  http://localhost:2583/xrpc/tools.aurora.describeCapabilities
```

Expected: `401`.

### A5. Non-admin bearer → 403

```bash
curl -s -o /dev/null -w "%{http_code}\n" \
  http://localhost:2583/xrpc/tools.aurora.describeCapabilities \
  -H "Authorization: Bearer $USER_TOKEN"
```

Expected: `403`.

---

## Section B — RouteRegistry consistency

Each B exercise picks a route from a different attribution
category and confirms shape vs. reachability agree.

### B1. Ops-tier route reachable and advertised

```bash
curl -s -o /dev/null -w "%{http_code}\n" \
  http://localhost:2583/xrpc/tools.aurora.ops.getInstanceMetrics \
  -H "Authorization: Bearer $ADMIN_TOKEN"

curl -s http://localhost:2583/xrpc/tools.aurora.describeCapabilities \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  | jq '.families["tools.aurora.ops"] | index("getInstanceMetrics")'
```

Expected: `200` from the first; a non-null integer from the
second. A `null` from the second means registry registration
and axum mount drifted — the regression Arc 8 is designed to
surface.

### B2. Extension-attributed route reachable

`batchTakedownAccounts` is attributed to `batch-takedown-v1`
per Step 2. POST with an empty body proves the route is
mounted (handler returns 400, not the router's 404):

```bash
curl -s -o /dev/null -w "%{http_code}\n" \
  -X POST http://localhost:2583/xrpc/tools.aurora.admin.batchTakedownAccounts \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{}'

curl -s http://localhost:2583/xrpc/tools.aurora.describeCapabilities \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  | jq '{
      in_admin: (.families["tools.aurora.admin"] | index("batchTakedownAccounts")),
      in_extensions: (.extensions | map(.name) | index("batch-takedown-v1"))
    }'
```

Expected: `400` from the first (handler reached, body
invalid); both fields non-null from the second. A `404` from
the curl means the route isn't mounted; a `405` means it's
mounted under a different verb. Either is a regression.

### B3. List C route reachable but not in the registry

The bsky-PDS-compat namespace (`com.atproto.admin.*`) is List
C per §8.15 — reachable, not capability-registry-bearing.

```bash
curl -s -o /dev/null -w "%{http_code}\n" \
  "http://localhost:2583/xrpc/com.atproto.admin.getUsers?limit=1" \
  -H "Authorization: Bearer $ADMIN_TOKEN"

curl -s http://localhost:2583/xrpc/tools.aurora.describeCapabilities \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  | jq '[.families[] | .[]] | map(select(. == "getUsers"))'
```

Expected: `200` from the first; `[]` from the second. The
empty array confirms List C exclusion — plain `.route()`
registration (no `.route_with_caps()`) keeps the endpoint out
of the registry.

---

## Section C — Regression triggers

`cargo test` invocations that are the canonical correctness
gates for Arc 8's contract. Not curl-driven, but load-bearing.

### C1. Wire snapshot test

```bash
cargo test --lib test_admin_route_registry_completeness
```

Expected: `1 passed; 0 failed; 0 ignored`. The byte-for-byte
literal at `src/api/admin.rs:7408-7501` is the canonical
wire-shape pin.

### C2. Capability-string versioning contract phrase

```bash
cargo test --test contract_phrases wire_extension_order_has_versioning_pattern
```

Expected: `1 passed`. Verifies the Step 3 re-anchor against
`src/api/registry.rs`'s `WIRE_EXTENSION_ORDER` docblock.

### C3. Full lib suite (baseline)

```bash
cargo test --lib
```

Expected: `948 passed; 0 failed; 0 ignored`.

---

## Section D — §8.15 documentation readability

Observational, not curl-driven — §8.15 readability is skydeval's
domain per the existing Phase B convention. Read the updated
prose at `docs/AURORA_ADMIN_UI_DESIGN.md` §8.15 end-to-end. The
three checks below are load-bearing for Arc 8.

- **D1.** The three-step capability-addition procedure is
  followable. Pick a hypothetical capability (e.g.,
  `account-warnings-v1` introduced by
  `tools.aurora.admin.issueAccountWarning`) and walk the three
  steps. A reader should produce a working
  `.route_with_caps(...)` call without consulting other docs.
- **D2.** The omission policy makes `.omitted()` obvious for
  the v0.5+ case. At the v0.4 freeze, no routes are omitted
  (List A is empty per Step 0 Q6); the policy exists for the
  next addition.
- **D3.** The List C category list covers the eight categories
  (bsky-PDS-compat namespace, capability-registry meta-
  endpoint, public XRPC namespaces, health checks, admin UI
  static assets, public OAuth surface, internal OAuth
  bootstrap, Prometheus scrape) without enumerating individual
  routes. Adding a new route to an existing category should
  require no §8.15 update; only a *new* category triggers a
  doc change.

If any section is ambiguous, note it down and tighten in a
Phase B addendum rather than fixing inline.

---

## Section E — Decoupling sweep

Cycle-narrow check against just the Arc 8 diff:

```bash
git diff --name-only c151c39^..HEAD | while read f; do
  [ -f "$f" ] && git grep -in "cairn\|hideaway\|pursuingpeace\|nearhorizon" -- "$f"
done
```

Expected: zero hits across all 14 Arc-8-touched files. The
`horizon` token is excluded from the cycle-narrow grep — the
new §8.15 prose adds no new English
`horizontal/horizontally` instances; pre-existing hits in
that doc are listed below.

Cycle-wide:

```bash
git grep -i "cairn\|hideaway\|horizon\|pursuingpeace\|nearhorizon"
```

Expected hits: only documented false positives —

- English `horizontal` / `horizontally` / `horizons` in
  `docs/AURORA_ADMIN_UI_DESIGN.md` (including meta-references
  documenting this grep).
- Design-doc self-references listing the forbidden tokens as
  decoupling-discipline criteria (`docs/V0*_DESIGN.md`).
- Lucide icon names containing `more-horizontal` (admin UI).
- Prior Phase B scripts at
  `docs/internal/arc6-phase-b-script.md`,
  `docs/internal/arc7-phase-b-commands.md`, and this file
  listing the same grep commands and their documented false
  positives.

---

## Section F — Test suite verification

### F1. Lib tests

```bash
cargo test --lib
```

Expected: `948 passed; 0 failed; 0 ignored`.

### F2. Contract phrase tests

```bash
cargo test --test contract_phrases
```

Expected: `14 passed; 0 failed; 0 ignored`.

### F3. Cross-instance integration tests (Arc 7 baseline)

```bash
cargo test --test distributed_substrate_test
```

Expected: `11 passed; 0 failed; 0 ignored`. Arc 8 doesn't
touch substrate code; the Arc 7 cross-instance suite still
passes. Prerequisite: Docker daemon accessible.

### F4. Grant-admin CLI tests

```bash
cargo test --test grant_admin_test
```

Expected: `8 passed; 0 failed; 0 ignored`.

### F5. Lib clippy

```bash
cargo clippy --lib --no-deps
```

Expected: ~25 warnings (pre-existing patterns); zero new
warnings on Arc 8 files (`src/api/admin.rs`,
`src/api/registry.rs`, `src/api/aurora_admin.rs`,
`src/api/aurora_moderator.rs`, `src/api/aurora_subscribe.rs`,
`src/api/mod.rs`, `src/auth.rs`, `src/context.rs`,
`src/main.rs`, `src/server.rs`).

---

## Notes

- **Token expiry**: JWT lasts ~1 hour; re-mint via Setup's
  `createSession` curl if a 401 appears mid-sweep.

- **No mode toggles in Arc 8**: unlike Arc 7's
  `PDS_DISTRIBUTED_STATE_MODE`, Arc 8 introduces no
  env-var-driven paths. The `RouteRegistry` is built at
  startup and is the sole source of truth at request time;
  no restart sequence is needed beyond the initial
  `cargo run --bin aurora-locus -- serve`.

- **No DB substrate to inspect**: Arc 8's runtime data lives
  in process memory (`Arc<RouteRegistry>` on `AppContext`).
  There is no `sqlite3 data/account.sqlite` query that exposes
  registry state. Section A's curls + Section C's snapshot
  test together exercise what substrate-table queries did for
  Arc 7.

- **No reapers**: Arc 8 adds no background jobs. The registry
  is static after startup.

- **A2 isn't a literal diff**: `jq -cS` produces canonical
  (alphabetical) ordering, while the test literal uses
  registration order per family. Treat A2's output as a shape
  and content sanity check against a live PDS; the canonical
  byte-for-byte gate is C1.

- **B2's 400-vs-404 distinction**: POST routes with required
  bodies return 400 from the handler when the body is empty;
  a 404 from the router would mean the route isn't mounted.
  That distinction is what proves the registry's attribution
  and the axum mount agree.

- **"If something looks off"**: same convention as Arc 6/7 —
  document expected vs actual in a Phase B addendum (separate
  file under `docs/internal/`), don't push, drop back to Nova
  for triage.

---

## Sign-off

Once all sections clear:

1. Document any findings or regressions in a Phase B addendum
   (separate file under `docs/internal/`).
2. If clean, Arc 8 closes; chainlink #54 can be closed.
3. v0.4 cycle close gate: all per-arc Phase B sweeps must
   pass before cycle-close release work begins.
