# Architecture

Aurora-Locus is a Rust-native ATProto Personal Data Server. It runs as
a single process backed by either SQLite (default, single-instance) or
Postgres (multi-instance, HA-capable). Per-actor repository storage is
always SQLite regardless of the shared-DB backend; this lets the
shared-DB layer scale across instances while keeping per-actor I/O
isolated and local. Federation is first-class: relay publication, lexicon
resolution, cross-PDS service auth, and DPoP-bound tokens are all in
the default build.

This doc covers the structural picture — what's in the tree and how the
pieces fit. Operator-facing depth lives in:

- [operator/configuration.md](operator/configuration.md) — every
  environment variable the server reads.
- [operator/multi-instance-deployment.md](operator/multi-instance-deployment.md)
  — Postgres + leader election + LISTEN/NOTIFY HA topology.
- [operator/wal-archiving.md](operator/wal-archiving.md) — Postgres
  backup and point-in-time recovery.
- [operator/admin-auth.md](operator/admin-auth.md) — admin authority
  model and SuperAdmin bootstrap.
- [operator/performance.md](operator/performance.md) — observability
  surface and profiling guidance.

---

## 1. Technology stack

Pinned versions from [`Cargo.toml`](../Cargo.toml) (this section is the
canonical "Built with..." reference; the README defers to it):

- **AT Protocol SDK** — [`proto-blue`](https://crates.io/crates/proto-blue)
  `0.3.1` (published crate; replaces the previously-vendored
  `Rust-Atproto-SDK` source tree).
- **HTTP server** — `axum` 0.7 (with `tower`, `tower-http`,
  `axum-extra`).
- **Database** — `sqlx` 0.8 (`sqlite`, `postgres`, `any`, `chrono`,
  `uuid` features). The `any` feature is load-bearing — it backs the
  dual-backend story.
- **Async runtime** — `tokio` 1 (full features).
- **Cryptography** — `k256` 0.13 (secp256k1 — repo-signing + PLC
  rotation keys), `p256` 0.13 (DPoP), `sha2` 0.10, `argon2` 0.5
  (Argon2id password hashing), `jsonwebtoken` 10 (HS256/ES256K JWTs).
- **Federation transport** — `reqwest` 0.12 (HTTPS), `tokio-tungstenite`
  0.27 (WebSocket relay), `hickory-resolver` 0.26.1 (DNS-TXT lexicon
  authority lookups).
- **Blob storage** — `aws-sdk-s3` 1 (S3 + S3-compatible: MinIO, R2,
  Spaces, B2), `image` 0.25 (thumbnails + metadata).
- **Rate limiting** — `governor` 0.7 (in-process token-bucket), plus a
  Postgres-CAS substrate for cross-instance limits (see §4).
- **Observability** — `tracing` 0.1 + `tracing-subscriber` 0.3
  (env-filter, json), `prometheus` 0.14 (process feature).
- **Email** — `lettre` 0.11 (SMTP).
- **CLI** — `clap` 4 (derive).
- **Misc** — `serde`, `serde_json`, `serde_yaml`, `serde_cbor`,
  `dashmap`, `moka`, `fs2` (cooperative file locking for the
  SQLite-backend liveness lock).

Workspace is `edition = "2021"`; no `rust-version` pin (any recent
stable toolchain).

---

## 2. Module layout

Top-level modules under [`src/`](../src/). One-line summaries; the
modules themselves carry doc-comments with the load-bearing detail.

| Module | Purpose |
|---|---|
| [`account/`](../src/account/) | AccountManager: CRUD, sessions, app passwords, email verification, password reset, invite codes, deletion/restoration. |
| [`actor_store/`](../src/actor_store/) | Per-actor SQLite repo storage; MST integration; transactional record writes; CAR export. |
| [`admin/`](../src/admin/) | Role / audit-chain / moderation / labels / reports / invites / appeals / mod-event-seq managers. |
| [`api/`](../src/api/) | HTTP handler modules per namespace: `server`, `repo`, `sync`, `blob`, `admin` (route table), `aurora_admin`, `aurora_moderator`, `aurora_lexicon`, `aurora_subscribe`, `firehose`, `health`, `well_known`, `identity`, `labels`, `moderation`, `oauth_server`, `oauth_admin`, `federation`, `appview`, `repo_import`, plus `extractors`, `middleware`, `registry`, `dev_routes`. |
| [`auth.rs`](../src/auth.rs) | Bearer-token auth: session JWT (Layer 1), HS256 admin scope (Layer 2), ES256K service-auth pre-check + verification (Layers 3-4), `admin_roles` lookup (Layer 5). See [operator/admin-auth.md](operator/admin-auth.md). |
| [`backup/`](../src/backup/) | Backup/restore subcommands (manifest format, file tree, audit-chain preservation). |
| [`blob_store/`](../src/blob_store/) | Disk + S3 backends (mutually exclusive), CID content-addressing, two-phase upload, GC sweep + quarantine, MIME sniffing, promoter. |
| [`cache/`](../src/cache/) | Cross-instance LISTEN/NOTIFY invalidation (Postgres-only) for the local-records cache. |
| [`cli/`](../src/cli/) | Subcommand implementations: `create-account`, `migrate-oauth`, `bulk-migrate-oauth`, `backup`, `restore`, `generate-did-key`, `generate-service-token`, `health-check`, `export-metrics`, `publish-identity{,-file}`, `rotate-keys{,-file}`, `validate-config`, `grant-admin`, `gc-sweep`, `debug` (sub-tree). |
| [`config.rs`](../src/config.rs) | Environment-variable loader; the comprehensive surface is in [operator/configuration.md](operator/configuration.md). |
| [`context.rs`](../src/context.rs) | `AppContext` — DI container threaded through every handler. |
| [`crypto/`](../src/crypto/) | Keypair handling, PLC client, secp256k1 + proto-blue signer integration. |
| [`db/`](../src/db/) | `sqlx::Any` pool factory, per-backend migration runner, advisory-lock registry, autocommit typed wrappers, cooperative `liveness_lock`. |
| [`distributed/`](../src/distributed/) | Cross-instance state substrate (`PostgresCasStore`): OAuth flow state, DPoP JTI replay, rate-limit buckets. Redis slot is forward-compat only. |
| [`error.rs`](../src/error.rs) | `PdsError` + `PdsResult` with wire-error mapping for HTTP responses. |
| [`federation/`](../src/federation/) | Relay client, service auth, DPoP, nonce store, entryway, lexicon resolver/cache/fetcher, DNS-TXT resolver, peer-PDS discovery, blob-fetch primitive, federated search. |
| [`identity/`](../src/identity/) | DID resolver (PLC + Web), identity cache, handle validation, reserved-handle table. |
| [`jobs/`](../src/jobs/) | Background tasks: grouped cleanup, cache reapers, GC sweeps, federation maintenance. |
| [`mailer/`](../src/mailer/) | SMTP email pipeline: notifications, templates, per-account rate limiting, tracking. |
| [`metrics.rs`](../src/metrics.rs) | Prometheus metric definitions (76+ named metrics across HTTP, DB, cache, jobs, moderation, repo). Render at `/metrics`. |
| [`oauth/`](../src/oauth/) | OAuth 2.1 server: authorize, token, device, consent, scope hierarchy, token rotation, flow-state adapter. |
| [`rate_limit.rs`](../src/rate_limit.rs) | Multi-axis throttling: global / per-endpoint / per-IP / per-user. Distributed-RL integration runs as PRIORITY 0 ahead of the in-process `governor` layer. |
| [`read_after_write/`](../src/read_after_write/) | Local-records cache (5s TTL) for read-after-write consistency on freshly-committed records; LISTEN/NOTIFY invalidated under Postgres. |
| [`repository/`](../src/repository/) | Blob-reference graph maintenance. |
| [`sequencer/`](../src/sequencer/) | Event log + WebSocket firehose (`com.atproto.sync.subscribeRepos`); Postgres-only leader election. |
| [`server.rs`](../src/server.rs) | HTTP server entry: router assembly, middleware stack, CORS, compression, `TraceLayer`, `/metrics` route. |
| [`service_auth.rs`](../src/service_auth.rs) | Cross-PDS service-JWT issuance (per-account ES256K signing). |
| [`validation/`](../src/validation/) | Record schema validation; Required / Optimistic / None modes per `VALIDATION_MODE`. |

The crate builds a single binary, `aurora-locus`.

---

## 3. Dual-backend architecture

The shared-DB backend is selectable via `PDS_DB_BACKEND={sqlite,postgres}`
(see [operator/configuration.md](operator/configuration.md) §4). Both
backends are first-class: every shared-DB code path goes through
`sqlx::AnyPool`, with backend-specific behavior contained to the few
spots where SQLite and Postgres genuinely diverge.

**What's shared (via `sqlx::Any`):**

- Account database (`account.sqlite` / Postgres `accounts` schema).
- Sequencer event log.
- DID cache.
- Admin tables (roles, audit-chain, moderation events, reports,
  appeals, invite codes).
- Distributed-state substrate tables (OAuth flow state, DPoP JTI,
  rate-limit buckets) under Postgres mode; absent under SQLite.

**What's per-backend specific by intentional design:**

- Migrations live in [`migrations/`](../migrations/) (SQLite) and
  [`migrations/postgres/`](../migrations/postgres/) (Postgres). The
  runner picks the right set based on `PDS_DB_BACKEND`.
- Boolean column reads use [`db::read_bool`](../src/db/mod.rs) to
  paper over SQLite-INTEGER vs Postgres-BOOLEAN decoding.
- Leader election and LISTEN/NOTIFY cache invalidation are
  Postgres-only — see §4.
- The distributed-state substrate is Postgres-CAS by default
  (`PDS_DISTRIBUTED_STATE_MODE=distributed`); SQLite deployments may
  opt to `single_instance_inmemory` to skip the maintenance pool.

**What's always SQLite regardless of backend:**

- Per-actor repository stores (one SQLite file per DID under
  [`PDS_ACTOR_STORE_DIRECTORY`](../src/actor_store/)). This is the
  architectural choice that lets a Postgres-backed Aurora-Locus
  scale out on the shared-DB tier without having to coordinate
  per-actor MST writes across instances — each actor's storage is
  local I/O.

---

## 4. Multi-instance shape

Single-instance Aurora-Locus on SQLite needs no coordination
substrate; the SQLite-backend `liveness_lock` (cooperative
`flock(2)` / `LockFileEx` via the `fs2` crate) is the only thing
preventing two `serve` processes from racing against one DB file.

Multi-instance Aurora-Locus on Postgres adds three pieces:

- **Leader election** ([`sequencer/leader_election.rs`](../src/sequencer/leader_election.rs)).
  One process holds a session-scoped `pg_try_advisory_lock` —
  that's the leader. Other processes are standbys that retry every
  `PDS_SEQUENCER_LEADER_RETRY_MS` (default 2000ms; bounds 500-30000).
  Connection drop releases the lock automatically; graceful shutdown
  releases explicitly so the next standby doesn't have to wait the
  retry interval. Only the leader writes sequencer events.
- **LISTEN/NOTIFY cache invalidation**
  ([`cache/invalidation.rs`](../src/cache/invalidation.rs)). Single
  channel `aurora_cache_invalidate` with JSON payload `{"type":
  "...", "key": "..."}`. Dedicated `PgListener` connection
  (not from the AnyPool) with auto-reconnect. Currently invalidates
  the local-records cache by DID. Missed notifications self-correct
  via per-cache TTL fallback.
- **Distributed-state substrate**
  ([`distributed/postgres_cas.rs`](../src/distributed/postgres_cas.rs)).
  Three cross-instance state surfaces — OAuth flow state, DPoP JTI
  replay tracking, rate-limit buckets — share a single
  `DistributedStore`-trait-shaped layer over Postgres CAS. The
  maintenance pool ([`PDS_MAINTENANCE_DB_*`](operator/configuration.md))
  is sized smaller than the main pool so total Postgres connection
  count stays predictable.

Full topology, leader-election failure modes, and operator runbook
live in [operator/multi-instance-deployment.md](operator/multi-instance-deployment.md).

---

## 5. Federation surface

`src/federation/` is 15 files; the structural pieces:

- **`relay.rs`** — WebSocket relay client. Auto-reconnect with backoff,
  multi-relay support (`PDS_FEDERATION_RELAY_URLS` is comma-separated).
  Publishes `#commit`, `#identity`, `#account`, `#sync`, `#tombstone`
  frames.
- **`service_auth.rs` + top-level `src/service_auth.rs`** — Cross-PDS
  service JWTs. Per-account ES256K signing; receiver-side validation
  via 11+ structural checks plus identity-resolver lookup.
- **`dpop.rs`** — DPoP proof generation/verification (P-256); replay
  tracking via the distributed substrate when running multi-instance.
- **`nonce_store.rs`** — Server-issued nonces for DPoP and service-auth
  challenges.
- **`lexicon_resolver.rs` + `lexicon_cache.rs` + `lexicon_fetcher_prod.rs`**
  — Dynamic lexicon loading. DNS-TXT authority resolution → HTTP
  fetch of `_lexicon.<host>` records → two-layer cache (in-memory hot
  + on-disk persistent) → single-flight de-dup on concurrent misses.
- **`dns_resolver.rs`** — `hickory-resolver` wrapper for
  `_lexicon.<host>` TXT lookups; the `PDS_LEXICON_DNS_NAMESERVER`
  override is a test-harness affordance and must not be set in
  production.
- **`entryway.rs` + `entryway_headers.rs`** — Entryway-mode HTTP
  clients and pass-through header machinery for the entryway
  integration.
- **`discovery.rs`** — Trusted peer-PDS allowlist plus relay-driven
  PDS-instance discovery.
- **`search.rs`** — Federated search (`searchActors`, `searchPosts`,
  Aurora's `aggregateTimeline`) with per-PDS circuit breakers
  (3 failures / 60s cooldown).
- **`blob_fetch.rs`** — Origin-PDS blob fetch primitive used by
  importRepo and cross-instance blob hydration.
- **`authentication.rs`** — Cross-PDS auth via DID resolution.

The user-facing federation summary is in the README's
Features → Federation & Interop checklist. The operator-facing
configuration surface is [operator/configuration.md](operator/configuration.md)
§17.

---

## 6. Cross-references

- [operator/configuration.md](operator/configuration.md) — environment
  variable reference (24 sections, every var the server reads).
- [operator/multi-instance-deployment.md](operator/multi-instance-deployment.md)
  — HA topology, leader-election failure modes, operator runbook.
- [operator/wal-archiving.md](operator/wal-archiving.md) — Postgres
  backup + PITR.
- [operator/admin-auth.md](operator/admin-auth.md) — bootstrap and the
  five-layer auth-resolution chain.
- [operator/performance.md](operator/performance.md) — observability
  surface, profiling guidance, tuning knobs.
- [operator/admin-endpoint-reference.md](operator/admin-endpoint-reference.md)
  — admin/moderation/ops endpoint inventory (as of 2026-05-03 snapshot).
