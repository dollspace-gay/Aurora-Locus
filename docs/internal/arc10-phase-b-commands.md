# Arc 10 Phase B exercise script

Localhost smoke-test script for skydeval's Phase B sweep of Arc 10
(`chainlink #57` — GC sweep for orphaned blob storage). Mirrors
the Arc 7/8/9 convention at
[`arc9-phase-b-commands.md`](arc9-phase-b-commands.md): curl
against `localhost:2583`, `cargo` invocations for deterministic
test-infra checks, no deployment framing.

> **Arc 11 dependency**: the Setup section uses
> [`dev.aurora.*`](dev-routes.md) HTTP endpoints introduced by
> Arc 11 (chainlink #56). The dev endpoints are present in
> debug builds only via `#[cfg(debug_assertions)]`; running
> Phase B against a release build requires falling back to the
> legacy `cargo run -- grant-admin` CLI (and accepting the
> stop-PDS / restart-PDS cycle Arc 11 was built to eliminate).
> Arc 10's CLI and scheduled-job surfaces are themselves
> debug-build-agnostic; only the Setup section depends on
> Arc 11.

Arc 10 ships the GC sweep for orphaned blob storage. Two
operator-facing consumers — the scheduled background job
`gc_sweep_job` and the offline-only `aurora-locus gc-sweep`
CLI — both call the Step 2 sweep primitive at
[`crate::blob_store::gc::run_sweep`](../../src/blob_store/gc.rs#L244)
through the [`BlobStore::run_gc_sweep`](../../src/blob_store/store.rs#L429)
wrapper. Phase B exercises:

- the synthetic IN-clause benchmark (Section A, Step 1 surface),
- the scheduled background job under both
  enabled-via-env and disabled-default modes (Section B),
- the CLI subcommand including `LivenessLock` fail-fast and
  override flags (Section C),
- the four `validate_gc_sweep_config` warnings under each
  triggering env-var combination (Section D),
- the three new Prometheus metrics (Section E),
- the operator doc structure (Section F),
- the regression-baseline gates (Section G),
- the decoupling sweep (Section H),
- an optional end-to-end orphan-deletion test if the dev
  environment supports synthetic blob writes (Section I).

## Prerequisites

- Working dir: `/mnt/d/- - CODING/RUST/aurora-locus`.
- Branch `skydeval/v0.4-cycle` at the Arc 10 Step 4 tip
  (`2c44ff5`) or its descendants.
- Free port 2583.
- `curl`, `jq` on the dev machine.
- `sqlite3` for the optional cross-reference in B / I.
- Linux/macOS dev environment (`touch -d` for backdating
  mtimes) for the optional Section I orphan-deletion test;
  Windows-native dev environments skip Section I.

---

## Setup (one-time per session)

### Start the PDS

```bash
cargo run -- serve
```

Expected log lines (order may vary):

```
Distributed-state substrate initialized (Postgres-CAS) ...
🚀 Aurora Locus PDS listening on 0.0.0.0:2583
```

If `gc_sweep.enabled` is unset (the default), the startup
log also emits (at debug level — visible only with
`RUST_LOG=debug` or similar):

```
GC sweep job disabled (gc_sweep.enabled = false)
```

Source: [src/jobs/mod.rs:155](../../src/jobs/mod.rs#L155).

### Health probe

```bash
curl -s http://localhost:2583/health | jq
```

Expected: `{"status":"ok",...}`.

### Provision the admin account (four POSTs, zero PDS restarts)

Arc 11's [`dev.aurora.*`](dev-routes.md) HTTP endpoints replace
the legacy `cargo run -- grant-admin` ceremony. The four POSTs
below provision an admin account end-to-end against the running
PDS — no stop, no restart, no `createSession` follow-up.

Arc 10 has **no sacrificial subject account**: the GC sweep
walks all blob storage and doesn't accept a per-account
subject. Only the admin account is needed (and only for
exercises that hit the running PDS — Sections B and E).

```bash
# 1. Create the admin account. Body shape per
#    src/api/dev_routes.rs:227-233 (CreateAccountBody);
#    response per src/api/dev_routes.rs:235-244
#    (CreateAccountResponse, `rename_all = "camelCase"`).
ADMIN_RESP=$(curl -s -X POST http://localhost:2583/xrpc/dev.aurora.createAccount \
  -H 'Content-Type: application/json' \
  -d '{
    "handle": "admin.localhost",
    "email": "admin@localhost",
    "password": "TestPassword123!"
  }')
export ADMIN_DID=$(echo "$ADMIN_RESP" | jq -r '.did')
echo "Admin DID: $ADMIN_DID"

# 2. Grant SuperAdmin. Body shape per
#    src/api/dev_routes.rs:77-86 (GrantAdminBody); response
#    per src/api/dev_routes.rs:88-93 (GrantAdminResponse).
curl -s -X POST http://localhost:2583/xrpc/dev.aurora.grantAdmin \
  -H 'Content-Type: application/json' \
  -d "{\"did\":\"$ADMIN_DID\",\"role\":\"superadmin\"}" \
  | jq

# 3. Mint a JWT carrying the new role. Body per
#    src/api/dev_routes.rs:296-300 (MintTokenBody); response
#    per src/api/dev_routes.rs:302-307 (MintTokenResponse —
#    wire field is `accessJwt` per `rename_all = "camelCase"`).
export ADMIN_TOKEN=$(curl -s -X POST http://localhost:2583/xrpc/dev.aurora.mintToken \
  -H 'Content-Type: application/json' \
  -d "{\"did\":\"$ADMIN_DID\"}" \
  | jq -r '.accessJwt')
echo "Token prefix: ${ADMIN_TOKEN:0:32}..."

# 4. Verify the token works against an admin-tier endpoint.
curl -s http://localhost:2583/xrpc/tools.aurora.describeCapabilities \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  | jq 'keys'
```

Expected: step 4 returns
`["extensions","families","implementation","version"]`. If
step 4 returns a 401/403 instead, the grant didn't land
correctly — re-check step 2's response (`role` field should
be `"superadmin"`). Tokens last ~1 hour; re-run step 3 if a
401 appears mid-sweep.

---

## Section A — `BlobBackend::list_all_blobs` trait surface (Step 1)

The trait method is library-internal — not directly curl-able.
Section A confirms the synthetic benchmark stays index-driven
in the operator's environment and that the trait method's
behaviour is unchanged from Step 1's verification.

### A1. Re-run the synthetic IN-clause benchmark

```bash
cargo test --test blob_in_clause_benchmark -- --ignored --nocapture
```

Expected:

```
Seeding 100000 synthetic blob rows...
Seed complete in <~15-25s>
IN-clause query at page_size=500: returned 250 present CIDs in <~5-10>ms
Query plan:
  SEARCH blob USING COVERING INDEX sqlite_autoindex_blob_1 (cid=?)
test benchmark_in_clause_query_at_page_500 ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in ~20s
```

The 50ms pass-fail threshold (defined at
[tests/blob_in_clause_benchmark.rs:43](../../tests/blob_in_clause_benchmark.rs#L43))
must hold; a plan that mentions `SCAN` instead of `SEARCH`
is a regression in the planner's choice and is the Step 0 Q5
fallback hierarchy's trigger (drop page size → per-CID
lookups → temp-table-join).

### A2. (Optional, deferred) Postgres-side benchmark

Currently a v0.6 candidate — no Postgres-side
`EXPLAIN ANALYZE` benchmark ships in v0.4. The SQLite synthetic
benchmark + the cross-backend `$1, $2, ...` placeholder syntax
via `sqlx::AnyPool` keep confidence high on Postgres without
additional scaffolding. Skip on Phase B; promote in a future
cycle if operator-driven Postgres rollouts surface a need.

---

## Section B — Scheduled background job (`gc_sweep_job`)

Exercises the scheduled mode against a live PDS. The
substantive smoke test for Arc 10's online path.

### B1. Confirm off-by-default

With no `PDS_GC_SWEEP_*` env vars set and no `gc_sweep`
block in the config file, restart the PDS:

```bash
# Stop the previously-started PDS if any (Ctrl-C in its shell).
RUST_LOG=aurora_locus=debug cargo run -- serve 2>&1 | grep "GC sweep"
```

Expected (within a few seconds of startup):

```
GC sweep job disabled (gc_sweep.enabled = false)
```

Source: [src/jobs/mod.rs:155](../../src/jobs/mod.rs#L155). The
`debug!` log level is intentional — operators who haven't
opted in shouldn't see a log line on every startup. Raise to
debug to confirm the gating path is taken; the absence of
the "GC sweep job scheduled" `info!` line is itself
confirmation under default RUST_LOG.

Stop the PDS (Ctrl-C) before B2.

### B2. Enable scheduled mode via env var + short interval

```bash
export PDS_GC_SWEEP_ENABLED=true
export PDS_GC_SWEEP_INTERVAL_SECS=30        # 30s for testing
export PDS_GC_SWEEP_DRY_RUN=true            # safe default; explicit here
RUST_LOG=aurora_locus=info cargo run -- serve
```

Expected log within a few seconds of startup (from
[src/jobs/mod.rs:148-153](../../src/jobs/mod.rs#L148-L153)):

```
GC sweep job scheduled interval_secs=30 dry_run=true max_deletes_per_run=10000
```

The `tokio::time::interval` first tick fires immediately
(matches `temp_blob_cleanup_job`'s shape), so the first sweep
runs within milliseconds of the "scheduled" log line.

### B3. Confirm the first sweep ran

Within ~30 seconds after the "scheduled" log line, expected
sequence (from [src/jobs/mod.rs:319-339](../../src/jobs/mod.rs#L319-L339)):

```
Running GC sweep job  dry_run=true max_deletes_per_run=10000 page_size=500
GC sweep complete  pages_scanned=N blobs_examined=M authorized=... \
  in_flight=... too_young=... confirmed_orphans_found=... \
  orphans_deleted=... orphans_skipped_safety_cap=... duration_seconds=...
```

For a fresh PDS with no blob uploads, expected report counts:

```
pages_scanned=1
blobs_examined=0
authorized=0
in_flight=0
too_young=0
confirmed_orphans_found=0
orphans_deleted=0
orphans_skipped_safety_cap=0
duration_seconds=<small, typically <0.01>
```

`pages_scanned` is 1 (not 0) because the empty-store path in
[src/blob_store/gc.rs:262-264](../../src/blob_store/gc.rs#L262-L264)
increments `pages_scanned` before the early-break check.

### B4. Wait one interval; confirm the loop ticks

Leave the PDS running. The next "GC sweep complete" line
appears ~30 seconds later (the configured
`PDS_GC_SWEEP_INTERVAL_SECS`). Counters reset between runs
in the sense that each emission is a single sweep's
report; the Prometheus counters (Section E) accumulate
across runs.

### B5. Disable + restart cycle

Stop the PDS (Ctrl-C):

```bash
unset PDS_GC_SWEEP_ENABLED PDS_GC_SWEEP_INTERVAL_SECS PDS_GC_SWEEP_DRY_RUN
RUST_LOG=aurora_locus=debug cargo run -- serve 2>&1 | grep "GC sweep"
```

Expected:

```
GC sweep job disabled (gc_sweep.enabled = false)
```

Confirms the env-var change took effect after restart (the
config is loaded once at startup; runtime env-var changes
have no effect until the next restart).

---

## Section C — CLI subcommand (`aurora-locus gc-sweep`)

Exercises the offline-only CLI path. All exercises require
**no PDS running** (the `LivenessLock` would otherwise
fast-fail per C3).

### C1. CLI signature verification

```bash
cargo run -- gc-sweep --help
```

Expected (verified against
[src/cli/mod.rs:233-258](../../src/cli/mod.rs#L233-L258) +
the rendered `clap` output captured during Step 3
verification):

```
Run an Arc 10 GC sweep against the offline PDS. Reconciles
blob storage against the `blob` / `temp_blob_metadata` tables
and deletes confirmed orphans. CLI is offline-only (acquires
`LivenessLock`); for online sweeps, enable the scheduled
`gc_sweep_job` via `PDS_GC_SWEEP_ENABLED=true`. Step 3 /
chainlink #57

Usage: aurora-locus gc-sweep [OPTIONS]

Options:
      --dry-run
          Force `dry_run` on regardless of `gc_sweep.dry_run`
          in config. There is no `--no-dry-run`; flip
          `PDS_GC_SWEEP_DRY_RUN=false` in config to enable
          destructive mode
      --report-only
          Force `report_only` on — classify and log only.
          Operator-intent disambiguation; see
          `cli::gc_sweep::run` docs
      --max-deletes <MAX_DELETES>
          Override `gc_sweep.max_deletes_per_run`
      --threshold-secs <THRESHOLD_SECS>
          Override `gc_sweep.freshness_threshold_secs`
      --page-size <PAGE_SIZE>
          Override `gc_sweep.page_size`
  -h, --help
          Print help
```

Five overrides total: `--dry-run`, `--report-only`,
`--max-deletes`, `--threshold-secs`, `--page-size`. **No
`--no-dry-run`** — confirms the safety-direction-only
override is intentional.

### C2. Run CLI against an offline PDS

Confirm no PDS is running on port 2583. Then:

```bash
cargo run -- gc-sweep
```

Expected output (params block + report block, verified
against
[src/cli/gc_sweep.rs:80-99](../../src/cli/gc_sweep.rs#L80-L99)):

```
GC sweep starting:
  dry_run:             true
  report_only:         false
  max_deletes_per_run: 10000
  freshness_threshold: 3600s
  page_size:           500

GC sweep complete:
  pages scanned:               1
  blobs examined:              0
  authorized:                  0
  in-flight:                   0
  too young:                   0
  confirmed orphans found:     0
  orphans deleted:             0
  orphans skipped (safety cap): 0
  duration:                    0.00s
```

The `freshness_threshold` line prints the `Duration`'s `Debug`
representation (per `params.freshness_threshold` formatted as
`{:?}` at [src/cli/gc_sweep.rs:84](../../src/cli/gc_sweep.rs#L84)),
which renders as `3600s` for the 1-hour default.

`dry_run: true` is the default per
[src/config.rs:213](../../src/config.rs#L213); the CLI
inherits this when no override is passed.

### C3. Confirm `LivenessLock` fail-fast when PDS is running

In one terminal:

```bash
cargo run -- serve
```

In another, with the PDS still running:

```bash
cargo run -- gc-sweep
echo "exit: $?"
```

Expected: non-zero exit code; error message from
[src/cli/gc_sweep.rs:55-63](../../src/cli/gc_sweep.rs#L55-L63):

```
Cannot run gc-sweep: <LivenessLock-error-from-acquire>
Stop the PDS before running gc-sweep, or enable the
scheduled `gc_sweep_job` via PDS_GC_SWEEP_ENABLED=true
for online sweeps.
```

The exact `<LivenessLock-error-from-acquire>` payload
depends on the database backend (Postgres advisory lock vs.
SQLite file lock per
[src/db/liveness_lock.rs:97-160](../../src/db/liveness_lock.rs#L97-L160));
both backends produce a clear "lock held by another
process" diagnostic. No sweep should execute — the lock
acquisition fails before any storage walk.

Stop the PDS before continuing.

### C4. Test CLI overrides

With no PDS running:

```bash
cargo run -- gc-sweep --report-only --threshold-secs 7200 --max-deletes 5 --page-size 100
```

Expected params block reflects the overrides (verified
against the override apply block at
[src/cli/gc_sweep.rs:66-78](../../src/cli/gc_sweep.rs#L66-L78)):

```
GC sweep starting:
  dry_run:             true
  report_only:         true
  max_deletes_per_run: 5
  freshness_threshold: 7200s
  page_size:           100
```

`dry_run` remains `true` because the config default is
`true` and the CLI didn't pass `--dry-run` to force it (the
override only forces dry_run ON, never OFF; per the
safety-direction-only design).

### C5. Confirm `--dry-run` is safety-direction-only

```bash
cargo run -- gc-sweep --no-dry-run 2>&1 | head -5
```

Expected: clap error citing `--no-dry-run` as an
unrecognised flag (the variant has no inverse — see
[src/cli/mod.rs:238-239](../../src/cli/mod.rs#L238-L239)
where `#[arg(long)]` on `dry_run: bool` produces only
`--dry-run`, no `--no-dry-run`).

---

## Section D — `validate-config` warnings (Step 3)

Exercises the four warnings in
[`validate_gc_sweep_config`](../../src/cli/validate_config.rs#L600).
All warnings are gated on
[`config.gc_sweep.enabled = true`](../../src/cli/validate_config.rs#L601-L604)
— off-by-default deployments stay warning-free.

### D1. Baseline (sweep disabled, no warnings)

```bash
unset PDS_GC_SWEEP_ENABLED PDS_GC_SWEEP_DRY_RUN \
      PDS_GC_SWEEP_MAX_DELETES_PER_RUN \
      PDS_GC_SWEEP_FRESHNESS_THRESHOLD_SECS \
      PDS_GC_SWEEP_INTERVAL_SECS PDS_GC_SWEEP_PAGE_SIZE
cargo run -- validate-config 2>&1 | grep -A 1 "GcSweep"
```

Expected: empty output (no GcSweep-category warnings render
because the `enabled = false` early-return at
[src/cli/validate_config.rs:601-604](../../src/cli/validate_config.rs#L601-L604)
short-circuits the function). The rest of the
validate-config output may show other unrelated warnings
(production-readiness checks, etc.) but no GcSweep entries.

### D2. Enabled + dry-run-false advisory

```bash
export PDS_GC_SWEEP_ENABLED=true
export PDS_GC_SWEEP_DRY_RUN=false
cargo run -- validate-config 2>&1 | grep -A 2 "GcSweep"
```

Expected (from
[src/cli/validate_config.rs:606-614](../../src/cli/validate_config.rs#L606-L614)):

```
  [GcSweep] dry_run is false - sweep will perform real deletes. Recommend running with dry_run=true for at least 7 days before enabling destructive mode to verify classification accuracy on this deployment's workload.
```

Only one warning fires here — `max_deletes_per_run` is still
the default (10,000), under the 100,000 threshold for the
blast-radius advisory.

### D3. High `max_deletes_per_run` + destructive

Keeping D2's env vars set:

```bash
export PDS_GC_SWEEP_MAX_DELETES_PER_RUN=500000
cargo run -- validate-config 2>&1 | grep -A 2 "GcSweep"
```

Expected: both the dry-run-false advisory **and** the
blast-radius advisory from
[src/cli/validate_config.rs:616-627](../../src/cli/validate_config.rs#L616-L627):

```
  [GcSweep] dry_run is false - sweep will perform real deletes. ...
  [GcSweep] max_deletes_per_run is 500000 (>100,000) and dry_run is false - a single misclassification could delete many blobs. Consider a lower cap until operational data confirms classification accuracy.
```

The blast-radius advisory is nested under the dry-run-false
branch ([validate_config.rs:606-628](../../src/cli/validate_config.rs#L606-L628))
— it never fires when `dry_run = true` regardless of cap
size. Confirm this by setting `PDS_GC_SWEEP_DRY_RUN=true`
and re-running; the blast-radius warning disappears.

### D4. Short freshness threshold

Reset and re-set:

```bash
unset PDS_GC_SWEEP_DRY_RUN PDS_GC_SWEEP_MAX_DELETES_PER_RUN
export PDS_GC_SWEEP_ENABLED=true
export PDS_GC_SWEEP_FRESHNESS_THRESHOLD_SECS=300
cargo run -- validate-config 2>&1 | grep -A 2 "GcSweep"
```

Expected (from
[src/cli/validate_config.rs:630-642](../../src/cli/validate_config.rs#L630-L642)):

```
  [GcSweep] freshness_threshold_secs is 300 (<10 minutes) - increases risk of classifying genuine in-flight uploads as orphans if the upload's `temp_blob_metadata` row hasn't committed by sweep time. Recommend >=3600 (1 hour) unless operational data justifies tightening.
```

The threshold warning fires independently of `dry_run` — a
too-aggressive threshold is a misclassification risk
regardless of whether the sweep deletes.

### D5. Short interval

Reset and re-set:

```bash
unset PDS_GC_SWEEP_FRESHNESS_THRESHOLD_SECS
export PDS_GC_SWEEP_ENABLED=true
export PDS_GC_SWEEP_INTERVAL_SECS=600
cargo run -- validate-config 2>&1 | grep -A 2 "GcSweep"
```

Expected (from
[src/cli/validate_config.rs:644-654](../../src/cli/validate_config.rs#L644-L654)):

```
  [GcSweep] interval_secs is 600 (<1 hour) - sweep cadence may exceed throughput on large stores. Recommend >=21600 (6 hours) unless operational data justifies tightening.
```

### D6. All four warnings simultaneously

```bash
export PDS_GC_SWEEP_ENABLED=true
export PDS_GC_SWEEP_DRY_RUN=false
export PDS_GC_SWEEP_MAX_DELETES_PER_RUN=500000
export PDS_GC_SWEEP_FRESHNESS_THRESHOLD_SECS=300
export PDS_GC_SWEEP_INTERVAL_SECS=600
cargo run -- validate-config 2>&1 | grep "GcSweep"
```

Expected: exactly four `[GcSweep]` lines, one per warning
listed above. Validation overall still succeeds (warnings
don't block startup; only `Severity::Error` issues exit
non-zero per
[src/cli/validate_config.rs:127-131](../../src/cli/validate_config.rs#L127-L131)).

Clean up before continuing to Section E:

```bash
unset PDS_GC_SWEEP_DRY_RUN PDS_GC_SWEEP_MAX_DELETES_PER_RUN \
      PDS_GC_SWEEP_FRESHNESS_THRESHOLD_SECS PDS_GC_SWEEP_INTERVAL_SECS
```

---

## Section E — Prometheus metrics

Exercises the three metrics registered in
[src/metrics.rs:265-285](../../src/metrics.rs#L265-L285).

### E1. Start scheduled mode + wait one sweep

```bash
export PDS_GC_SWEEP_ENABLED=true
export PDS_GC_SWEEP_INTERVAL_SECS=30
cargo run -- serve
```

Wait ~35 seconds (one interval plus margin) for the first
sweep to complete; confirm "GC sweep complete" appears in
the log.

### E2. Scrape the metrics endpoint

In another terminal:

```bash
curl -s http://localhost:2583/metrics | grep -A 1 "gc_sweep"
```

Expected three metric families, registered at
[src/metrics.rs:265](../../src/metrics.rs#L265),
[src/metrics.rs:275](../../src/metrics.rs#L275), and
[src/metrics.rs:282](../../src/metrics.rs#L282):

```
# HELP gc_sweep_orphans_found_total Total blobs classified as confirmed orphans during GC sweeps
# TYPE gc_sweep_orphans_found_total counter
gc_sweep_orphans_found_total 0
# HELP gc_sweep_orphans_deleted_total Total orphans deleted during GC sweeps
# TYPE gc_sweep_orphans_deleted_total counter
gc_sweep_orphans_deleted_total 0
# HELP gc_sweep_duration_seconds Duration of GC sweep runs (seconds)
# TYPE gc_sweep_duration_seconds histogram
gc_sweep_duration_seconds_bucket{le="0.005"} 1
gc_sweep_duration_seconds_bucket{le="0.01"} 1
...
gc_sweep_duration_seconds_sum <small-value>
gc_sweep_duration_seconds_count 1
```

The two counters report 0 because the fresh PDS has no
storage entries that classify as orphans. The histogram's
`_count` advances by 1 per sweep run (incremented at
[src/blob_store/gc.rs:354](../../src/blob_store/gc.rs#L354)).

The `/metrics` route is registered at
[src/server.rs:77](../../src/server.rs#L77); the handler at
[src/server.rs:115](../../src/server.rs#L115) emits the
default Prometheus text format covering every metric in
[src/metrics.rs](../../src/metrics.rs).

### E3. Counter increments with orphans (optional)

Skip if Section I (end-to-end orphan-deletion) is not
exercised — there's no way to produce a non-zero
`gc_sweep_orphans_found_total` without test orphan
candidates in storage. The bookkeeping at
[src/blob_store/gc.rs:351-354](../../src/blob_store/gc.rs#L351-L354)
increments `_found_total` by `confirmed_orphans_found` and
`_deleted_total` by `orphans_deleted` after each sweep
completes; both counters are monotonic across sweep runs.

Stop the PDS before continuing.

---

## Section F — Operator doc readability

Observational — skydeval's domain.

### F1. Confirm section header inventory

```bash
grep -E "^## " docs/operator/blob-gc-sweep.md
```

Expected exactly eight `##` headers in this order:

- What the sweep does
- When to enable the sweep
- Enabling the sweep
- The dry-run shakedown
- CLI subcommand
- Metrics
- Troubleshooting
- Configuration reference

Plus a "Related" footer (technically also `##` but
positioned as a closing section).

### F2. Verify the dry-run shakedown procedure is followable

Read the "The dry-run shakedown" section. The procedure
should be unambiguous about:

- The mandatory `dry_run: true` start state.
- The 7-day duration (7 sweep runs at default 24h cadence).
- The three verification axes: classification accuracy
  (manual SQL cross-references), orphan rate sanity (per-
  sweep `confirmed_orphans_found` count), and sweep
  duration (`duration_seconds` vs `interval_secs`).
- The promotion path: flip `dry_run: false` and restart the
  PDS.

If any of these are unclear to an external reader, capture
the ambiguity in a Phase B addendum for Step 5 follow-up.

### F3. Verify the troubleshooting section covers operational failure modes

The four scenarios under "Troubleshooting" should match the
operationally-relevant failure modes:

- Sweep not running (config / restart issues).
- Sweep running but no orphans found (most common; benign).
- Sweep deleting more than expected (false-positive
  classification — the worst case).
- Sweep duration approaching interval (throughput pressure).

Plus a fifth subsection cross-referencing the four
`validate-config` warnings.

### F4. Verify the configuration reference table

```bash
grep "^| " docs/operator/blob-gc-sweep.md | grep -E "PDS_GC_SWEEP_"
```

Expected: six rows, one per field, each carrying:
- field name
- env var (`PDS_GC_SWEEP_*`)
- default
- allowed values
- notes

Six fields verified against
[src/config.rs:177-217](../../src/config.rs#L177-L217)
(GcSweepConfig struct definition): `enabled`,
`interval_secs`, `dry_run`, `max_deletes_per_run`,
`freshness_threshold_secs`, `page_size`.

---

## Section G — Regression triggers

Canonical correctness gates. These `cargo` invocations are
the cycle-wide-baseline confirmations; G1 is the load-bearing
gate.

### G1. Full lib suite

```bash
cargo test --lib
```

Expected: `test result: ok. 991 passed; 0 failed; 0 ignored`.
Step 3 baseline preserved by Step 4 (which is docs + CHANGELOG
only).

### G2. Cross-instance integration tests (Arc 7 baseline)

```bash
cargo test --test distributed_substrate_test
```

Expected: `11 passed; 0 failed; 0 ignored`. **Known flake**:
`cross_instance_first_touch_race_resolves_cleanly` (at
[tests/distributed_substrate_test.rs:603](../../tests/distributed_substrate_test.rs#L603))
can flake under bundle execution due to Postgres
concurrent-first-touch behaviour producing token-remaining
pairs the assertion doesn't accept. Pre-existing; Arc 10
doesn't touch substrate code. v0.6 candidate for assertion-
widening. Workaround: re-run the single test in isolation:

```bash
cargo test --test distributed_substrate_test \
  cross_instance_first_touch_race_resolves_cleanly
```

Should pass on the isolated re-run.

Prerequisite: Docker daemon accessible.

### G3. Contract phrase tests

```bash
cargo test --test contract_phrases
```

Expected: `14 passed; 0 failed; 0 ignored`.

### G4. Grant-admin CLI tests

```bash
cargo test --test grant_admin_test
```

Expected: `8 passed; 0 failed; 0 ignored`.

### G5. Synthetic IN-clause benchmark (re-run of A1)

```bash
cargo test --test blob_in_clause_benchmark -- --ignored
```

Expected: `1 passed; 0 failed`; query under 50ms; plan
unchanged from A1. (A1's `--nocapture` is for inspection;
G5's plain invocation is the regression gate.)

### G6. Clippy zero-error preservation

```bash
cargo clippy --lib --no-deps -- -D warnings
```

Expected: `Finished` with zero errors. Arc 10 introduced no
clippy regressions across all four steps.

---

## Section H — Decoupling sweep

Cycle-narrow check against just the Arc 10 diff:

```bash
git diff --name-only 02bacdd..HEAD | while read f; do
  [ -f "$f" ] && git grep -in "cairn\|hideaway\|pursuingpeace\|nearhorizon" -- "$f"
done
```

`02bacdd` is the parent of Step 1's first commit (`f41c483`),
so this covers all four Arc 10 steps' diffs.

Expected: zero hits. Arc 10's diff introduces no documented
false-positive blocks — neither the CHANGELOG entry, the
operator doc, the source files, nor the Step 1 benchmark
contains forbidden tokens.

The `horizon` token is excluded from the cycle-narrow grep
because the codebase carries English `horizontal/horizontally`
references that pre-date Arc 10.

Cycle-wide `horizon` sweep (matches the Arc 7/8/9 pattern):

```bash
git grep -i "horizon" -- '*.md' '*.rs' | grep -v "horizontal\|more-horizontal" | head -20
```

Expected: only documented false positives — design-doc
self-references listing the forbidden tokens as decoupling-
discipline criteria (`docs/V0*_DESIGN.md`,
`docs/AURORA_ADMIN_UI_DESIGN.md` decoupling-discipline grep
documentation block), prior Phase B scripts at
`docs/internal/arc*-phase-b-commands.md` listing the same
grep commands and their documented false positives, this
file listing the same.

---

## Section I — End-to-end orphan-deletion smoke (optional)

Substantive exercise that requires writing synthetic orphan
files into blob storage and confirming the sweep deletes
them. Skip on Windows-native dev environments where
`touch -d` for backdating mtimes is unavailable.

The sweep's freshness threshold (1h default) means a
fresh-mtime synthetic file classifies as `TooYoung`, not
`ConfirmedOrphan`. Backdating the mtime is mandatory for
the end-to-end test.

### I1. Stop the PDS

Confirm port 2583 is free.

### I2. Locate the blob storage directory

The default disk-backend location is `./data/blobs/` (per
[src/blob_store/mod.rs:122-126](../../src/blob_store/mod.rs#L122-L126));
the sharding scheme is `{base}/{first2chars}/{cid}` (per
[src/blob_store/disk.rs:31-38](../../src/blob_store/disk.rs#L31-L38)).

```bash
ls -la ./data/blobs/ 2>/dev/null
```

If the directory doesn't exist yet, create it:

```bash
mkdir -p ./data/blobs/aa
```

### I3. Write a synthetic orphan with a backdated mtime

```bash
# Write a fake blob whose CID matches no `blob` row.
echo "synthetic orphan content" > ./data/blobs/aa/bafyaaorphan001

# Backdate the mtime past the 1h freshness threshold.
touch -d "2 hours ago" ./data/blobs/aa/bafyaaorphan001

ls -la ./data/blobs/aa/bafyaaorphan001
stat -c '%y' ./data/blobs/aa/bafyaaorphan001 2>/dev/null || stat -f '%Sm' ./data/blobs/aa/bafyaaorphan001
```

Expected: file exists; mtime is ~2 hours in the past.

### I4. Run the CLI sweep in dry-run (default)

```bash
cargo run -- gc-sweep
```

Expected in the report block:

```
GC sweep complete:
  pages scanned:               1
  blobs examined:              1
  authorized:                  0
  in-flight:                   0
  too young:                   0
  confirmed orphans found:     1
  orphans deleted:             0
  ...
```

`confirmed_orphans_found = 1` (the synthetic blob is past
the threshold); `orphans_deleted = 0` (dry-run default).

The file is still in storage:

```bash
ls -la ./data/blobs/aa/bafyaaorphan001
# still present
```

### I5. Run the CLI sweep in destructive mode

The CLI has no `--no-dry-run` flag (per C5). Set the env
var instead:

```bash
PDS_GC_SWEEP_DRY_RUN=false cargo run -- gc-sweep
```

Expected report:

```
  confirmed orphans found:     1
  orphans deleted:             1
```

The file is gone:

```bash
ls -la ./data/blobs/aa/bafyaaorphan001
# No such file or directory
```

### I6. Confirm the metric counters via scheduled mode

For a metric-counter check (optional), enable scheduled
mode with another synthetic orphan and a fast interval:

```bash
echo "second orphan" > ./data/blobs/aa/bafyaaorphan002
touch -d "2 hours ago" ./data/blobs/aa/bafyaaorphan002
export PDS_GC_SWEEP_ENABLED=true
export PDS_GC_SWEEP_INTERVAL_SECS=30
export PDS_GC_SWEEP_DRY_RUN=false  # destructive mode
cargo run -- serve
```

Wait ~35 seconds for the first scheduled sweep. Then in
another terminal:

```bash
curl -s http://localhost:2583/metrics | grep "gc_sweep_orphans"
```

Expected:

```
gc_sweep_orphans_found_total 1
gc_sweep_orphans_deleted_total 1
```

Stop the PDS, unset the env vars.

---

## Notes

- **Token expiry**: JWT lasts ~1 hour; re-mint via
  [`dev.aurora.mintToken`](dev-routes.md#devauroramintToken)
  if a 401 appears mid-sweep. No `createSession` cycle, no
  PDS restart.

- **Off-by-default**: `gc_sweep.enabled = false` is the
  ship default. Existing deployments don't gain a new
  background task silently. Phase B exercises explicitly
  enable scheduled mode via env vars and unset them before
  continuing.

- **Dry-run is the safety direction**: every test of
  scheduled mode runs with `dry_run = true` unless
  explicitly flipped via env var. The CLI `--dry-run` flag
  forces dry-run on; there's no CLI flag to force dry-run
  off (intentional design per Step 3 — destructive mode
  requires explicit config + restart, no CLI-only bypass).

- **`LivenessLock` fail-fast**: CLI offline-only is enforced
  via the lock (per
  [src/cli/gc_sweep.rs:55](../../src/cli/gc_sweep.rs#L55)).
  If a PDS is running, `aurora-locus gc-sweep` errors out
  before doing anything; this is intended behaviour, not a
  regression. Mirrors the `grant-admin` CLI pattern from
  Arc 6 Step 3.

- **Scheduled-mode env-var changes require restart**: the
  `gc_sweep` config block is loaded once at startup
  ([src/config.rs:1049-1059](../../src/config.rs#L1049-L1059)).
  Mid-run env var or config file changes take effect on the
  next PDS restart.

- **Sweep duration vs interval**: production cadence is 24h
  (the default
  [`PDS_GC_SWEEP_INTERVAL_SECS=86400`](../../src/config.rs#L228));
  Phase B uses 30s for testing. Don't run 30s cadence in
  production — it can exceed throughput on large stores
  (this is one of the four `validate-config` warnings).

- **Operator doc cross-references**: Section F's readability
  check confirms the doc reads cleanly. Implementation
  details (stateless mode rationale, classification
  precedence, freshness-threshold race window) stay in the
  design corpus; the operator doc cites "belt-and-braces"
  only where it informs operator action.

- **Known pre-existing flake**: G2's
  `cross_instance_first_touch_race_resolves_cleanly` flakes
  under bundle execution due to Postgres concurrent-first-
  touch behaviour. Not Arc 10's regression; v0.6 candidate
  for assertion-widening. Workaround in G2.

- **No `pages_scanned = 0` end state**: even an empty store
  yields `pages_scanned = 1` because the sweep's loop
  increments `pages_scanned` before the early-break check
  ([src/blob_store/gc.rs:258-264](../../src/blob_store/gc.rs#L258-L264)).
  An operator looking for "the sweep didn't run at all"
  signal should look for the absence of the "GC sweep
  complete" log line or `gc_sweep_duration_seconds_count`
  remaining at 0.

- **Mtime backdating availability**: Section I uses
  `touch -d "2 hours ago"`. This works on Linux and macOS
  (BSD `touch` accepts the same syntax). Windows-native dev
  environments lack the syntax — Section I skips on those.
  A future cycle could add a `filetime` dep for portable
  mtime control in test fixtures (v0.6 candidate per Step 2
  / Step 3 surfaced).

- **"If something looks off"**: same convention as Arc 6-9 —
  document expected vs actual in a Phase B addendum (separate
  file under `docs/internal/`), don't push, drop back to Nova
  for triage. Cycle close depends on a clean Phase B sweep
  for every per-arc Phase B script.

---

## Sign-off

Once all sections clear:

1. Document any findings or regressions in a Phase B addendum
   (separate file under `docs/internal/`).
2. If clean, Arc 10 closes; chainlink #57 can be closed.
3. v0.4 cycle close gate: all per-arc Phase B sweeps
   (including this Arc 10 one) must pass before cycle-close
   release work begins (CHANGELOG cleanup, signed tag, PR to
   doll's branch).
