# Postgres Phase 4: Multi-Instance Aurora-Locus

**Status:** design-locked, no production code yet
**Tracks:** chainlink #88 (this doc), #89 (leader election), #90 (LISTEN/NOTIFY), #91 (integration tests)
**Reference:** [POSTGRES_BACKEND_ASSESSMENT.md §6 Phase 4](../POSTGRES_BACKEND_ASSESSMENT.md)

---

## 1. Goals

Make Aurora-Locus deployable as multiple instances against one Postgres
backend without:

- **Sequencer races** — only one instance writes the firehose stream at a
  time. Two writers would interleave events and break the `seq`
  monotonic-increasing contract that consumers rely on.
- **Stale per-process caches** — when one instance mutates state that
  another instance has cached in-process, the cached entry must be
  invalidated cross-instance, not just locally.

Phase 4 is what turns "Aurora-Locus runs on Postgres" (Phases 1–3 done)
into "Aurora-Locus runs as horizontally-scalable infrastructure on
Postgres."

## 2. Non-goals

- **No Postgres-level replication.** Phase 4 assumes a single Postgres
  endpoint (or a managed cluster fronted as a single endpoint). HA at
  the Postgres layer is out of scope; that's the operator's choice of
  managed Postgres / streaming replication setup.
- **No consensus protocols.** No Raft, no Paxos, no gossip. Postgres
  advisory locks and LISTEN/NOTIFY are the coordination primitives.
- **No cross-region coordination.** Latency and partition handling
  beyond what Postgres natively provides are operator concerns.
- **No distributed rate limiting.** The `distributed_rate_limiter`
  hook in [src/rate_limit.rs](../src/rate_limit.rs) is unwired today;
  wiring it is a separate workstream (likely Postgres Phase 5 or its
  own follow-up). Phase 4 does *not* attempt to distribute rate-limit
  counters via NOTIFY.
- **No Redis-backed state migration.** OAuth in-flight state and DPoP
  nonces are explicitly per-process today (see §6.2); making them
  multi-instance-safe is its own design decision deferred past v0.2.

## 3. Sequencer leader election

### 3.1 Mechanism — `pg_try_advisory_lock`

On startup, the sequencer attempts to acquire a single int64-keyed
session-level advisory lock via `pg_try_advisory_lock(key)`. The lock
holder is the *leader*; non-holders are *standbys*. Only the leader
writes new firehose events.

Postgres advisory locks are session-scoped: when the holding
connection terminates (graceful close, network drop, server-side
timeout), the lock is released automatically. This gives us free
failure detection — no application-level heartbeat needed for v0.2.

Standbys still serve read traffic and accept writes that don't
generate firehose events; they just don't advance the `repo_seq`
table. A writer that gets a request requiring firehose emission while
the local instance is a standby returns 503 Service Unavailable
("sequencer not local; retry"). Operators run a load balancer in
front of multiple instances; 503 retries land on the leader on the
next attempt.

> **Open question — see §7.1.** Whether to forward firehose-write
> requests to the leader internally vs. returning 503. The 503-and-retry
> path is simpler and matches stateless-LB ergonomics; forwarding adds
> instance-to-instance plumbing.

### 3.2 Standby retry interval

Standbys re-attempt `pg_try_advisory_lock` every 2 seconds (default).
Configurable via env var `AURORA_SEQUENCER_LEADER_RETRY_MS` (default
`2000`). Bounds: minimum `500ms` (no faster than that to avoid lock
churn under contention), maximum `30000ms` (no slower than that to
keep failover latency bounded for the operator).

### 3.3 Failure modes and recovery

- **Connection drop** → lock auto-releases; the dropping instance is
  no longer leader. The standbys pick up on their next 2s retry tick.
  The drop side reconnects, then enters the standby retry loop just
  like a fresh process.
- **Slow but alive leader** → not detected by Postgres alone. v0.2
  accepts this as a known limitation: a leader that's stuck
  (deadlocked, GC pause, etc.) but still holding its TCP connection
  will block standbys indefinitely. Adding an application-level
  heartbeat is a future consideration; flagged in §7.2.
- **Network partition** → Postgres terminates the leader's connection
  via TCP keepalive eventually (default ~7200s on Linux, but Postgres
  can be tuned with `tcp_keepalives_idle`/`_interval`/`_count`).
  Operators tune Postgres for the partition-detection latency they
  want; the application doesn't need to know.
- **Postgres restart** → all advisory locks are released. All
  standbys race to acquire on next retry tick. One wins. The others
  remain standby.

### 3.4 Lock key derivation

The advisory lock key is the first 8 bytes of `SHA-256("aurora-locus.
sequencer.leader")` interpreted as a signed int64 (Postgres advisory
locks accept `bigint`). Hashing the human-readable identifier:

- Avoids collisions with other applications using advisory locks on
  the same Postgres database.
- Survives schema-namespace changes (the key isn't tied to a database
  identifier).
- Is reproducible — two instances of the same Aurora-Locus build
  derive the same key without coordination.

Hardcoded constant in code; not configurable. If two Aurora-Locus
deployments need to share a Postgres database (uncommon, but possible
in development), the operator separates them by Postgres database or
schema, not by lock key.

### 3.5 Startup and shutdown flow

```text
startup:
  loop:
    if pg_try_advisory_lock(key):
      role = Leader
      sequencer.activate_writer()
      break
    else:
      role = Standby
      sleep(retry_interval)
      continue

  on connection_drop:
    role = Standby
    sequencer.deactivate_writer()
    reconnect()
    goto startup loop

shutdown (graceful):
  if role == Leader:
    sequencer.deactivate_writer()
    pg_advisory_unlock(key)
  close_pool()
```

`activate_writer` / `deactivate_writer` are the existing
[Sequencer](../src/sequencer/sequencer.rs) hooks for enabling/disabling
event writes; they already exist for pause/resume admin endpoints, so
Phase 4.2 reuses them.

## 4. Cache invalidation via LISTEN/NOTIFY

### 4.1 Mechanism

Postgres LISTEN/NOTIFY: the writing instance issues `NOTIFY <channel>,
'<payload>'` after the modifying transaction commits; listening
instances asynchronously receive the payload on a long-lived
LISTEN connection and invalidate matching local cache entries.

### 4.2 Channel name

Single channel: `aurora_cache_invalidate`.

One channel rather than per-cache-type channels because:

- Subscriber count is small (one per process per channel; multiple
  channels just multiplies LISTEN connections).
- Payload-based dispatch is cheap (JSON parse + match on `type`).
- Simpler operator mental model — one channel, one `pg_notify` site.

### 4.3 Payload schema

```json
{ "type": "<cache-type>", "key": "<key>" }
```

`type` is a short string identifying which cache to invalidate. `key`
is the cache key (format depends on `type`).

Currently a single `type` is defined: `"local_records:<did>"` invalidates
the per-DID entries in `LocalRecordsCache` (see §4.7).

The schema is intentionally extensible: new cache types added in
future phases (DPoP nonces, OAuth state, distributed rate-limit
counters, etc.) can use the same channel and payload shape without
schema-versioning the channel itself. Receivers ignore `type` values
they don't recognize, so old code coexists with new senders.

### 4.4 Write-site instrumentation

`NOTIFY` calls happen *after* the modifying SQL transaction commits.
Two reasons:

- **Avoid invalidating before the new data is visible.** If A NOTIFYs
  inside its transaction and B receives + re-reads before A commits,
  B re-caches stale data from A's pre-commit view.
- **Avoid double-invalidation under rollback.** A NOTIFY inside a
  transaction that later rolls back would still fire (NOTIFY is
  buffered until commit in Postgres; it does NOT fire on rollback,
  but only because Postgres specifically buffers it — emitting via
  a separate connection would lose this guarantee).

Concretely in Aurora-Locus: write sites that mutate per-DID repo state
emit one NOTIFY per affected DID after the existing commit. The
specific call sites (in [src/api/repo.rs](../src/api/repo.rs) and
[src/account/manager.rs](../src/account/manager.rs)) are identified
in Phase 4.3 implementation work.

NOTIFY only fires when the backend is Postgres. SQLite-backed
deployments are inherently single-instance and don't need cross-
process invalidation.

### 4.5 Listener — dedicated connection

Each process opens one dedicated long-lived Postgres connection,
issues `LISTEN aurora_cache_invalidate`, and processes notifications
in a Tokio task that loops on `connection.notifications().recv()`.

The connection is *not* drawn from the main `AnyPool` because:

- Pool connections cycle, and a `LISTEN` on a connection that's
  returned to the pool stops delivering notifications.
- LISTEN connections are long-idle by design; mixing them with the
  pool perturbs pool sizing heuristics.

Connection drop is handled by reconnect: the listener task catches
the connection error, sleeps a backoff (1s, 2s, 4s, capped at 30s),
reconnects, re-issues LISTEN, resumes the recv loop. Notifications
emitted during the disconnected window are lost; the TTL-fallback in
each invalidatable cache covers this case (see §4.6).

### 4.6 Connection drop recovery

- **During disconnect**, no NOTIFYs are received. Caches may serve
  stale data for the duration of the disconnect.
- **Recovery is automatic** via the listener reconnect loop.
- **TTL fallback** ensures eventual consistency: every invalidatable
  cache also has a TTL (LocalRecordsCache: 5 seconds), so stale
  entries expire even if a NOTIFY was missed during disconnect. The
  tradeoff: up to one TTL window of staleness in the worst case (a
  NOTIFY missed exactly when the cache entry was fresh enough to be
  served). For LocalRecordsCache's 5s TTL this is acceptable; see
  §6.3 if longer-TTL caches are added in future phases.

### 4.7 Cache types requiring invalidation — verified inventory

Audit conducted Phase 4.1 across `src/`. Six cache-shaped pieces of
state were found; **only one is purely in-memory and requires
cross-instance NOTIFY invalidation**.

| Cache | Location | Storage | Cross-instance NOTIFY? |
|---|---|---|---|
| `LocalRecordsCache` | [src/read_after_write/cache.rs](../src/read_after_write/cache.rs) | In-memory Moka, 5s TTL, LRU 10k | **Yes** |
| `DidCache` (DID docs + handle mappings) | [src/identity/cache.rs](../src/identity/cache.rs) | Postgres `did_doc` / `did_handle` tables | No — Postgres is the SoT, no per-process layer |
| `RateLimiter` request counts | [src/rate_limit.rs](../src/rate_limit.rs) | In-memory `governor` state | Out of scope — distributed rate limiting is a separate workstream |
| `OAuthStateStore` | [src/api/oauth_admin.rs](../src/api/oauth_admin.rs) | In-memory HashMap, removed on callback | Out of scope — explicitly noted as single-instance limitation |
| `DPopNonceStore` | [src/federation/dpop.rs](../src/federation/dpop.rs) | In-memory HashMap, 5min TTL | Out of scope — DPoP integration not enabled (`#[allow(dead_code)]`) |
| `NonceStore`, `PdsDiscovery`, federated-search circuit breakers, `EmailRateLimiter` | various | Per-instance in-memory | No — semantics don't require cross-instance consistency |

**Why DidCache doesn't need NOTIFY** — the cache is a thin SQL wrapper
around `did_doc` and `did_handle` tables. With a Postgres backend, all
instances share the same tables; per-process cache state doesn't
exist. The TTL semantics live in the SQL columns (`fresh_until`,
`expires_at`). Stale-while-revalidate works without coordination.

**Why distributed rate-limiting is deferred** — the `governor` library
keeps state in-process. Distributing it correctly requires either a
distributed counter store (Redis) or a probabilistic algorithm
(token-bucket-with-Postgres-CAS, etc.). Both are larger pieces of
work than fits in Phase 4. The current per-instance limit is a known
softness: a malicious caller distributing requests across multiple
instances can exceed the intended global rate. Acceptable for the
v0.2 deployment profile (one or two instances behind a load balancer,
not adversarial-grade rate enforcement).

**Why OAuth state and DPoP nonces are deferred** — both are flagged in
their own modules as single-instance limitations. The OAuth admin
flow specifically uses sticky sessions or a Redis-backed alternative
in production deployments; v0.2's bsky-PDS-parity admin UI accepts
this. DPoP isn't wired into the OAuth path yet.

So the Phase 4.3 implementation scope reduces to: **one cache type
(`local_records`), NOTIFY emitted at the existing `invalidate_did` call
sites in repo write handlers, single LISTEN connection per process**.

## 5. Operator considerations

### 5.1 Connection pool sizing

Each Aurora-Locus instance opens:

- The existing `AnyPool` for application queries (default 10
  connections; configured via `AURORA_DB_MAX_CONNECTIONS`).
- One dedicated connection for the sequencer leader-election lock
  (the lock is held for the lifetime of a leader, so it's a
  long-idle connection).
- One dedicated connection for the LISTEN listener (long-idle).

So each instance uses `pool_size + 2` connections against Postgres.
Operators sizing managed-Postgres connection limits should account
for `(pool_size + 2) × instance_count`.

### 5.2 Failover characteristics

- **Leader-process termination** (kill, OOM, graceful shutdown):
  standbys reacquire within 2s (or `AURORA_SEQUENCER_LEADER_RETRY_MS`,
  whichever is configured). Firehose has up to 2s of write-side
  silence; events queued in Postgres `repo_seq` are not lost — the
  new leader picks up where the old left off, sequencing forward.
- **Postgres restart**: all instances reconnect; one wins the lock
  race. ~connection-establishment latency before firehose resumes.
- **Network partition** between an instance and Postgres: the
  isolated instance's TCP connection eventually times out (operator-
  tuned via `tcp_keepalives_*`); during the timeout window the
  isolated instance still thinks it's leader and may attempt writes
  that fail at the network layer.

### 5.3 Configuration

New env vars introduced in Phase 4:

- `AURORA_SEQUENCER_LEADER_RETRY_MS` (default `2000`): standby retry
  interval. Bounds 500–30000.

No other configuration; channel names, lock keys, payload schema are
all hardcoded.

## 6. Out of scope (deferred or excluded)

Spelled out separately from §2 because these are *positive deferrals*
— work we know about, that's specifically not happening in Phase 4.

### 6.1 Distributed rate limiting

The `distributed_rate_limiter` hook in `RateLimiter` is unwired.
Wiring it would require either a Redis backend or a Postgres-CAS-
based token bucket. Both are larger than fits in Phase 4. Filed as a
separate concern; not in any current chainlink issue. Acceptable for
v0.2 deployment scale.

### 6.2 OAuth state and DPoP nonces multi-instance

Both flagged as known per-process limitations in their own modules.
Solving multi-instance for them needs a different mechanism (Redis or
a SQL-backed transient store) and probably a different design
discussion. Out of scope for v0.2.

### 6.3 Caches with TTL > 5s

The TTL-fallback story (§4.6) costs at most one TTL window of
staleness on a missed NOTIFY. For the only currently-NOTIFY'd cache
(LocalRecordsCache, 5s TTL), that's 5s. If future caches with longer
TTLs are added (minutes to hours), the design needs to either:

- Accept longer-staleness windows on listener-disconnect events.
- Add a cache-version-vector exchange after listener reconnect that
  invalidates everything cached since the disconnect started.
- Move the long-TTL cache to Postgres-backed (like DidCache) so
  there's no per-process layer to invalidate.

For now: only short-TTL in-memory caches use the NOTIFY mechanism.

### 6.4 LocalRecordsCache invalidation granularity

Audit surfaced that `LocalRecordsCache::invalidate_did()` actually
calls `cache.invalidate_all()` due to a Moka API limitation —
prefix-based eviction isn't directly supported. The current cost is
one full cache flush per per-DID write; under Phase 4's NOTIFY
mechanism that becomes one full cache flush per per-DID write *per
instance*. For a few-hundred-instance deployment this is fine
(small cache, 5s TTL, low write rate per DID); for larger
deployments this is wasteful. Filing a follow-up to optimize via
secondary index or DashMap migration is appropriate but not Phase 4
work.

## 7. Open questions

### 7.1 Standby behaviour on firehose-write requests

Should standbys forward firehose-emitting requests to the leader
internally, or return 503 Service Unavailable for the load balancer
to retry?

- **503-and-retry**: simpler to implement, matches stateless-LB
  ergonomics (LB sees the 503 and retries on a different instance,
  hopefully the leader). Cost: extra round trip on failover.
- **Internal forwarding**: invisible to clients; standby maintains a
  short-lived pool to the leader's HTTP endpoint. Cost: per-instance
  service discovery, internal trust boundary.

Default plan for Phase 4.2: **503-and-retry**. Internal forwarding
can be added later if operators report it as an issue.

### 7.2 Heartbeat for stalled-but-alive leader

A leader holding its connection but stalled (deadlock, long GC
pause) blocks standbys indefinitely. Phase 4 accepts this as a known
limitation. A future phase could add an application-level heartbeat
(leader writes to a `leader_alive_at` row periodically; standbys
check it before deferring).

Decision deferred to operator feedback. If we hit the case in
practice, file an issue.

### 7.3 NOTIFY de-duplication

If two instances NOTIFY for the same DID concurrently (e.g. both
forward a write to the leader, both observe the commit), the
listeners receive two notifications and invalidate twice. The
invalidation is idempotent so this is harmless; but it does make
NOTIFY traffic noisier than strictly necessary. Phase 4 doesn't
optimize for this. If NOTIFY traffic ever becomes an operator
concern, de-dupe by writing-instance-id + timestamp into the
payload.

---

## Appendix A — Cache audit raw findings

Conducted Phase 4.1 on commit `105f444` (admin/mod Phase 2.4 head).
Twelve in-process state-holding modules surveyed; categorized in
§4.7. Audit transcript preserved in chainlink #88's session log;
not reproduced here.
