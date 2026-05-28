# Performance & Observability

This doc covers monitoring and profiling guidance for an
Aurora-Locus deployment. It is **not** a benchmarks-result doc:
meaningful latency / throughput numbers depend on hardware,
workload shape, backend (SQLite vs Postgres), blob storage (disk
vs S3), federation topology, and concurrency level. Operators are
expected to establish their own baselines against representative
load and watch for regressions over time. The architecture is
designed for sub-100ms operations on typical hardware; specific
targets are deployment-dependent.

For the structural picture of how the pieces fit, see
[../architecture.md](../architecture.md). For the env-var surface
that controls observability (`RUST_LOG`, `LOG_FORMAT`), see
[configuration.md](configuration.md) §15.

---

## 1. Design principles

The principles behind the latency story (all source-confirmed
against current code):

- **Async I/O.** All request paths are non-blocking via
  [`tokio`](../architecture.md#1-technology-stack) (full features).
  Long-running operations (CAR exports, firehose streaming) yield
  to the runtime instead of blocking worker threads.
- **Connection pooling.** Database access goes through
  [`sqlx::AnyPool`](../architecture.md#3-dual-backend-architecture)
  with operator-tunable pool size, min/max bounds, acquire timeout,
  idle timeout, max lifetime (see [configuration.md](configuration.md)
  §4). A separate maintenance pool (§6 in configuration) sizes the
  Postgres-CAS substrate (DPoP JTI replay, rate-limit buckets) so
  request and substrate traffic don't compete.
- **Streaming.** Large CAR exports (`com.atproto.sync.getRepo`) and
  the firehose (`com.atproto.sync.subscribeRepos`) stream rather
  than buffer — memory use stays bounded regardless of repo size.
- **Compiled queries.** `sqlx::query!` macros validate SQL at
  compile time.
- **Compression.** `tower_http::CompressionLayer` is applied on the
  router so responses are gzip'd on the wire when the client
  supports it (see [src/server.rs](../../src/server.rs)).
- **Release builds.** `cargo build --release` is mandatory for any
  performance-sensitive deployment — debug builds are ~10× slower
  on every code path (no optimization, no inlining). All operator
  documentation defaults to `cargo run --release --bin aurora-locus`
  for the same reason.

---

## 2. What to monitor

### 2.1 Health endpoints

Four health endpoints under `/health` (defined in
[src/api/health.rs](../../src/api/health.rs)):

- **`/health`** — basic liveness; returns `{"status": "ok",
  "version": "X.Y.Z"}`. No state checks. Suitable as a load-balancer
  ping when you want the cheapest possible probe.
- **`/health/live`** — Kubernetes liveness probe. Same shape as
  `/health` but explicitly named for the probe semantics. Returns
  200 when the process is alive; intended to trigger a restart only
  on hard hang.
- **`/health/ready`** — Kubernetes readiness probe. Checks database
  connectivity and blob storage. Returns 503 when either is
  unreachable so the load balancer drains the instance.
- **`/health/detailed`** — full component-by-component health
  (database, blob storage, background jobs, sequencer). JSON with
  per-component status, response time, error message,
  details object. Suitable for monitoring dashboards. Includes
  uptime in seconds (from the `UPTIME_SECONDS` gauge).

### 2.2 Prometheus `/metrics` endpoint

Aurora-Locus exposes a Prometheus scrape endpoint at `/metrics`
(wired in [src/server.rs](../../src/server.rs); the handler renders
`text/plain; version=0.0.4` which is the Prometheus exposition
format).

The metric surface is wide — 76+ named metrics defined in
[src/metrics.rs](../../src/metrics.rs). Honest framing: many
metrics are emitter-side defined and wired, but the module carries
`#![allow(dead_code)]` because some metrics are forward-substrate
that aren't yet incremented from every relevant code path. Spot-check
the metrics you actually care about before relying on them.

Metric families that ARE wired and reliable:

- **HTTP** (`http_requests_total`, `http_request_duration_seconds`,
  `http_requests_active`) — request count by method/path/status,
  latency histogram, in-flight count.
- **Database** (`db_queries_total`, `db_query_duration_seconds`,
  `db_connections_active`, `db_connections_pool_size`) — query
  count by operation/table, query latency, active connections, pool
  size.
- **Cache** (`cache_hits_total`, `cache_misses_total`, `cache_size`)
  — per-cache-type hit rate.
- **Sequencer** (`sequencer_events_total`, `sequencer_current_seq`,
  `sequencer_lag`, `sequencer_cursor_position`) — event throughput
  + consumer lag.
- **Relay** (`relay_events_total`,
  `relay_event_processing_duration_seconds`,
  `relay_connection_status`, `relay_connections_total`,
  `relay_events_published_total`) — federation publish path
  (relay_connection_status is 0/1 — page on it transitioning to 0
  and not recovering).
- **GC sweep** (`gc_sweep_orphans_found_total`,
  `gc_sweep_orphans_deleted_total`) — blob orphan reclamation
  signal; useful for sizing the orphan backlog before promoting GC
  out of dry-run mode (see [blob-gc-sweep.md](blob-gc-sweep.md)).
- **Background jobs** (`background_jobs_total`,
  `background_job_duration_seconds`, `background_jobs_active`).
- **Validation** (`validation_total`,
  `validation_duration_seconds`, `validation_failures_total`).
- **Blob storage** (`blob_uploads_total`, `blob_storage_bytes_total`,
  `blob_count_total`).
- **Repo** (`repo_operations_total`, `repo_records_total`,
  `repo_records_by_collection`, `repo_size_bytes`,
  `repo_storage_by_collection_bytes`, `repo_commits_total`).
- **Federation** (`federation_requests_total`,
  `federation_latency_seconds`).
- **Process** — the `prometheus` crate's `process` feature pulls in
  CPU, memory, open-FD, start-time gauges automatically.
- **`uptime_seconds`** — process uptime gauge (also surfaced via
  `/health/detailed`).

### 2.3 Tracing / logging

`tracing` 0.1 with `tracing-subscriber` 0.3 (env-filter, json
features); the filter is controlled by `RUST_LOG` (see
[configuration.md](configuration.md) §15). `LOG_FORMAT={text,json}`
picks between pretty-print (development) and JSON (production
log aggregators).

`tower_http::TraceLayer::new_for_http()` is applied on the router
so every HTTP request gets a span at the configured level. For
deeper debugging, filter to a specific module: `RUST_LOG=aurora_locus=info,aurora_locus::federation=debug,aurora_locus::apply_writes=debug`.

### 2.4 CLI introspection

Two CLI subcommands for one-shot health / metrics snapshots:

- `aurora-locus health-check [--format json|text]` — runs the same
  checks as `/health/detailed` but from the command line.
- `aurora-locus export-metrics [--format text|json] [--output PATH]`
  — emits a Prometheus snapshot to stdout or a file. Useful for
  pre-deploy / post-deploy comparison.

### 2.5 Key signals to alert on

A reasonable starting alert set (translate to whatever alert
manager you use):

- **Liveness fail loop**: more than 3 `/health/live` failures in 60s.
- **Readiness flap**: `/health/ready` flipping between 200 and 503
  more than 5 times in 5 minutes (usually DB pool exhaustion or
  blob store outage).
- **Relay disconnection**: `relay_connection_status == 0` for
  > 2 minutes. Federation publishes will queue up behind a stalled
  relay; recovery is automatic but operator visibility matters.
- **Sequencer lag**: `sequencer_lag` over some workload-dependent
  threshold (e.g. > 1000 events for > 5 minutes).
- **Pool exhaustion**: `db_connections_active` at
  `db_connections_pool_size` and HTTP request latency p99 rising
  — usually means the pool is undersized for the workload.
- **Auth failures**: HTTP requests with 401/403 status rising
  sharply (could be a misconfigured relay, an expired admin token,
  or an attack).

---

## 3. Acceptable latency targets

Aurora-Locus's design targets sub-100ms operations on typical
hardware for the common request paths (record CRUD, session
operations, single-record reads). Real numbers are
deployment-dependent — the relevant variables:

- **Backend.** Postgres adds network round-trip vs SQLite's
  in-process I/O; the trade-off is HA + multi-instance capability.
- **Blob storage.** Local disk is faster than S3-compatible
  storage; S3 trades latency for durability and operational
  simplicity.
- **Concurrency.** The pool sizing (see
  [configuration.md](configuration.md) §4) is the main lever; the
  default 25 max connections suits modest single-instance
  deployments.
- **Repo size.** CAR exports are streamed and stay bounded in
  memory; latency scales with repo size at network throughput.

Establish baselines against your representative workload before
deploying. Watch the histogram tails
(`http_request_duration_seconds_bucket`) — p50 is rarely
load-bearing; p99 + p999 are what wake the on-call.

---

## 4. Profiling

For deeper-than-metrics investigation:

- **Release-mode profile.** Always profile against
  `cargo build --release` (or `cargo run --release --bin
  aurora-locus`) — debug-build profiles are misleading on every
  axis.
- **`tracing` spans.** The existing instrumentation gets you
  per-request and per-job spans; turning the level up
  (`RUST_LOG=aurora_locus=debug`) surfaces the most-common
  hotspots without external tooling.
- **External profilers.** Standard Rust tooling applies:
  `perf` + `cargo flamegraph` for CPU, `heaptrack` for allocation,
  `tokio-console` for async-runtime introspection. None are
  integrated into the binary (no `console_subscriber` dep); they
  attach externally to the running release process.

---

## 5. Cross-references

- [../architecture.md](../architecture.md) — the structural picture
  (technology stack, module layout, dual-backend, multi-instance,
  federation).
- [configuration.md](configuration.md) — `RUST_LOG`, `LOG_FORMAT`,
  pool tuning vars, rate-limit vars, distributed-state substrate
  config.
- [multi-instance-deployment.md](multi-instance-deployment.md) — HA
  topology and the leader-election / LISTEN-NOTIFY tuning knobs that
  affect cross-instance latency.
- [blob-gc-sweep.md](blob-gc-sweep.md) — interpreting the GC sweep
  metrics and the dry-run-to-destructive promotion ceremony.

---

## 6. Future work

Areas where observability is thin and worth tightening (flagged
honestly):

- **`#![allow(dead_code)]` on metrics.rs.** Several metrics are
  defined but not yet wired into every relevant code path. Drop the
  blanket allow and surface the unwired ones, then either wire them
  or remove them.
- **Reproducible benchmark harness.** No in-tree benchmark suite
  produces canonical latency numbers; current operator-facing
  expectations are "sub-100ms on typical hardware" without a
  citable methodology. A `criterion`-based microbenchmark suite
  for the hot paths would let operators reason about changes
  before deploying.
- **OpenTelemetry / OTLP export.** `tracing` is wired but there's no
  OTLP exporter — traces stay in stdout. An OpenTelemetry exporter
  would let operators correlate Aurora-Locus traces with upstream
  service traces (relay, identity resolver).
- **`tokio-console` integration.** No `console_subscriber`
  integration today. For runtime debugging of stuck tasks /
  contended locks, attaching tokio-console requires adding the dep
  and gating it behind a feature flag.
