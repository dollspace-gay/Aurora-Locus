# Arc 7 Phase B exercise script

Localhost smoke-test script for skydeval's Phase B sweep of Arc 7
(`chainlink #53` — Multi-instance auth state + rate limiting).
Mirrors the Arc 6 convention at
[`arc6-phase-b-script.md`](arc6-phase-b-script.md): curl
against `localhost:2583`, `sqlite3 data/account.sqlite` for DB
inspection, PDS restarts via env-var toggle, no deployment
framing.

## Prerequisites

- Working directory: the Aurora-Locus checkout (WSL path on
  skydeval's environment).
- Branch `skydeval/v0.4-cycle` at the Arc 7 Phase B tip or
  later.
- A free port 2583 (default `PDS_PORT`).
- `curl` and `jq` on the dev machine.
- `sqlite3` on the dev machine (the default backend is SQLite at
  `data/account.sqlite`).

### SQLite vs Postgres caveat

Aurora-Locus's local-dev default backend is **SQLite**. Arc 7's
distributed-state substrate is designed for **Postgres
multi-instance** deployments, but the SQL it uses is portable
via `sqlx::Any` — the substrate tables exist and the trait
operations behave identically on SQLite. Phase B exercises the
substrate's single-instance behaviour (atomic inserts,
version-based CAS, reaper sweeps) against the local SQLite
file.

**Cross-instance correctness** (the load-bearing reason
Arc 7 exists) was verified during Steps 1-3 by `cargo test
--test distributed_substrate_test` running against
testcontainers Postgres. That covers the
JTI-accepted-on-A-rejected-on-B / bucket-exhausted-on-A-
visible-on-B properties. Localhost Phase B doesn't re-prove
those — it confirms the substrate operations work end-to-end
against a real PDS handling real curl traffic. Section J
re-runs the integration tests if you want fresh
cross-instance proof.

If you want to exercise Arc 7 against local Postgres
specifically, set `PDS_DB_BACKEND=postgres` and
`PDS_DB_URL=postgres://...` before `cargo run --bin aurora-locus -- serve`. All
the curl and sqlite3 commands below assume the default
SQLite path; swap `sqlite3 data/account.sqlite` for
`psql $PDS_DB_URL` if running against Postgres.

---

## Setup (one-time per Phase B session)

### Start the PDS in default mode

Default mode is `Distributed` (Step 1's config wiring landed
this). The substrate is constructed; the JTI replay reaper,
OAuth state cleanup, and rate-limit bucket reaper jobs all
spawn at startup.

```bash
cargo run --bin aurora-locus --release -- serve
```

Expected log lines on startup:

```
Distributed-state substrate initialized (Postgres-CAS) max_connections=15 min_connections=2
dpop_jti_replay reaper job started
OAuth authorization_request cleanup job started
rate_limit_buckets reaper job started
🚀 Aurora Locus PDS listening on 0.0.0.0:2583
```

### Verify it's up

```bash
curl -s http://localhost:2583/health | jq
```

Expected: `{"status":"ok",...}`.

### Create a test account

```bash
curl -s -X POST http://localhost:2583/xrpc/com.atproto.server.createAccount \
  -H 'Content-Type: application/json' \
  -d '{"email":"alice@localhost","handle":"alice.localhost","password":"TestPassword123!"}' \
  | jq
```

Expected: 200 with `did`, `handle`, `accessJwt`. Save the DID
(format `did:plc:...`) for later if skydeval's existing local
state doesn't already have one.

### Grant SuperAdmin

(Skip if skydeval's local state already has a SuperAdmin from
prior Phase B sessions.)

```bash
cargo run --bin aurora-locus --release -- grant-admin \
  --did did:plc:<from-above> \
  --role SuperAdmin \
  --notes "Phase B sweep"
```

Expected: success message; audit chain entry written.

### Mint the session JWT

```bash
SESSION=$(curl -s -X POST http://localhost:2583/xrpc/com.atproto.server.createSession \
  -H 'Content-Type: application/json' \
  -d '{"identifier":"alice.localhost","password":"TestPassword123!"}')
export ADMIN_TOKEN=$(echo "$SESSION" | jq -r '.accessJwt')
echo "Token prefix: ${ADMIN_TOKEN:0:32}..."
```

Expected: an `eyJ...` prefix. Token lasts ~1 hour; re-mint via
the same command if it expires mid-sweep.

---

## Section A — DPoP JTI replay (distributed mode)

DPoP JTI replay tracking is a per-authenticated-request path
that requires a DPoP keypair + signed proof on each request.
Exercising the full HTTP-level path via curl requires
scripting the DPoP ceremony (P-256 keypair, JWT signing,
PKCE), which is impractical for a Phase B smoke test.

Phase B instead exercises the **substrate operations directly
via SQL**, matching exactly what
`DPopNonceStore.check_and_record_jti` does internally in
`Distributed` mode. The full HTTP-level path is covered by
`src/federation/dpop.rs`'s 21 inline tests + the
substrate-level integration test
`tests/distributed_substrate_test.rs::cross_instance_dpop_jti_replay_rejection`.

### A1. Substrate INSERT records a JTI

Simulate "instance accepted a JTI" via the same INSERT shape
the substrate uses:

```bash
sqlite3 data/account.sqlite "
INSERT INTO dpop_jti_replay (jti, jkt, exp_at_epoch_ms, created_at_epoch_ms)
VALUES (
  'phase-b-jti-1',
  'thumb-a',
  CAST(strftime('%s','now') AS INTEGER) * 1000 + 60000,
  CAST(strftime('%s','now') AS INTEGER) * 1000
);
"
```

Confirm:

```bash
sqlite3 data/account.sqlite "
SELECT jti, jkt FROM dpop_jti_replay WHERE jti = 'phase-b-jti-1';
"
```

Expected: `phase-b-jti-1|thumb-a`.

### A2. Replay rejection: duplicate INSERT fails

Re-run A1's INSERT. The `dpop_jti_replay` table's TEXT PRIMARY
KEY on `jti` rejects the duplicate; SQLite returns
`SQLITE_CONSTRAINT_PRIMARYKEY` (extended code 1555).

```bash
sqlite3 data/account.sqlite "
INSERT INTO dpop_jti_replay (jti, jkt, exp_at_epoch_ms, created_at_epoch_ms)
VALUES (
  'phase-b-jti-1',
  'thumb-a',
  CAST(strftime('%s','now') AS INTEGER) * 1000 + 60000,
  CAST(strftime('%s','now') AS INTEGER) * 1000
);
"
```

Expected: `Runtime error: UNIQUE constraint failed:
dpop_jti_replay.jti`. This is what the substrate translates to
`DistributedError::KeyExists`, which `check_and_record_jti`
maps to `Ok(false)`, which the verifier maps to
`Authentication("DPoP proof jti replay or expired")`.

### A3. Reaper sweeps expired rows

Stage an expired row (exp_at_epoch_ms in the past):

```bash
sqlite3 data/account.sqlite "
INSERT INTO dpop_jti_replay (jti, jkt, exp_at_epoch_ms, created_at_epoch_ms)
VALUES (
  'phase-b-stale-jti',
  'thumb-stale',
  CAST(strftime('%s','now') AS INTEGER) * 1000 - 60000,
  CAST(strftime('%s','now') AS INTEGER) * 1000 - 120000
);
"
```

The reaper runs every 300s. Either wait, or simulate the
predicate directly to confirm the SQL shape:

```bash
sqlite3 data/account.sqlite "
DELETE FROM dpop_jti_replay
WHERE exp_at_epoch_ms < CAST(strftime('%s','now') AS INTEGER) * 1000;
"
```

Confirm:

```bash
sqlite3 data/account.sqlite "
SELECT jti FROM dpop_jti_replay WHERE jti = 'phase-b-stale-jti';
"
```

Expected: zero rows. The active row from A1 also went if its
`exp_at_epoch_ms` was past `now`; that's correct sweep behaviour.

### Cross-instance verification

Single-PDS localhost can't exercise cross-instance JTI
rejection (no sibling instance to reject from). The
substrate-level integration test
`cross_instance_dpop_jti_replay_rejection` against
testcontainers Postgres covers that property; re-run via
Section J's J2 if you want fresh proof.

---

## Section B — DPoP JTI replay (single_instance_inmemory mode)

In this mode, JTI state lives in `DPopNonceStore.nonces`
(in-memory `HashMap`). No substrate table writes.

### B1. Restart PDS in single_instance_inmemory mode

Stop the running PDS (`Ctrl-C` or `kill`).

```bash
PDS_DISTRIBUTED_STATE_MODE=single_instance_inmemory \
cargo run --bin aurora-locus --release -- serve
```

Expected log line on startup:

```
Distributed-state substrate disabled (PDS_DISTRIBUTED_STATE_MODE=single_instance_inmemory) — auth state lost on restart
```

NO `Distributed-state substrate initialized` log. The reaper
jobs still spawn (loop guards skip the sweep when
`distributed_store` is `None`).

Re-mint `$ADMIN_TOKEN` via Setup's createSession command —
the prior token is still valid (session table is DB-backed),
but exporting again is cheap.

### B2. In-memory path leaves the substrate table untouched

Snapshot the row count before any auth traffic:

```bash
sqlite3 data/account.sqlite "
SELECT COUNT(*) FROM dpop_jti_replay;
" > /tmp/jti_before.txt
cat /tmp/jti_before.txt
```

Issue an authenticated request:

```bash
curl -s http://localhost:2583/xrpc/com.atproto.server.describeServer \
  -H "Authorization: Bearer $ADMIN_TOKEN" > /dev/null
```

Confirm the count is unchanged:

```bash
sqlite3 data/account.sqlite "
SELECT COUNT(*) FROM dpop_jti_replay;
" > /tmp/jti_after.txt
diff /tmp/jti_before.txt /tmp/jti_after.txt && echo "unchanged"
```

Expected: `unchanged`. The in-memory `HashMap` is the JTI
tracker in this mode; no rows written to the substrate
table.

### B3. In-memory replay rejection

Replay rejection still works in this mode — it just lives in
process memory. From the same PDS process (without restart),
the in-memory map catches duplicate JTIs. The full HTTP-level
exercise has the same DPoP-ceremony complexity as Section A;
the inline unit tests
`test_check_and_record_jti_replay_rejected` and
`test_check_and_record_jti_already_expired_rejected` in
`src/federation/dpop.rs` cover this.

### B4. Restart loses in-memory state

Stop the PDS. Confirm `dpop_jti_replay`'s row count is still
whatever it was before B1 (nothing flushed to the table on
shutdown).

Restart again in the same in-memory mode. Any JTI accepted
before the restart would now be accepted again — there's no
persistent record. This is the documented trade-off of
`single_instance_inmemory` mode and **not a regression**.

### Restart back to default mode

Stop the PDS. Restart in default `Distributed` mode for the
rest of the script:

```bash
cargo run --bin aurora-locus --release -- serve
```

Re-mint `$ADMIN_TOKEN`.

---

## Section C — Rate-limit middleware (distributed mode)

The distributed rate-limit pre-check (Step 3) runs in the
middleware before the existing in-process governor. Each
authenticated request hits a bucket keyed by the endpoint
path. The full curl+SQL exercise covers C1-C4 cleanly; C5
(substrate-consult fall-through) is skipped — see C5 below.

### C1. First-touch bucket creation

Pick an endpoint and hit it once:

```bash
curl -s http://localhost:2583/xrpc/com.atproto.server.describeServer \
  -H "Authorization: Bearer $ADMIN_TOKEN" > /dev/null
```

Confirm the bucket appeared with first-touch state:

```bash
sqlite3 data/account.sqlite "
SELECT bucket_key, tokens_remaining, max_tokens, refill_rate, version
FROM rate_limit_buckets
WHERE bucket_key = 'endpoint|/xrpc/com.atproto.server.describeServer';
"
```

Expected: one row with `tokens_remaining=99` (max=100 minus
cost=1), `max_tokens=100`, `refill_rate=100`, `version=0`.

### C2. Subsequent requests decrement tokens_remaining

Hit the same endpoint several more times:

```bash
for i in 1 2 3 4 5; do
  curl -s http://localhost:2583/xrpc/com.atproto.server.describeServer \
    -H "Authorization: Bearer $ADMIN_TOKEN" > /dev/null
done
```

Re-query:

```bash
sqlite3 data/account.sqlite "
SELECT tokens_remaining, version
FROM rate_limit_buckets
WHERE bucket_key = 'endpoint|/xrpc/com.atproto.server.describeServer';
"
```

Expected: `tokens_remaining` lower than after C1 (`99 - 5 + refill`
within the elapsed time — at 100 tokens/sec, the count drops by
roughly the number of requests minus the refill); `version`
incremented by the number of UPDATEs.

### C3. Refill recovers capacity

Wait 2 seconds (100 tokens/sec × 2s = 200 tokens of refill,
capped at max=100 so the bucket fully refills):

```bash
sleep 2
curl -s http://localhost:2583/xrpc/com.atproto.server.describeServer \
  -H "Authorization: Bearer $ADMIN_TOKEN" > /dev/null
sqlite3 data/account.sqlite "
SELECT tokens_remaining FROM rate_limit_buckets
WHERE bucket_key = 'endpoint|/xrpc/com.atproto.server.describeServer';
"
```

Expected: `99` (refilled to max=100, minus the one request just
made). `window_start_at_epoch_ms` updated to current time.

### C4. Exhaustion returns 429

Hammer the endpoint above the refill rate:

```bash
for i in $(seq 1 300); do
  curl -s -o /dev/null -w "%{http_code}\n" \
    http://localhost:2583/xrpc/com.atproto.server.describeServer \
    -H "Authorization: Bearer $ADMIN_TOKEN"
done | sort | uniq -c
```

Expected: a mix of `200` and `429` responses. The 429 comes with
`Retry-After: 1` and body `Too Many Requests`. The 200/429 split
depends on machine speed — slower hosts see more 200s (refill
keeps up); fast hosts see more 429s.

### C5. Substrate-consult fall-through — skipped

Per Step 3's design, if the distributed-store consult fails on
the rate-limit hot path, the request falls through to the
governor's per-instance defense (logged as `warn`, not 503'd).
Simulating Postgres unavailability mid-request against a local
SQLite-backed PDS isn't practical — the substrate IS the
SQLite file the application uses for everything else; "killing"
it means stopping the PDS entirely.

The non-fatal fall-through behaviour is verified by the unit
tests in `src/rate_limit.rs::distributed_tests` + the
substrate-level integration tests in
`tests/distributed_substrate_test.rs`. **Skip C5 for localhost
Phase B**; this is intentional.

---

## Section D — Rate-limit middleware (single_instance_inmemory mode)

### D1. Restart in single-instance mode; governor-only path

Stop the PDS. Restart:

```bash
PDS_DISTRIBUTED_STATE_MODE=single_instance_inmemory \
cargo run --bin aurora-locus --release -- serve
```

Re-mint `$ADMIN_TOKEN` if needed.

Snapshot the rate_limit_buckets count:

```bash
sqlite3 data/account.sqlite "
SELECT COUNT(*) FROM rate_limit_buckets;
" > /tmp/buckets_before.txt
cat /tmp/buckets_before.txt
```

Hit a few endpoints:

```bash
for i in 1 2 3 4 5; do
  curl -s http://localhost:2583/xrpc/com.atproto.server.describeServer \
    -H "Authorization: Bearer $ADMIN_TOKEN" > /dev/null
done
```

Confirm no new rows:

```bash
sqlite3 data/account.sqlite "
SELECT COUNT(*) FROM rate_limit_buckets;
" > /tmp/buckets_after.txt
diff /tmp/buckets_before.txt /tmp/buckets_after.txt && echo "unchanged"
```

Expected: `unchanged`. The governor handles rate limiting
in-process; no substrate-table writes.

### Restart back to default mode

Stop the PDS. Restart in default `Distributed` mode:

```bash
cargo run --bin aurora-locus --release -- serve
```

---

## Section E — Rate-limit bucket reaper

The reaper sweeps inactive buckets (window_start_at_epoch_ms
older than 7 days) every hour. Phase B simulates the
predicate directly rather than waiting an hour.

### E1. Stage an inactive bucket

```bash
sqlite3 data/account.sqlite "
INSERT INTO rate_limit_buckets
  (bucket_key, tokens_remaining, max_tokens, refill_rate,
   window_start_at_epoch_ms, version)
VALUES (
  'endpoint|/xrpc/phase-b-stale-bucket',
  0, 100, 100,
  CAST(strftime('%s','now') AS INTEGER) * 1000 - 8 * 24 * 3600 * 1000,
  0
);
"
```

### E2. Stage an active bucket

```bash
sqlite3 data/account.sqlite "
INSERT INTO rate_limit_buckets
  (bucket_key, tokens_remaining, max_tokens, refill_rate,
   window_start_at_epoch_ms, version)
VALUES (
  'endpoint|/xrpc/phase-b-active-bucket',
  50, 100, 100,
  CAST(strftime('%s','now') AS INTEGER) * 1000 - 1 * 24 * 3600 * 1000,
  0
);
"
```

### E3. Simulate the reaper sweep

Either wait an hour (Phase B time isn't free) or run the
predicate directly:

```bash
sqlite3 data/account.sqlite "
DELETE FROM rate_limit_buckets
WHERE window_start_at_epoch_ms <
      CAST(strftime('%s','now') AS INTEGER) * 1000 - 7 * 24 * 3600 * 1000;
"
```

Confirm:

```bash
sqlite3 data/account.sqlite "
SELECT bucket_key FROM rate_limit_buckets
WHERE bucket_key LIKE 'endpoint|/xrpc/phase-b-%-bucket';
"
```

Expected: only `phase-b-active-bucket` remains. The stale
8-days-ago row is gone.

To confirm the actual in-process reaper is alive (rather than
just the SQL predicate working), check the
`background_jobs_total` Prometheus counter:

```bash
curl -s http://localhost:2583/metrics | grep -E 'background_jobs_total{' | head -10
```

Expected: counter entries with `job_type` labels matching the
running reapers; sustained `status="success"` counts confirm
the loops are alive.

---

## Section F — OAuth flow state adapter

The `OAuthFlowStateAdapter` (Step 2) wraps the existing
`authorization_request` table; it's always constructed, in
either mode.

### F1. Initiate authorize endpoint, confirm row appears

```bash
curl -s -i "http://localhost:2583/oauth/authorize?\
response_type=code&\
client_id=test-client&\
redirect_uri=http://localhost:8080/cb&\
scope=atproto&\
code_challenge=abc123_challenge&\
code_challenge_method=S256&\
state=phase-b-csrf" \
  | head -10
```

Expected: a `302 Found` redirect to
`/oauth/consent?request_id=<UUID>`. Capture the `request_id`
value.

Confirm the row:

```bash
sqlite3 data/account.sqlite "
SELECT request_id, did, client_id, code_used, expires_at
FROM authorization_request
WHERE request_id = '<paste-request-id-here>';
"
```

Expected: one row with `code_used=0` (false), `expires_at`
about 10 minutes ahead of now. The substrate's
`OAuthFlowStateAdapter::insert` is what wrote it.

### F2. Sweeper is wired

The `cleanup_expired_requests` sweeper was wired into the
JobScheduler in Step 1 (Step 0 Q1 finding: pre-existing
sweeper existed but was unwired). Step 2 converted it to
route through the trait's `reap_expired("oauth_flow_state",
_)`. No exercise needed beyond confirming the startup log
mentioned `OAuth authorization_request cleanup job started`
(see Setup).

### F3. Stage an expired row; simulate sweep

```bash
sqlite3 data/account.sqlite "
INSERT INTO authorization_request
  (request_id, did, client_id, code_challenge, code_challenge_method,
   scope, redirect_uri, state, created_at, expires_at, code_used)
VALUES (
  'phase-b-stale-oauth',
  'did:plc:alice',
  'test-client',
  'c', 'S256',
  'atproto',
  'http://localhost:8080/cb',
  'st',
  strftime('%Y-%m-%dT%H:%M:%S.000Z', datetime('now', '-15 minutes')),
  strftime('%Y-%m-%dT%H:%M:%S.000Z', datetime('now', '-5 minutes')),
  0
);
"
```

Simulate the OAuth state reaper (every 300s; or just trigger
the predicate):

```bash
sqlite3 data/account.sqlite "
DELETE FROM authorization_request
WHERE expires_at < strftime('%Y-%m-%dT%H:%M:%S.000Z', 'now');
"
```

Confirm:

```bash
sqlite3 data/account.sqlite "
SELECT request_id FROM authorization_request
WHERE request_id = 'phase-b-stale-oauth';
"
```

Expected: zero rows.

### F4. Full OAuth happy path — out of Phase B scope

The full happy path (consent screen click-through, PKCE
verifier, DPoP keypair signing, token redemption) is
interactive in the browser and would require multi-page
scripting. Phase B confirms the substrate write path is
engaged via F1; the full HTTP-level OAuth flow lives in
`tests/oauth_tests.rs` and the new substrate-level
cross-instance tests
(`cross_instance_oauth_state_visible_to_siblings` etc).

---

## Section G — Config validation

### G1. `PDS_DISTRIBUTED_STATE_MODE=redis` startup rejection

Stop the PDS. Restart:

```bash
PDS_DISTRIBUTED_STATE_MODE=redis cargo run --bin aurora-locus --release -- serve
```

Expected: startup fails with an error message containing
`PDS_DISTRIBUTED_STATE_MODE=redis is not implemented in v0.4;
use 'distributed' (default) or 'single_instance_inmemory'`.
The PDS exits non-zero before any Postgres connection
attempt — validation runs at `config.validate()` time.

### G2. `PDS_DISTRIBUTED_STATE_MODE=garbage` startup rejection

```bash
PDS_DISTRIBUTED_STATE_MODE=garbage cargo run --bin aurora-locus --release -- serve
```

Expected: startup fails with
`PDS_DISTRIBUTED_STATE_MODE must be one of 'distributed',
'single_instance_inmemory', 'redis' (got: "garbage")`.

### G3. Custom maintenance pool sizing accepted

```bash
PDS_MAINTENANCE_DB_MAX_CONNECTIONS=5 \
PDS_MAINTENANCE_DB_MIN_CONNECTIONS=1 \
PDS_MAINTENANCE_DB_ACQUIRE_TIMEOUT_SECS=5 \
cargo run --bin aurora-locus --release -- serve
```

Expected log line:

```
Distributed-state substrate initialized (Postgres-CAS) max_connections=5 min_connections=1
```

The PDS starts cleanly with the overridden sizing. Stop the
PDS after observing the log.

### G4. Invalid maintenance pool sizing rejected

```bash
PDS_MAINTENANCE_DB_MAX_CONNECTIONS=0 \
cargo run --bin aurora-locus --release -- serve
```

Expected:
`PDS_MAINTENANCE_DB_MAX_CONNECTIONS must be greater than 0`.

```bash
PDS_MAINTENANCE_DB_MIN_CONNECTIONS=20 \
PDS_MAINTENANCE_DB_MAX_CONNECTIONS=15 \
cargo run --bin aurora-locus --release -- serve
```

Expected:
`PDS_MAINTENANCE_DB_MIN_CONNECTIONS (20) must not exceed
PDS_MAINTENANCE_DB_MAX_CONNECTIONS (15)`.

### Restart back to default mode

```bash
cargo run --bin aurora-locus --release -- serve
```

Re-mint `$ADMIN_TOKEN`.

---

## Section H — Migration verification

### H1. `0007_distributed_state` applied

```bash
sqlite3 data/account.sqlite "
SELECT version, description, success
FROM _sqlx_migrations
WHERE version >= 7
ORDER BY version;
"
```

Expected: row(s) for version 7+ with `success=1`. The
`description` for version 7 is derived from the migration
filename (`distributed_state` or similar).

### H2. Substrate table schema matches Step 0.6

```bash
sqlite3 data/account.sqlite ".schema dpop_jti_replay"
```

Expected:

```
CREATE TABLE dpop_jti_replay (
    jti                     TEXT PRIMARY KEY,
    jkt                     TEXT NOT NULL,
    exp_at_epoch_ms         BIGINT NOT NULL,
    created_at_epoch_ms     BIGINT NOT NULL
);
CREATE INDEX idx_dpop_jti_replay_exp
    ON dpop_jti_replay(exp_at_epoch_ms);
```

```bash
sqlite3 data/account.sqlite ".schema rate_limit_buckets"
```

Expected:

```
CREATE TABLE rate_limit_buckets (
    bucket_key                  TEXT PRIMARY KEY,
    tokens_remaining            BIGINT NOT NULL,
    max_tokens                  BIGINT NOT NULL,
    refill_rate                 BIGINT NOT NULL,
    window_start_at_epoch_ms    BIGINT NOT NULL,
    version                     BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX idx_rate_limit_buckets_window
    ON rate_limit_buckets(window_start_at_epoch_ms);
```

---

## Section I — Decoupling sweep

```bash
git grep -i "cairn\|hideaway\|horizon\|pursuingpeace\|nearhorizon"
```

Expected hits: only documented false positives —

- English `horizontal` / `horizontally` / `horizons` in normal
  prose (e.g., the new operator doc at
  `docs/operator/multi-instance-deployment.md` uses
  "horizontally" once).
- Design-doc self-references listing the forbidden tokens as
  decoupling-discipline criteria (`docs/V03_DESIGN.md`,
  `docs/V04_DESIGN.md`, `docs/V02_DESIGN.md` if present).
- Lucide icon names containing `more-horizontal` (admin UI).

Cycle-narrow check against just the Arc 7 diff:

```bash
git diff --name-only 0041dbb..HEAD | while read f; do
  [ -f "$f" ] && git grep -in "cairn\|hideaway\|pursuingpeace\|nearhorizon" -- "$f"
done
```

Expected: zero hits across all 35 Arc-7-touched files.

---

## Section J — Test suite verification

### J1. Lib tests

```bash
cargo test --lib
```

Expected: `test result: ok. 924 passed; 0 failed; 0 ignored`.

### J2. Cross-instance integration tests (testcontainers Postgres)

```bash
cargo test --test distributed_substrate_test
```

Expected: `test result: ok. 11 passed; 0 failed; 0 ignored`.

Prerequisite: Docker daemon accessible to the test runner.
If Docker isn't running, the tests fail with a clear
`Failed to start Postgres container — is Docker accessible?`
panic; that's a prerequisite issue, not a regression.

### J3. Lib clippy

```bash
cargo clippy --lib --no-deps
```

Expected: ~24 lib-wide warnings (pre-existing patterns
unrelated to Arc 7), zero new warnings on Arc 7 code:
`src/distributed/*`, `src/oauth/flow_state_adapter.rs`,
`src/federation/dpop.rs`, `src/rate_limit.rs`,
`src/jobs/mod.rs`, `src/context.rs`.

---

## Notes

- **Token expiry**: session JWT lasts ~1 hour. Re-mint via the
  Setup section's `createSession` curl if `$ADMIN_TOKEN`
  starts producing 401s mid-sweep. Each mode-toggle restart
  also produces a window where the old token might briefly
  fail revalidation; re-minting is the safest reflex.

- **Restart sequence for mode toggles**: env vars
  (`PDS_DISTRIBUTED_STATE_MODE`,
  `PDS_MAINTENANCE_DB_MAX_CONNECTIONS`, etc) are read at
  `AppContext::new` startup. Each mode-toggle exercise
  documents the restart inline. Order: stop with `Ctrl-C` or
  `kill`, set env vars, `cargo run --bin aurora-locus --release -- serve` (the
  `--release` flag is optional but recommended for cleaner
  log output).

- **DB inspection**: `sqlite3 data/account.sqlite` for all
  queries. The substrate tables (`dpop_jti_replay`,
  `rate_limit_buckets`) live alongside the OAuth + admin
  tables in the same SQLite file by default. If running
  against local Postgres instead, swap for `psql $PDS_DB_URL`
  and adjust SQL syntax where needed (e.g., epoch-ms
  arithmetic, `INSERT OR IGNORE` is SQLite-specific —
  Postgres uses `ON CONFLICT DO NOTHING`).

- **Cross-instance behaviour**: not exercisable via single-PDS
  Phase B. The substrate-level integration tests
  (`tests/distributed_substrate_test.rs`) cover
  cross-instance JTI replay rejection (4 substrate + 4 OAuth
  + 3 rate-limit = 11 tests against testcontainers
  Postgres). Section J's J2 re-runs them.

- **C5 (substrate-consult fall-through) skipped**: simulating
  Postgres unavailability against a SQLite-backed local PDS
  isn't practical — the substrate IS the same file the
  application uses for everything else. The non-fatal
  fall-through behaviour is verified by the unit + integration
  tests; skip for localhost Phase B.

- **Reaper-trigger workaround**: no CLI subcommand exists in
  v0.4 to fire a specific reaper manually. SQL `DELETE`
  matching the reaper's predicate is the canonical Phase B
  simulation (Sections A3, E3, F3). Waiting the full reaper
  cadence (300s for DPoP JTI / OAuth state, 1h for rate-limit
  buckets) is the alternative but rarely used during Phase B.

- **DPoP-bound auth ceremony**: the full HTTP-level DPoP-bound
  authenticated-request path requires a P-256 keypair, JWT
  signing, and PKCE — too much for a Phase B smoke test.
  Sections A and B use SQL-based substrate simulation
  exclusively. The full ceremony is exercised by
  `src/federation/dpop.rs`'s 21 inline tests + the
  substrate-level integration test
  `cross_instance_dpop_jti_replay_rejection`.

- **"If something looks off"**: same convention as Arc 6 —
  document expected vs actual in a Phase B addendum
  (separate file under `docs/internal/`), don't push the
  addendum or any fix-up commits, drop back to Nova for
  triage. Cycle close depends on a clean Phase B sweep.

---

## Sign-off

Once all sections clear:

1. Document any findings or regressions in a Phase B addendum
   (separate file under `docs/internal/`).
2. If clean, Arc 7 closes; chainlink #53 can be closed.
3. v0.4 cycle close gate: all per-arc Phase B sweeps must
   pass before the cycle-close release work begins.
