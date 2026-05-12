# Arc 7 Phase B exercise script

Consolidated verification script for Chrys's Phase B sweep
against an Aurora-Locus deployment running the Arc 7 substrate.
Aggregates the per-step verification matrices from
`/tmp/arc7_step1_report.md` through
`/tmp/arc7_step4_report.md` into a single coherent run-through,
plus the mode-coverage matrix from Step 3's report.

Mirrors the per-arc convention established by
[`arc6-phase-b-script.md`](arc6-phase-b-script.md). Sections A
through J cover the substantive code surfaces; the Setup
section bootstraps the credentials reused throughout.

## Prerequisites

- A Postgres instance reachable by the PDS process. Distributed
  mode is the default and the load-bearing path; SingleInstance
  mode falls back to in-memory state for the two migrated
  surfaces.
- Branch `skydeval/v0.4-cycle` at the Step 4 tip (post-CHANGELOG
  commit) or later.
- Docker daemon access — only required for re-running
  `cargo test --test distributed_substrate_test` per Section J;
  the manual exercises run against a single live PDS.
- `psql` against the same Postgres the PDS uses, for the
  cross-instance simulation exercises and DB inspection.
- `jq` and `curl` for the HTTP-level exercises.

Staged-data exercises (Sections A3, E1-3, F2-3) are called out
inline. Where Arc 7's cross-instance correctness can't be
exercised by a single-PDS Phase B, the script documents the
substrate-level test (`tests/distributed_substrate_test.rs`)
as the canonical proof and uses `psql` to simulate
"instance B" against the shared table.

---

## Setup (one-time per Phase B session)

### Start the PDS in Distributed mode (default)

Distributed mode is the default. No env var needs to be set for
the substrate-on path:

```bash
PDS_HOSTNAME=localhost \
PDS_PORT=2583 \
PDS_DB_BACKEND=postgres \
PDS_DB_URL=postgres://aurora:aurora@localhost:5432/aurora \
cargo run --release -- serve
```

Expected log lines on startup:
- `Distributed-state substrate initialized (Postgres-CAS)`
  with `max_connections=15 min_connections=2`.
- `dpop_jti_replay reaper job started`.
- `rate_limit_buckets reaper job started`.
- `OAuth authorization_request cleanup job started`.
- `🚀 Aurora Locus PDS listening on 0.0.0.0:2583`.

### Verify it's up

```bash
curl -s http://localhost:2583/health | jq
```

Expected: `200 OK` with health-status JSON.

### Create a test account

```bash
curl -s -X POST http://localhost:2583/xrpc/com.atproto.server.createAccount \
  -H 'Content-Type: application/json' \
  -d '{"email":"alice@localhost","handle":"alice.localhost","password":"TestPassword123!"}' \
  | jq
```

Expected: `200 OK` with `did`, `handle`, and `accessJwt`. Save
the DID for later (e.g., `did:plc:alice...`).

### Grant SuperAdmin

```bash
cargo run --release -- grant-admin \
  --did did:plc:alice... \
  --role SuperAdmin \
  --notes "Phase B sweep"
```

Expected: success message; the audit chain entry is written.

### Mint a session JWT

```bash
SESSION=$(curl -s -X POST http://localhost:2583/xrpc/com.atproto.server.createSession \
  -H 'Content-Type: application/json' \
  -d '{"identifier":"alice.localhost","password":"TestPassword123!"}')
export ADMIN_TOKEN=$(echo "$SESSION" | jq -r '.accessJwt')
echo "${ADMIN_TOKEN:0:32}..."
```

Expected: a `eyJ...` prefix (JWT). Token lasts ~1 hour.
Re-mint via `createSession` if mid-sweep.

### Optional: maintenance pool sizing override

The defaults (15 max / 2 min / 10s acquire-timeout) are tuned
for the typical Phase-B scale. To exercise the override path
in Section G3:

```bash
PDS_MAINTENANCE_DB_MAX_CONNECTIONS=5 \
PDS_MAINTENANCE_DB_MIN_CONNECTIONS=1 \
PDS_MAINTENANCE_DB_ACQUIRE_TIMEOUT_SECS=5 \
cargo run --release -- serve
```

(restart required — env vars are read at startup).

---

## Section A — DPoP JTI replay (distributed mode)

### A1. Single-instance baseline: JTI recorded on first sighting

Issue an authenticated DPoP-bound request. The simplest path is
the OAuth token endpoint with a fresh DPoP proof — the existing
`DPopVerifier` exercises the JTI replay path on every
authenticated request.

For Phase B, the easier exercise is to simulate via direct SQL:
the substrate's `dpop_jti_replay` table is the load-bearing
artifact.

```bash
# Insert a JTI (simulating a successful first verification).
psql $PDS_DB_URL -c "
INSERT INTO dpop_jti_replay (jti, jkt, exp_at_epoch_ms, created_at_epoch_ms)
VALUES ('phase-b-jti-1', 'thumb-a',
        extract(epoch from now())::bigint * 1000 + 60000,
        extract(epoch from now())::bigint * 1000);"
```

Confirm insertion:

```bash
psql $PDS_DB_URL -c "
SELECT jti, jkt, exp_at_epoch_ms, created_at_epoch_ms
FROM dpop_jti_replay WHERE jti = 'phase-b-jti-1';"
```

Expected: one row. **Ready to exercise.**

### A2. Replay rejection: same JTI on a second attempt

Re-attempt the same insert via the substrate's `insert` path —
this is what `check_and_record_jti` does internally. Simulate
the second-attempt path:

```bash
psql $PDS_DB_URL -c "
INSERT INTO dpop_jti_replay (jti, jkt, exp_at_epoch_ms, created_at_epoch_ms)
VALUES ('phase-b-jti-1', 'thumb-a',
        extract(epoch from now())::bigint * 1000 + 60000,
        extract(epoch from now())::bigint * 1000);"
```

Expected: Postgres returns
`duplicate key value violates unique constraint
"dpop_jti_replay_pkey"`. SQLSTATE `23505`. This is what the
substrate translates to `DistributedError::KeyExists` →
`check_and_record_jti` returns `Ok(false)` → verifier returns
`Authentication("DPoP proof jti replay or expired")`. **Ready
to exercise.**

### A3. Cross-instance replay simulation

True cross-instance verification requires two PDS processes
against the same Postgres. The substrate-level integration test
(`tests/distributed_substrate_test.rs::cross_instance_dpop_jti_replay_rejection`)
exercises this against testcontainers Postgres. Phase B
simulates by directly inserting "instance A's accept" into the
shared table, then attempting to issue a DPoP proof with that
JTI from "this PDS" (mimicking instance B):

```bash
# "Instance A" accepted JTI 'phase-b-cross-jti'.
psql $PDS_DB_URL -c "
INSERT INTO dpop_jti_replay (jti, jkt, exp_at_epoch_ms, created_at_epoch_ms)
VALUES ('phase-b-cross-jti', 'thumb-instance-a',
        extract(epoch from now())::bigint * 1000 + 60000,
        extract(epoch from now())::bigint * 1000);"
```

Then issue an authenticated request with a DPoP proof whose
JTI equals `phase-b-cross-jti`. The verifier's
`check_and_record_jti` calls `store.insert(...)`, which returns
`KeyExists` (SQLSTATE 23505), and the request is rejected with
`Authentication("DPoP proof jti replay or expired")`.

**Construction cost**: producing a valid DPoP proof with a
chosen JTI is non-trivial (requires ES256 keypair, manual
signing). For Phase B sweep purposes, the substrate-level
correctness is the load-bearing proof; the manual exercise is
satisfied by confirming A2's `KeyExists` translation works
(same code path).

**Documented limitation**: single-PDS Phase B cannot directly
exercise the full cross-instance proof path; the integration
test does. If true HTTP-level cross-instance verification is
required, spin up a second PDS against the same Postgres
following the Setup section.

### A4. Reaper sweep deletes expired rows

Insert an expired JTI:

```bash
psql $PDS_DB_URL -c "
INSERT INTO dpop_jti_replay (jti, jkt, exp_at_epoch_ms, created_at_epoch_ms)
VALUES ('phase-b-stale-jti', 'thumb-stale',
        extract(epoch from now())::bigint * 1000 - 60000,
        extract(epoch from now())::bigint * 1000 - 120000);"
```

The dpop_jti_replay reaper runs every 300s. Either wait, or
manually trigger via SQL to confirm the sweep predicate:

```bash
psql $PDS_DB_URL -c "
DELETE FROM dpop_jti_replay
WHERE exp_at_epoch_ms < extract(epoch from now())::bigint * 1000;"
```

Then confirm:

```bash
psql $PDS_DB_URL -c "
SELECT jti FROM dpop_jti_replay WHERE jti = 'phase-b-stale-jti';"
```

Expected: zero rows. **Ready to exercise** with manual reap.

Confirm the in-process reaper runs by checking
`background_jobs_total` (see Section J for `/metrics` scrape):

```bash
curl -s http://localhost:2583/metrics | grep -E 'background_jobs_total{[^}]*dpop'
```

Expected: a counter present (success or failed). If status is
`success` and count climbs over time, the reaper is alive.

---

## Section B — DPoP JTI replay (single-instance-inmemory mode)

### B1. Restart PDS in single-instance-inmemory mode

```bash
PDS_HOSTNAME=localhost \
PDS_PORT=2583 \
PDS_DB_BACKEND=postgres \
PDS_DB_URL=postgres://aurora:aurora@localhost:5432/aurora \
PDS_DISTRIBUTED_STATE_MODE=single_instance_inmemory \
cargo run --release -- serve
```

Expected log lines on startup:
- `Distributed-state substrate disabled
  (PDS_DISTRIBUTED_STATE_MODE=single_instance_inmemory) —
  auth state lost on restart`.
- NO `Distributed-state substrate initialized` log.
- Reapers (dpop_jti_replay, rate_limit_buckets) still spawn but
  no-op when they tick (the loop's `let Some(store) = ... else
  { continue; };` guard).

### B2. JTI tracked in-memory only

Re-mint the session JWT (token may have expired across
restart).

The in-memory `DPopNonceStore.nonces` HashMap is the JTI tracker
in this mode. Issue an authenticated request; the JTI is added
to the in-memory map; no row appears in `dpop_jti_replay`:

```bash
# Confirm the substrate table is untouched in this mode.
psql $PDS_DB_URL -c "SELECT COUNT(*) FROM dpop_jti_replay;"
```

Expected: zero rows (assuming the table was reset between A4 and
B1, or filter for `created_at_epoch_ms > $RESTART_TIME`).

**Documented limitation**: Phase B can't directly observe the
in-memory HashMap from outside the process; the absence of new
rows in `dpop_jti_replay` is the proof that the in-memory path
is engaged.

### B3. Restart-loss check

In this mode, JTI state is lost across restart (the in-memory
map doesn't persist). This is the documented trade-off — the
operator opts into restart loss for hot-path latency savings.

Test: issue an authenticated request → JTI accepted. Restart
PDS. Issue the SAME request again with the same DPoP proof
(same JTI). Expected outcome: the JTI is accepted again (no
record of the prior acceptance survives the restart).

This is **correct behaviour** for `single_instance_inmemory`
mode and **not a regression**. Operators who can't tolerate
this should use `distributed` mode.

**Ready to exercise** (but the practical observation is
indirect — see Section B2 limitation).

---

## Section C — Rate-limit middleware (distributed mode)

Switch back to default `distributed` mode for this section:

```bash
PDS_HOSTNAME=localhost PDS_PORT=2583 \
PDS_DB_BACKEND=postgres \
PDS_DB_URL=postgres://aurora:aurora@localhost:5432/aurora \
cargo run --release -- serve
```

Re-mint `ADMIN_TOKEN` per Setup.

### C1. First-touch bucket creation

Hit any endpoint behind the rate-limit middleware. The
distributed pre-check runs against a bucket keyed by the path:

```bash
curl -s http://localhost:2583/xrpc/com.atproto.server.describeServer \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  | jq
```

Confirm the bucket appeared:

```bash
psql $PDS_DB_URL -c "
SELECT bucket_key, tokens_remaining, max_tokens, refill_rate, version
FROM rate_limit_buckets
WHERE bucket_key LIKE 'endpoint|%';"
```

Expected: at least one row with `bucket_key =
'endpoint|/xrpc/com.atproto.server.describeServer'`,
`max_tokens = 100`, `refill_rate = 100`, `tokens_remaining = 99`
(or similar — depends on how many requests have already gone
through). `version = 0` for first-touch. **Ready to exercise.**

### C2. Decrement on subsequent requests

Hit the same endpoint several more times:

```bash
for i in 1 2 3 4 5; do
  curl -s http://localhost:2583/xrpc/com.atproto.server.describeServer \
    -H "Authorization: Bearer $ADMIN_TOKEN" > /dev/null
done
```

Re-query:

```bash
psql $PDS_DB_URL -c "
SELECT bucket_key, tokens_remaining, version
FROM rate_limit_buckets
WHERE bucket_key = 'endpoint|/xrpc/com.atproto.server.describeServer';"
```

Expected: `tokens_remaining` has decreased; `version` has
increased by the number of consumes. **Ready to exercise.**

### C3. Refill recovery over time

Wait 2 seconds (the refill rate is 100 tokens/sec; in 2s a
fully-drained bucket should be fully refilled). Hit the
endpoint once:

```bash
sleep 2
curl -s http://localhost:2583/xrpc/com.atproto.server.describeServer \
  -H "Authorization: Bearer $ADMIN_TOKEN" > /dev/null
psql $PDS_DB_URL -c "
SELECT tokens_remaining, window_start_at_epoch_ms
FROM rate_limit_buckets
WHERE bucket_key = 'endpoint|/xrpc/com.atproto.server.describeServer';"
```

Expected: `tokens_remaining = 99` (refilled to max=100, minus
the one request just made), `window_start_at_epoch_ms` updated
to the current epoch-ms. **Ready to exercise.**

### C4. Exhaustion returns 429

Hammer the endpoint until the bucket exhausts. With 100
tokens/sec refill and 100 max_tokens, exhausting requires
sending requests substantially faster than 100/sec:

```bash
for i in $(seq 1 200); do
  curl -s -o /dev/null -w "%{http_code}\n" \
    http://localhost:2583/xrpc/com.atproto.server.describeServer \
    -H "Authorization: Bearer $ADMIN_TOKEN"
done | sort | uniq -c
```

Expected output: a count of `200`s and some `429`s. The
distributed pre-check or the in-process governor (whichever
fires first) returns 429 with `Retry-After: 1`. The 429
response body is `Too Many Requests`. **Ready to exercise.**

### C5. Substrate-consult fall-through (non-fatal)

Per Step 3's design: if the distributed-store consult fails on
the rate-limit hot path, the request continues via the
governor's per-instance defense rather than failing closed.

Simulate by killing the maintenance pool's connections via
`pg_terminate_backend`:

```bash
# Find the PDS's connection PIDs.
psql $PDS_DB_URL -c "
SELECT pid, application_name, client_addr
FROM pg_stat_activity
WHERE datname = current_database()
  AND state = 'idle'
ORDER BY backend_start;"

# Terminate a few (operator's choice; the pool reconnects on next use).
# Then issue a request immediately.
curl -s http://localhost:2583/xrpc/com.atproto.server.describeServer \
  -H "Authorization: Bearer $ADMIN_TOKEN" -o /dev/null -w "%{http_code}\n"
```

Expected: the request returns `200` even with maintenance-pool
disruption. Check the PDS log for a warn-level message
containing
`distributed rate-limit consult failed, falling through to
governor`. The bucket may show stale state until the pool
recovers; subsequent requests resume the substrate path
seamlessly. **Ready to exercise** (advanced — requires
DBA-style intervention).

**Note**: this is a graceful-degradation test, not a stress
test. Sustained fall-through means the substrate is degraded;
brief windows are tolerable.

---

## Section D — Rate-limit middleware (single-instance-inmemory mode)

### D1. Restart PDS in single-instance-inmemory mode

Same restart as Section B1.

After restart, hit an endpoint:

```bash
curl -s http://localhost:2583/xrpc/com.atproto.server.describeServer \
  -H "Authorization: Bearer $ADMIN_TOKEN" > /dev/null
```

Confirm no new rows in `rate_limit_buckets`:

```bash
psql $PDS_DB_URL -c "
SELECT COUNT(*), MAX(window_start_at_epoch_ms)
FROM rate_limit_buckets;"
```

Expected: the COUNT is whatever was there at restart (the table
isn't wiped); the MAX of `window_start_at_epoch_ms` is from
before the restart, NOT advancing on each new request. Rate
limit enforcement comes from the in-process governor only in
this mode. **Ready to exercise.**

---

## Section E — Rate-limit bucket reaper

Switch back to default `distributed` mode.

### E1. Stage an inactive bucket (8 days old)

```bash
psql $PDS_DB_URL -c "
INSERT INTO rate_limit_buckets
  (bucket_key, tokens_remaining, max_tokens, refill_rate,
   window_start_at_epoch_ms, version)
VALUES
  ('endpoint|/xrpc/phase-b-stale-bucket', 0, 100, 100,
   extract(epoch from now())::bigint * 1000 - 8*24*3600*1000, 0);"
```

### E2. Stage an active bucket (1 day old)

```bash
psql $PDS_DB_URL -c "
INSERT INTO rate_limit_buckets
  (bucket_key, tokens_remaining, max_tokens, refill_rate,
   window_start_at_epoch_ms, version)
VALUES
  ('endpoint|/xrpc/phase-b-active-bucket', 50, 100, 100,
   extract(epoch from now())::bigint * 1000 - 1*24*3600*1000, 0);"
```

### E3. Trigger the reaper or wait

The `rate_limit_buckets` reaper runs hourly (`Duration::from_secs(3600)`).
Either wait an hour, or simulate the sweep predicate manually:

```bash
psql $PDS_DB_URL -c "
DELETE FROM rate_limit_buckets
WHERE window_start_at_epoch_ms <
      extract(epoch from now())::bigint * 1000 - 7*24*3600*1000;"
```

Confirm:

```bash
psql $PDS_DB_URL -c "
SELECT bucket_key FROM rate_limit_buckets
WHERE bucket_key LIKE 'endpoint|/xrpc/phase-b-%-bucket';"
```

Expected: only `phase-b-active-bucket` remains; the stale row
is gone. **Ready to exercise** with manual reap simulation.

If you want to confirm the in-process reaper itself runs (not
just the predicate), check the reaper's startup log line at
PDS boot (`rate_limit_buckets reaper job started`) and the
`background_jobs_total{job_type="..."}` counter:

```bash
curl -s http://localhost:2583/metrics \
  | grep -E 'background_jobs_total{.*}'
```

Expected: a counter with `job_type` labels matching the
reapers; sustained success counts confirm the loop is alive.

**Documented workaround**: no CLI exists to fire the reaper
manually; SQL simulation is the practical Phase-B path. If
an out-of-process trigger is desired in v0.6, it would be a
`debug` CLI subcommand similar to the existing
`aurora-locus debug verify-audit-chain`.

---

## Section F — OAuth flow state adapter

### F1. Single-instance OAuth flow happy path

Initiate the authorize endpoint, grant consent, redeem the
code. The OAuth handler routes through the `OAuthFlowStateAdapter`
in any mode (the adapter is always constructed — see Step 2
report).

```bash
# Step 1: authorize endpoint (creates the authorization_request row).
curl -s "http://localhost:2583/oauth/authorize?\
response_type=code&\
client_id=test-client&\
redirect_uri=http://localhost:8080/cb&\
scope=atproto&\
code_challenge=abc123_challenge&\
code_challenge_method=S256&\
state=phase-b-csrf" \
  -i | head -10
```

Expected: a 302 redirect to `/oauth/consent?request_id=<UUID>`.
The substrate's `insert` was invoked under the hood. Capture
the `request_id` from the redirect URL.

Confirm the row exists:

```bash
psql $PDS_DB_URL -c "
SELECT request_id, did, code_used, expires_at
FROM authorization_request
WHERE request_id = '<UUID>';"
```

Expected: one row, `code_used = false`,
`expires_at = created_at + 10min`. **Ready to exercise.**

Grant + redeem are interactive in the consent screen; for Phase
B purposes, confirming the `insert` and the table state after a
direct UPDATE setting `authorization_code` (mimicking grant)
covers the substrate path. The full happy-path flow is
exercised by the integration tests
(`tests/oauth_tests.rs` for the existing tests; Step 2's tests
in `tests/distributed_substrate_test.rs` for the cross-instance
adapter properties).

### F2. Cross-instance simulated read

A row inserted by "instance A" should be visible to
"instance B's" read. Phase B simulates by writing directly via
psql, then attempting to read via the substrate's get path
(through `get_authorization_request`):

```bash
psql $PDS_DB_URL -c "
INSERT INTO authorization_request
  (request_id, did, client_id, code_challenge, code_challenge_method,
   scope, redirect_uri, state, created_at, expires_at, code_used)
VALUES
  ('phase-b-cross-req', 'did:plc:alice', 'cross-instance-client',
   'challenge-cross', 'S256', 'atproto',
   'http://localhost:8080/cb', 'csrf-cross',
   to_char(now() at time zone 'utc', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'),
   to_char((now() + interval '10 minutes') at time zone 'utc',
           'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'),
   false);"
```

Then read via the PDS (e.g., navigate to the consent screen):

```bash
curl -s "http://localhost:2583/oauth/consent?request_id=phase-b-cross-req" \
  -i | head -5
```

Expected: HTML consent screen renders. The substrate's `get`
returned `Some(bytes)` for the request_id even though no PDS
process initiated the row. **Ready to exercise.**

### F3. Cross-instance simulated consume-and-reject-replay

Setup: F2's row in place. Simulate "instance B consumed the
code" via direct UPDATE:

```bash
psql $PDS_DB_URL -c "
UPDATE authorization_request
SET code_used = TRUE, authorization_code = 'ac_phase_b_consumed'
WHERE request_id = 'phase-b-cross-req';"
```

Then attempt to redeem the code via the token endpoint
(simulating an attacker who captured the code mid-flow):

```bash
curl -s -X POST http://localhost:2583/oauth/token \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  -d 'grant_type=authorization_code&\
code=ac_phase_b_consumed&\
code_verifier=arbitrary&\
client_id=cross-instance-client&\
redirect_uri=http://localhost:8080/cb' \
  | jq
```

Expected: `Authentication` error — "Authorization code invalid
or already used" or similar. The substrate's atomic
UPDATE-with-predicate (`WHERE code_used = FALSE`) refuses to
flip the already-consumed row; the consumer translates to a
401-class authentication failure. **Ready to exercise.**

### F4. OAuth state reaper

Stage an expired authorization_request:

```bash
psql $PDS_DB_URL -c "
INSERT INTO authorization_request
  (request_id, did, client_id, code_challenge, code_challenge_method,
   scope, redirect_uri, state, created_at, expires_at, code_used)
VALUES
  ('phase-b-stale-req', 'did:plc:alice', 'client', 'c', 'S256',
   'atproto', 'http://x/cb', 'st',
   to_char((now() - interval '15 minutes') at time zone 'utc',
           'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'),
   to_char((now() - interval '5 minutes') at time zone 'utc',
           'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'),
   false);"
```

Either wait 5 minutes for the OAuth reaper, or simulate:

```bash
psql $PDS_DB_URL -c "
DELETE FROM authorization_request
WHERE expires_at < to_char(now() at time zone 'utc',
                            'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"');"
```

Confirm:

```bash
psql $PDS_DB_URL -c "
SELECT request_id FROM authorization_request
WHERE request_id = 'phase-b-stale-req';"
```

Expected: zero rows. **Ready to exercise** with manual reap.

---

## Section G — Config validation

### G1. `PDS_DISTRIBUTED_STATE_MODE=redis` startup rejection

```bash
PDS_HOSTNAME=localhost PDS_PORT=2583 \
PDS_DB_BACKEND=postgres \
PDS_DB_URL=postgres://aurora:aurora@localhost:5432/aurora \
PDS_DISTRIBUTED_STATE_MODE=redis \
cargo run --release -- serve
```

Expected: PDS fails to start. The error message contains
`PDS_DISTRIBUTED_STATE_MODE=redis is not implemented in v0.4;
use 'distributed' (default) or 'single_instance_inmemory'`.
The validation is at `config.validate()` time; no Postgres
connection is opened before the validation runs. **Ready to
exercise.**

### G2. `PDS_DISTRIBUTED_STATE_MODE=garbage` startup rejection

```bash
PDS_DISTRIBUTED_STATE_MODE=garbage cargo run --release -- serve
```

Expected: PDS fails to start. The error message contains
`PDS_DISTRIBUTED_STATE_MODE must be one of 'distributed',
'single_instance_inmemory', 'redis' (got: "garbage")`.
**Ready to exercise.**

### G3. Maintenance pool sizing env vars accepted

```bash
PDS_MAINTENANCE_DB_MAX_CONNECTIONS=5 \
PDS_MAINTENANCE_DB_MIN_CONNECTIONS=1 \
PDS_MAINTENANCE_DB_ACQUIRE_TIMEOUT_SECS=5 \
cargo run --release -- serve
```

Expected log line:
`Distributed-state substrate initialized (Postgres-CAS)
max_connections=5 min_connections=1`. Confirms the
overrides took effect. **Ready to exercise.**

### G4. Invalid maintenance pool sizing rejected

```bash
PDS_MAINTENANCE_DB_MAX_CONNECTIONS=0 \
cargo run --release -- serve
```

Expected: PDS fails to start with
`PDS_MAINTENANCE_DB_MAX_CONNECTIONS must be greater than 0`.

```bash
PDS_MAINTENANCE_DB_MIN_CONNECTIONS=20 \
PDS_MAINTENANCE_DB_MAX_CONNECTIONS=15 \
cargo run --release -- serve
```

Expected: `PDS_MAINTENANCE_DB_MIN_CONNECTIONS (20) must not
exceed PDS_MAINTENANCE_DB_MAX_CONNECTIONS (15)`. **Ready to
exercise.**

---

## Section H — Migration

### H1. Migration applied check

```bash
psql $PDS_DB_URL -c "
SELECT version, description, success, execution_time
FROM _sqlx_migrations
WHERE version = 7;"
```

Expected: one row with `description LIKE 'distributed state%'`
(or similar — sqlx names from the file prefix),
`success = true`. **Ready to exercise.**

### H2. Tables and indexes present

```bash
psql $PDS_DB_URL -c "\d dpop_jti_replay"
```

Expected: columns `jti TEXT PRIMARY KEY`, `jkt TEXT NOT NULL`,
`exp_at_epoch_ms BIGINT NOT NULL`,
`created_at_epoch_ms BIGINT NOT NULL`. Index
`idx_dpop_jti_replay_exp ON dpop_jti_replay(exp_at_epoch_ms)`
present.

```bash
psql $PDS_DB_URL -c "\d rate_limit_buckets"
```

Expected: columns `bucket_key TEXT PRIMARY KEY`,
`tokens_remaining BIGINT NOT NULL`, `max_tokens BIGINT NOT NULL`,
`refill_rate BIGINT NOT NULL`,
`window_start_at_epoch_ms BIGINT NOT NULL`,
`version BIGINT NOT NULL DEFAULT 0`. Index
`idx_rate_limit_buckets_window ON
rate_limit_buckets(window_start_at_epoch_ms)` present.

**Ready to exercise.**

---

## Section I — Decoupling sweep

### I1. Cycle-wide sweep against Arc 7's diff

```bash
git diff --name-only 0041dbb..HEAD | while read f; do
  [ -f "$f" ] && git grep -in "cairn\|hideaway\|pursuingpeace\|nearhorizon" -- "$f"
done
```

Expected: zero hits. (Step 4 already confirmed; re-run for the
cycle-close audit.)

### I2. Informational "horizon" sweep

```bash
git diff --name-only 0041dbb..HEAD | while read f; do
  [ -f "$f" ] && git grep -in "horizon" -- "$f"
done
```

Expected: one hit — `docs/operator/multi-instance-deployment.md`
contains "horizontally" in a normal English sentence. Documented
false positive per the kickoff list.

### I3. Whole-tree sweep (broader paranoia)

```bash
git grep -i "cairn\|hideaway\|pursuingpeace\|nearhorizon"
```

Expected: false positives only — design-doc self-references in
`docs/V03_DESIGN.md` / `docs/V04_DESIGN.md` / `docs/V02_DESIGN.md`
listing the forbidden tokens as decoupling-discipline criteria.

---

## Section J — Test suite verification

### J1. Library tests

```bash
cargo test --lib
```

Expected: 924 passing, 0 failed, 0 ignored. (Step 3 baseline;
Step 4 added no lib tests so the count is unchanged.)

### J2. Cross-instance substrate integration tests

```bash
cargo test --test distributed_substrate_test
```

Expected: 11 passing, 0 failed (4 substrate + 4 OAuth adapter
+ 3 rate-limit). Requires Docker daemon access for
testcontainers Postgres.

If Docker is unreachable, the tests fail with a clear
`Failed to start Postgres container — is Docker accessible?`
message; that's a test-prerequisite issue, not a regression.

### J3. Lib-only clippy

```bash
cargo clippy --lib --no-deps
```

Expected: 24 lib-wide warnings unchanged from Step 3 baseline.
Zero warnings on Arc 7 code (`src/distributed/*`,
`src/oauth/flow_state_adapter.rs`,
`src/federation/dpop.rs`, `src/rate_limit.rs`,
`src/jobs/mod.rs`, `src/context.rs`). The 24 pre-existing
warnings are unrelated codebase-wide patterns (doc-overindented
lists, unused methods in unrelated modules).

### J4. All-targets clippy (informational)

```bash
cargo clippy --all-targets --no-deps
```

Expected: ~52 warnings, primarily "never used" in the
`bin "aurora-locus" test` target for substrate primitives that
the bin target doesn't import. Lib target uses them via the
test consumers. Not actionable — artifact of the bin/lib split.

---

## Notes

- **PDS restart sequence**: mode toggles
  (`PDS_DISTRIBUTED_STATE_MODE`) require restart — env vars are
  read at `AppContext::new` startup, not runtime. Same for
  maintenance pool sizing.

- **Token expiry**: session JWT lasts ~1 hour. If `ADMIN_TOKEN`
  expires mid-sweep, re-mint via Setup's createSession step.
  Restarts also invalidate previously-issued tokens in
  `single_instance_inmemory` mode (session table is DB-backed;
  tokens survive, but the running PDS's session manager state
  is fresh).

- **DB inspection**: `psql $PDS_DB_URL` against the same
  Postgres the PDS uses, for every cross-instance simulation
  exercise. The substrate tables (`dpop_jti_replay`,
  `rate_limit_buckets`) live alongside the application tables
  in `account_db` regardless of substrate mode.

- **Reaper-trigger workaround**: no CLI subcommand exists in
  v0.4 to fire a specific reaper manually. The canonical
  approach is either wait for the natural cadence (5 min for
  DPoP JTI / OAuth state, 1 hour for rate-limit buckets) or
  simulate the sweep predicate via `psql` DELETE statements as
  shown above. A `debug trigger-reaper` CLI subcommand similar
  to `aurora-locus debug verify-audit-chain` would be a v0.6
  candidate if Phase-B-driven manual sweep ergonomics matter.

- **Single-PDS cross-instance simulation**: where Sections A3,
  F2-3 require simulating "instance B", the script uses direct
  `psql` writes against the shared table to mimic the second
  instance's view. True multi-instance verification — two
  Aurora-Locus processes pointing at the same Postgres —
  requires spinning up a second PDS following the Setup
  section. The cross-instance correctness invariants are
  validated by the substrate-level integration tests in
  [`tests/distributed_substrate_test.rs`](../../tests/distributed_substrate_test.rs)
  (11 tests against real Postgres via testcontainers); the
  manual Phase-B exercises are operator-confidence smoke
  tests, not the proof of correctness.

- **Substrate-consult fall-through interpretation**: brief
  warn-log windows from the rate-limit middleware are
  acceptable (Postgres hiccup, maintenance-pool brief
  saturation). Sustained fall-through indicates the substrate
  is degraded — check Postgres health, maintenance-pool
  saturation (`pg_stat_activity`), and the connection-budget
  math in [`docs/operator/multi-instance-deployment.md`](../operator/multi-instance-deployment.md).

- **"If something looks off" failure routing**: same as the
  Arc 6 convention. Document expected vs actual outcome in
  a Phase B addendum (separate from this script); don't push
  the addendum or any fix-up commits — drop back to Nova for
  triage. Cycle close depends on a clean Phase B sweep.

---

## Sign-off

Once all sections clear:

1. Document any findings or regressions in a Phase B addendum
   (separate file under `docs/internal/`).
2. If clean, Arc 7 closes; chainlink #53 can be closed.
3. v0.4 cycle close gate: all arc Phase B sweeps must pass
   before the cycle-close release work begins (Chrys's call).
