# Multi-instance Aurora-Locus deployment

Operator guide for running Aurora-Locus across two or more
instances sharing a single Postgres backend. Introduced in
v0.4 / Arc 7 (chainlink #53; design at
[`docs/V04_DESIGN.md`](../V04_DESIGN.md) §6).

## Overview

"Multi-instance" in this document means **two or more
Aurora-Locus binary processes**, each running the standard
HTTP server, behind a load balancer, sharing one Postgres
database and one blob store. Each instance handles arbitrary
requests; routing is the load balancer's responsibility.

Prior to v0.4, Aurora-Locus could share Postgres and a blob
store across instances (those primitives have been in place
since v0.2), but three pieces of in-process state were
per-instance:

- **DPoP JTI replay tracking** — a client's authentication
  proof, identified by its JTI claim, was accepted-or-rejected
  based on what *one* instance had seen, not the deployment.
  A replay arriving on a different instance from the one that
  saw the original would be accepted again.
- **Rate-limit token buckets** — each instance ran its own
  in-memory counters. A client hitting two instances
  round-robin saw their effective rate doubled.
- (OAuth flow state was already DB-backed even before Arc 7.
  Step 0 reconnaissance corrected the original design's
  framing on that point.)

Arc 7 introduces the **`DistributedStore` substrate** — a
trait-shaped abstraction backed by Postgres-CAS that makes
both of the above cross-instance-coherent. Operators get to
choose between the substrate (cross-instance correct,
slightly higher hot-path latency) and the pre-Arc-7 in-memory
behaviour (lower latency, single-instance only).

### When multi-instance makes sense

- **High availability**: surviving an instance crash without
  user-visible downtime requires ≥2 instances. The load
  balancer health-checks the failed instance out; the
  other(s) keep serving.
- **Vertical-scale ceiling reached**: a single instance's
  CPU / memory / file-descriptor budget is the limit. Adding
  instances scales request handling horizontally.
- **Geographic redundancy**: instances in multiple
  availability zones (sharing one Postgres + blob store)
  reduce blast radius for regional outages.

### When it doesn't make sense

- **Small deployments** (one operator, one PDS, a hundred
  users): the operational complexity of two instances —
  monitoring, log aggregation, coordinated upgrades —
  outweighs the availability benefit. Run one instance,
  back up the database, restore on failure.
- **Constrained operator tooling**: multi-instance assumes
  Postgres as the backend. Operators on SQLite see a
  warning at startup if they enable `distributed` mode
  (the substrate works but offers no multi-instance
  benefit when there's only one SQLite file).
- **Strict-SLA seamless-upgrade requirements**: Aurora-Locus
  does **not** support seamless rolling upgrades; the
  documented model is coordinated full-deployment restart
  with brief downtime. See "Operational reality" below.

## Prerequisites

- **Postgres backend** (required for any multi-instance
  Aurora-Locus deployment; v0.2 introduced this and
  Arc 7 extends its use).
- **Connection budget**: the Postgres `max_connections`
  server-side limit must accommodate
  `(main_pool + maintenance_pool + 2) × instance_count`,
  plus headroom for operator tooling (psql, monitoring,
  backups). Worked example for 4 instances at defaults:
  `(25 + 15 + 2) × 4 = 168` Postgres connections, plus
  ~20 for tooling → provision `max_connections ≥ 200`.
- **Shared blob storage** — either a shared filesystem
  (NFS, EFS) or object storage (S3-compatible). Pre-Arc-7
  requirement; not new.
- **Synchronised system clocks** across instances. The
  substrate uses each instance's wall-clock for refill
  arithmetic and lease expiry; meaningful drift produces
  unfair rate-limit behaviour. NTP-disciplined hosts are
  sufficient.

## Configuration

### `PDS_DISTRIBUTED_STATE_MODE`

The headline operator knob. Three values:

| Value | Behaviour |
|---|---|
| **`distributed`** *(default)* | Substrate enabled. OAuth state, DPoP JTI replay, rate-limit buckets all cross-instance-coherent via the maintenance-pool-backed Postgres tables. |
| `single_instance_inmemory` | Substrate skipped. DPoP JTI replay + rate-limit buckets live in process memory; OAuth state is still DB-backed (the pre-Arc-7 behaviour). Lower latency on the auth hot path; **lost on restart** for the in-memory surfaces. |
| `redis` | **Forward-compat slot only — not implemented in v0.4.** Setting this fails fast at startup with `PdsError::Validation`. Reserved so a future cycle can ship a Redis backend without re-shaping the operator config surface. |

The default is the right choice for both deployment shapes:

- **Multi-instance**: `distributed` is the only mode that
  makes the cross-instance correctness story work.
- **Single-instance**: `distributed` works too, at the cost
  of a few extra Postgres round-trips per authenticated
  request. Most operators see this as a fair trade for the
  durability win (DPoP JTI replay state and rate-limit
  buckets survive restarts). Single-instance operators who
  explicitly prefer the pre-Arc-7 latency profile and accept
  restart loss opt into `single_instance_inmemory`.

### Maintenance pool sizing

The substrate uses a **dedicated maintenance pool**, separate
from the main application pool, so distributed-state
round-trips can't starve regular request handling under load.
Three env vars control its sizing:

| Env var | Default | Purpose |
|---|---|---|
| `PDS_MAINTENANCE_DB_MAX_CONNECTIONS` | `15` | Upper bound on concurrent maintenance-pool connections. Tune up for high authenticated-QPS deployments; tune down for resource-constrained ones. |
| `PDS_MAINTENANCE_DB_MIN_CONNECTIONS` | `2` | Lower bound; keeps a small warm pool to avoid connect latency on quiet bursts. |
| `PDS_MAINTENANCE_DB_ACQUIRE_TIMEOUT_SECS` | `10` | How long a hot-path request waits for a free maintenance-pool connection before giving up. **Deliberately tighter than the main pool's 30s default** — DPoP / rate-limit are hot-path-sensitive; failing fast and degrading gracefully is better than blocking a request thread for half a minute. |

The defaults are tuned for typical deployments under
~1000 authenticated requests-per-second per instance. The
maintenance pool's hot consumers are:

1. DPoP JTI replay tracking — one INSERT per
   DPoP-bound authenticated request.
2. Distributed rate-limit pre-check — one UPDATE (or
   sometimes UPDATE + SELECT + INSERT) per rate-limited
   request.
3. Reaper sweeps — one DELETE per minute, low traffic.

If your deployment exceeds ~1000 req/s/instance, raise
`MAX_CONNECTIONS` proportionally; if the maintenance pool
saturates under load, requests fall through to the per-
instance defense (governor-side rate-limit + in-memory JTI
state in the corresponding mode).

### Configuration env-var inventory

| Env var | Used by | Notes |
|---|---|---|
| `PDS_DISTRIBUTED_STATE_MODE` | `AppContext::new` | Mode selector (see above). |
| `PDS_MAINTENANCE_DB_MAX_CONNECTIONS` | `AppContext::new` | See above. |
| `PDS_MAINTENANCE_DB_MIN_CONNECTIONS` | `AppContext::new` | See above. |
| `PDS_MAINTENANCE_DB_ACQUIRE_TIMEOUT_SECS` | `AppContext::new` | See above. |
| `PDS_DB_BACKEND` | Existing (v0.2). | Must be `postgres` for genuine multi-instance benefit. Arc 7 emits a startup warning if `distributed` mode is paired with `sqlite`. |
| `PDS_DB_URL` | Existing (v0.2). | Required for `postgres` backend. |
| `PDS_DB_MAX_CONNECTIONS` | Existing (v0.2). | Main application pool. Default 25. |

## Migration path

### Upgrading a single-instance v0.3 deployment to v0.4

1. **Stop the v0.3 instance.**
2. **Deploy the v0.4 binary.** On first start, the
   `sqlx::migrate!` macro auto-applies migration
   `0007_distributed_state.sql`, creating two new tables
   (`dpop_jti_replay` and `rate_limit_buckets`). No data
   migration runs; existing tables (including
   `authorization_request`) are untouched.
3. **Start the v0.4 binary.** Default
   `PDS_DISTRIBUTED_STATE_MODE=distributed`. The
   maintenance pool is constructed, reapers spawn,
   substrate is wired into the auth hot path.
4. *(Optional)* If you want the pre-Arc-7 latency profile
   on a still-single-instance deployment, set
   `PDS_DISTRIBUTED_STATE_MODE=single_instance_inmemory`
   and restart.

Total operator-visible downtime for the v0.3 → v0.4
upgrade: the same as any v0.3 binary swap — ~30 seconds
for typical deployments.

### Scaling a v0.4 deployment from one instance to many

1. **Provision additional instances** pointing at the same
   Postgres and the same blob store. Each instance gets its
   own host / container; they don't communicate with each
   other directly — coordination happens through Postgres.
2. **Confirm `PDS_DISTRIBUTED_STATE_MODE=distributed`** on
   every instance. Mixed-mode deployments (one instance in
   `single_instance_inmemory`, others in `distributed`)
   produce inconsistent rate-limit and DPoP-replay behaviour
   and are unsupported.
3. **Set the load balancer** to route to all instances.
   Health-check each instance on `/health` (existing
   pre-Arc-7 endpoint).
4. **Verify cross-instance behaviour** with the smoke
   checks listed under "Verification" below.

No schema migration runs when scaling up — the tables are
already in place from step 2 of the v0.3 → v0.4 upgrade.

### Verification smoke checks (multi-instance)

Run these from a client machine that can reach the load
balancer:

- **OAuth flow continuity**: initiate the OAuth flow against
  the load balancer. The authorization endpoint may land on
  instance A; the token endpoint may land on instance B.
  Confirm completion succeeds. (This was already true
  pre-Arc-7 since the OAuth flow state was DB-backed; it's
  retained as a regression check.)
- **DPoP replay rejection across instances**: capture an
  authenticated request's DPoP proof; replay it twice
  through the load balancer. The first request succeeds;
  the second returns the existing "DPoP proof jti replay or
  expired" error (`Authentication` variant), regardless of
  which instance saw the first request.
- **Rate-limit consistency**: send requests at a rate above
  the configured per-endpoint limit. Confirm 429 responses
  start arriving after the bucket is exhausted, and that
  rerouting to a sibling instance doesn't reset the bucket.

## Operational reality: rolling upgrade is full restart

**Aurora-Locus does not support seamless rolling upgrades.**
The documented upgrade model for multi-instance deployments
is a coordinated full restart:

1. **Drain traffic** from all instances (load balancer
   health-check fail, traffic redirect, or graceful
   shutdown signal).
2. **Stop all instances.**
3. **Apply the new binary**, including any new
   `sqlx::migrate!` migrations that ship with the
   release.
4. **Start all instances** on the new binary.
5. **Restore traffic.**

Total operator-visible downtime for typical deployments:
~30 seconds. Operators with strict-SLA seamless-upgrade
requirements should evaluate whether Aurora-Locus is the
right fit for their use case.

### Why not rolling

Schema migrations occasionally introduce cross-instance
incompatibilities mid-rollout: a new column the new code
reads but old code doesn't write; an UPDATE pattern the new
code emits but old code doesn't recognize. Multi-phase
rolling-upgrade strategies (expand-and-contract migrations,
feature flags, compatibility shims) make these problems
solvable but at substantial complexity cost — every
schema-touching change becomes a multi-cycle coordination
project.

Aurora-Locus picks the simpler model. A 30-second
coordinated restart is the documented cost; it's bounded,
operator-visible, and doesn't compound across cycles.

## Monitoring

Arc 7's distributed-state substrate plugs into Aurora-Locus's
existing Prometheus + tracing instrumentation; no new
Arc-7-specific Prometheus metric families were added in
v0.4. The existing metrics that DO reflect substrate
behaviour:

| Metric | What it tells you |
|---|---|
| `background_jobs_total{job_type,status}` | Reaper sweep run counts. Arc 7's reapers report under `job_type` values matching the in-job tracing labels (`dpop_jti_replay`, `oauth_flow_state`, `rate_limit_buckets`); the existing `cleanup_*` jobs report under their own names. Alert on `status="failed"` count climbing. |
| `background_job_duration_seconds{job_type}` | Histogram of reaper sweep duration. Sustained elevation indicates table growth outpacing the sweep budget. |
| `background_jobs_active` | Gauge of in-flight background jobs. |
| `db_query_duration_seconds{operation,table}` | Per-query Postgres latency. The substrate's operations show up under the relevant `table` value (`dpop_jti_replay`, `rate_limit_buckets`, `authorization_request`). Histogram p99 climbing past ~50ms indicates either Postgres saturation or maintenance-pool exhaustion. |

For deeper diagnostics, the substrate emits **structured
tracing logs** (not Prometheus metrics) on its operational
seams:

- `tracing::warn!` on substrate-consult failures from the
  rate-limit middleware (the fall-through path described
  below). Search log aggregations for
  `distributed rate-limit consult failed, falling through to
  governor`.
- `tracing::warn!` on reaper sweep failures with the failing
  table and error.
- `tracing::info!` on reaper sweeps that swept ≥1 row, with
  the table and count.

**v0.6 roadmap**: dedicated Prometheus metric families for
substrate operations
(`aurora_distributed_store_operations_total`,
`aurora_distributed_store_latency_seconds`,
`rate_limit_substrate_fallthrough_total`) are on the v0.6
candidate accumulator. v0.4 ships with the tracing-side
observability and the existing DB-query / background-job
metrics; if your deployment needs more granular surfaces,
you can build them on top of the existing labels or wait
for v0.6.

### Substrate-consult fall-through

A deliberate Step-3 design decision: **if the distributed-store
consult fails on the rate-limit hot path** (Postgres
hiccup, maintenance-pool saturation, transient network
error), the request continues via the existing in-process
governor rather than failing closed with a 503. The
fall-through is non-fatal:

- Operators see degraded cross-instance protection in the
  warn-log stream and (eventually, post-v0.6) in dedicated
  metrics.
- Users see continued service.

This trades a brief window of per-instance-only rate limit
enforcement for availability. The window closes as soon as
Postgres is healthy again; nothing persists about the
fall-through after the moment it happens.

If your deployment requires fail-closed behaviour on
substrate failure, the underlying middleware can be patched
to invert the decision; doing so is a one-line change in
`src/rate_limit.rs:rate_limit_middleware`. Open an issue or
follow up with the maintainers if this is a deployment
requirement.

## Known limitations (v0.4)

- **Distributed rate-limit defaults are hardcoded**: 100
  tokens at 100 tokens/sec for the per-endpoint pre-check.
  The existing governor's `EndpointRateLimitConfig::bluesky_defaults`
  per-endpoint multi-limit configuration is unchanged and
  remains active in series with the distributed check.
  Per-endpoint configurability for the distributed path is
  a v0.6 candidate.
- **`rate_limit_buckets` retention is hardcoded at 7 days**.
  The reaper sweeps buckets whose `window_start_at_epoch_ms`
  hasn't moved in a week. Configurable threshold is a v0.6
  candidate.
- **DPoP server-side nonce issuance stays in-memory**. The
  `/xrpc/com.atproto.federation.getDpopNonce` endpoint
  issues §8 nonces; those are *not* migrated to the
  substrate in v0.4. The substrate's DPoP scope is JTI
  replay only (RFC 9449 §11.1). The `dpop_jti_replay`
  table name reflects this honestly.
- **No DPoP parse-result cache** in v0.4. The `TtlCache`
  primitive (`src/distributed/cache.rs`) is in place from
  Step 1, but no consumer wires through it; Step 3 deferred
  shipping pending profiling that demonstrates parse-step
  latency is a real bottleneck. v0.6 candidate.
- **Redis backend slot reserved but not implemented**.
  `PDS_DISTRIBUTED_STATE_MODE=redis` fails fast at startup.
  No deployment work-around in v0.4; future cycles may add
  a Redis backend against the same trait surface.
- **Manual hot-path smoke testing is the operator's
  responsibility**. The Arc 7 cycle validated correctness
  via 924 lib unit tests + 11 cross-instance integration
  tests against real Postgres (testcontainers). End-to-end
  HTTP-level smoke tests across two `axum::serve` instances
  were not built; the substrate-level tests cover the
  cross-instance correctness invariants but don't exercise
  the full handler wire.
- **No dedicated Arc-7 Prometheus metrics**. See "Monitoring"
  above. v0.6 candidate.

## Performance characteristics

The Arc 7 cycle did not perform formal benchmarks; the
numbers below are **engineering estimates against healthy
infrastructure**, not measured deployment data:

- **DPoP verification** (with substrate consult): adds one
  Postgres INSERT per authenticated request. Sub-millisecond
  on primary-key-indexed `dpop_jti_replay` against a healthy
  Postgres on the same host or LAN; ~5-10ms over slower
  network. The verification's existing cryptographic work
  (JWK parsing, ES256 signature verification) is unchanged.
- **Rate-limit middleware** (with distributed pre-check):
  adds one Postgres UPDATE per request to a rate-limited
  endpoint. Similar latency profile to DPoP — single primary-
  key-indexed write.
- **First-touch bucket cost**: one extra SELECT + one
  INSERT for the first request against a bucket key the
  substrate hasn't seen recently (or for the first request
  after the reaper swept the bucket). Subsequent requests
  hit the UPDATE-only happy path.
- **Reaper sweeps**: hourly DELETE on `rate_limit_buckets`,
  every 5 minutes on `dpop_jti_replay` and
  `authorization_request`. Bounded by table size, which is
  in turn bounded by active set size. For deployments with
  ~10k concurrent active clients, expect a few-hundred-row
  deletions per sweep, all completing in milliseconds.

If your deployment's profile differs substantially from
these estimates and you measure latencies that look wrong,
the most likely causes are:

1. **Maintenance pool saturation**: raise
   `PDS_MAINTENANCE_DB_MAX_CONNECTIONS`.
2. **Postgres `max_connections` exhaustion**: raise the
   server-side limit (see "Prerequisites" above).
3. **Postgres replication lag** if you're using read
   replicas: the substrate writes to the primary; reads
   should not be routed to replicas.

## Troubleshooting

### "Why is my deployment slow after upgrading to v0.4?"

Likely the maintenance pool is saturated or
`PDS_MAINTENANCE_DB_MAX_CONNECTIONS` is set too low for
your traffic. Check:

1. Postgres `pg_stat_activity` for connection-count
   pressure.
2. Substrate-consult fall-through warnings in the log
   stream.
3. The application's main pool's saturation (separate from
   the maintenance pool — the two are isolated by design).

### "Why are users seeing replay rejection?"

Expected behaviour on a *real* replay (client sends the
same DPoP proof twice, often via retry-on-network-error
client logic that didn't realize the first request
succeeded). Check the JTI in the warn log; if it's the
same value across two requests close in time, the client
is the issue. If you see different JTIs being rejected,
look for clock skew between the client and server (JTIs
have an `exp` claim; past-exp proofs are rejected).

### "Rate limits feel tighter than v0.3"

Expected when running in `distributed` mode for the
first time. Pre-Arc-7 each instance ran its own rate-limit
counter; if you had 4 instances, a client effectively saw
4× the rate budget. Arc 7's distributed pre-check
aggregates across instances — the deployment-wide limit
is what the bucket configuration says it is.

For single-instance deployments where you preferred the
pre-Arc-7 latency profile, set
`PDS_DISTRIBUTED_STATE_MODE=single_instance_inmemory` to
fall back to per-instance limits.

### "I see `distributed rate-limit consult failed, falling
through to governor` in logs"

The substrate consult timed out or errored; the request
continued via the per-instance governor. Brief windows of
this are tolerable; sustained fall-through indicates the
substrate is degraded:

1. Check maintenance-pool saturation.
2. Check Postgres health.
3. Confirm `PDS_DB_BACKEND=postgres` if you expected
   distributed behaviour (SQLite + `distributed` mode
   produces a startup warning and degraded behaviour).

If fall-through persists despite Postgres being healthy,
open an issue with logs and metrics — this would indicate
a substrate-side bug.

### "How do I confirm the substrate is actually working?"

The shortest end-to-end test: with two instances behind a
load balancer, capture a real DPoP-authenticated request,
replay it; the second request should be rejected with the
existing "DPoP proof jti replay or expired" error regardless
of which instance the original request hit. If the second
request succeeds, the distributed substrate isn't taking
effect — check mode configuration first.

## References

- Design doc: [`docs/V04_DESIGN.md`](../V04_DESIGN.md) §6
  for the full Arc 7 design including friction-risk
  analysis (§6.5) and verification criteria (§6.6).
- Migration file:
  [`migrations/0007_distributed_state.sql`](../../migrations/0007_distributed_state.sql)
  + Postgres twin.
- Substrate code: [`src/distributed/`](../../src/distributed/)
  for the trait surface, registry, and backend
  implementations.
- Issue tracker: chainlink #53 (Arc 7 — Multi-instance
  auth state + rate limiting).
