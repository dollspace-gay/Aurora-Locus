# Aurora-Locus Postgres Backend — Assessment

**Surface:** Selective Postgres backend for shared global state; per-actor state stays SQLite
**Status:** Assessment — scaffold exists, schema and integration work remain
**Reference target:** sqlx 0.8 with both `sqlite` and `postgres` features (already configured)
**Depends on:** Existing `src/db/` infrastructure, `migrations/` directory, `AppContext` construction
**Date:** 2026-04-30

---

## 1. Where Aurora-Locus stands today

Aurora-Locus's Postgres support is partially scaffolded. The infrastructure shell exists: `Cargo.toml` declares both `sqlite` and `postgres` features for sqlx, [src/db/postgres.rs](src/db/postgres.rs) defines `PostgresConfig` with env-driven configuration plus `create_pool` and `run_migrations` functions, and [migrations/postgres/](migrations/postgres/) exists as a directory ready to receive schema files. None of this is wired into the actual application.

The gap is the wiring, the schema, and the per-file query layer. Not a from-scratch project.

### 1.1 What's already there

[src/db/postgres.rs](src/db/postgres.rs) ships a working Postgres pool factory:

- `PostgresConfig` struct with `database_url`, connection pool sizing, timeouts, lifetime/idle parameters
- `PostgresConfig::from_env()` reads `DATABASE_URL` or `POSTGRES_URL` plus tunable env vars (`POSTGRES_MAX_CONNECTIONS`, etc.)
- `create_pool(config)` returns a `PgPool` with the configured options applied
- `run_migrations(pool)` runs `sqlx::migrate!("./migrations/postgres")` against the pool

Both functions and the struct are marked `#[allow(dead_code)]` because nothing currently calls them. The module is reachable through `pub mod postgres` in [src/db/mod.rs](src/db/mod.rs) but `AppContext` construction never reaches it.

### 1.2 What's missing

Three independent pieces of work:

**The Postgres schema.** `migrations/postgres/` is an empty directory. No `0001_initial.sql` Postgres counterpart exists. The SQLite schema at `migrations/0001_initial.sql` is 479 lines and needs translation — direct copy doesn't work because of type differences (`INTEGER PRIMARY KEY AUTOINCREMENT` vs `BIGSERIAL`), boolean handling (SQLite's INTEGER 0/1 vs Postgres's real `BOOLEAN`), datetime types (`DATETIME` vs `TIMESTAMPTZ`), and pragma idioms that Postgres doesn't have.

**Backend selection in `AppContext`.** [src/context.rs](src/context.rs) currently calls into `db::create_pool` (the SQLite version in [src/db/mod.rs](src/db/mod.rs)) directly with hardcoded `SqlitePool` types. There's no configuration-driven dispatch between SQLite and Postgres. The `AppContext` struct holds `account_db: SqlitePool` as a concrete type, not a backend-abstract pool.

**Query-layer compatibility across the 17 files that touch shared databases.** Every manager struct that holds a database handle (`AccountManager`, `OAuthClientStore`, `BlobStore`, the seven admin managers, the sequencer, the mailer tracker, the DID cache) currently types its handle as `SqlitePool`. Without abstraction, none of them can take a `PgPool`. Some queries also use SQLite-specific syntax (`= 0`/`= 1` on boolean columns) that Postgres rejects.

These three pieces are independent in scope but sequential in execution: the schema must land first because there's nothing for `run_migrations` to apply; backend selection comes second because the `AppContext` must be able to choose between SQLite and Postgres before per-file refactoring can be tested against the Postgres path; query-layer compatibility comes last because it's the broadest and most error-prone work.

### 1.3 The architectural decision that shapes everything

Aurora-Locus operates **three logically distinct database surfaces**, each with different access patterns:

1. **`account_db`** — global state shared across all actors: accounts, sessions, OAuth, invites, moderation queue, labels, blobs, mailer tracking, sequencer events. High concurrent writes; fan-out reads.
2. **`did_cache_db`** — DID document and handle resolution cache. Read-heavy with periodic TTL eviction.
3. **Per-actor `repo.sqlite`** — one SQLite file per user under `data/actors/<did>/repo.sqlite`. Holds the actor's MST state, records, and repo blocks. Lazy pool creation per-DID via LRU cache.

The first two benefit from Postgres in production deployments. Multi-instance writers, real backup tooling (`pg_dump`/`pg_basebackup`/WAL archiving), shared cache state across instances — these are Postgres affordances that SQLite cannot replicate.

**Per-actor state stays SQLite.** Migrating per-actor stores to Postgres would mean either schema-per-actor (operationally awful at scale) or shared tables with `actor_did` columns (loses the actor-isolation property bsky-PDS deliberately preserves). Per-actor SQLite is the right tool for the job: each actor's data is a single file — trivially backupable, exportable, deletable, migratable. No cross-actor query contention. Lazy pool creation means closed actors don't consume connection slots. This matches bsky-PDS's deliberate choice; account migration, account export, and deletion are all "move/copy/remove this one file" operations under that model.

The scope of this work is therefore **selective Postgres**: shared global state migrates to a configurable backend (SQLite for hobbyist deployments, Postgres for production); per-actor state always stays SQLite. Both deployment paths are first-class.

---

## 2. The three-database architecture

Aurora-Locus's existing [src/context.rs](src/context.rs) already separates the three surfaces in its `AppContext` initialization. The hybrid backend model preserves that separation — only the first two surfaces become backend-configurable.

### 2.1 Database 1: `account_db` (global state, → configurable backend)

**Path (SQLite):** `data/account.sqlite`
**Path (Postgres):** Configured via `DATABASE_URL`

**Tables (per [migrations/0001_initial.sql](migrations/0001_initial.sql)):** `actor`, `account`, `session`, `app_password`, `email_token`, `plc_keys`, `invite_code`, `account_moderation`, `record_label`, `report`, `appeal`, `admin_event`, `admin_role`, `admin_audit_log`, `mailer_tracking`, `oauth_*` (multiple OAuth tables), `blob`, `record_blob`, `temp_blob_metadata`, `nonce_store`, `sequencer_*`.

**Access pattern:** Heavy concurrent writes (account creation, session refresh, moderation actions, sequencer event commits); fan-out reads (admin dashboards, queries against the moderation queue, OAuth flow lookups).

**Why Postgres helps here:**
- Multi-instance writers via advisory locks
- Real concurrency vs SQLite's single-writer-at-a-time model
- Production-grade backup/restore (`pg_dump`, `pg_basebackup`, WAL archiving for point-in-time recovery)
- Cross-instance event broadcast via `LISTEN/NOTIFY` (relevant if multi-instance deployments use Aurora-Locus)

### 2.2 Database 2: `did_cache_db` (resolution cache, → configurable backend)

**Path (SQLite):** `data/did_cache.sqlite`
**Path (Postgres):** Same `DATABASE_URL` as `account_db` (different schema namespace) or separate URL via config

**Tables:** `did_doc`, `did_handle` (DID document content, handle-to-DID resolution cache).

**Access pattern:** Read-heavy with periodic TTL eviction. Most reads are cache hits.

**Why Postgres helps here:**
- Shared cache state across multiple Aurora-Locus instances (avoids each instance maintaining its own cache and re-resolving the same DIDs independently)
- Consistent eviction (TTL deletion via cron-like job runs once globally instead of N times per instance)

For single-instance deployments, the Postgres benefit over SQLite is marginal — the cache is small and SQLite handles read-heavy workloads well. Multi-instance deployments are where this matters.

### 2.3 Database 3: Per-actor `repo.sqlite` (always SQLite)

**Path:** `data/actors/<did>/repo.sqlite` (one file per actor)

**Tables (created inline in [src/actor_store/store.rs](src/actor_store/store.rs)):** `repo_root`, `repo_block`, `record`. The `proto-blue` SDK's `RepoStorage` trait now backs these via the `SqliteRepoStorage` bridge in [src/actor_store/repo_storage.rs](src/actor_store/repo_storage.rs); the per-actor SQLite remains the underlying store.

**Access pattern:** All reads/writes for a given actor go to that actor's file. No cross-actor queries. Lazy pool creation per-DID via LRU cache; closed actors don't hold connection slots.

**Why Postgres does NOT help here:**
- Postgres can't naturally do "one database per user" — schema-per-actor is operationally awful at any reasonable user count, and shared-tables-with-actor-did columns lose actor isolation
- Account migration / export / deletion are file-level operations under the per-actor SQLite model; under shared Postgres they'd require row-level coordination across many tables
- The bsky-PDS reference design is explicitly per-actor SQLite for these reasons; Aurora-Locus matches that

This work explicitly **does not** include any per-actor store migration to Postgres. The hybrid (global state on configurable backend, per-actor state always SQLite) is the architecture.

---

## 3. Schema translation: SQLite → Postgres

The existing [migrations/0001_initial.sql](migrations/0001_initial.sql) is 479 lines covering the `account_db` schema. A Postgres counterpart at `migrations/postgres/0001_initial.sql` requires per-construct translation. Most of the work is mechanical, but there are categories of transformation worth enumerating.

### 3.1 Type translations

| SQLite | Postgres | Notes |
|---|---|---|
| `TEXT` | `TEXT` | Direct |
| `INTEGER` | `INTEGER` or `BIGINT` | Pick by domain — counters and IDs go `BIGINT` |
| `INTEGER PRIMARY KEY AUTOINCREMENT` | `BIGSERIAL PRIMARY KEY` | Postgres autoincrement idiom |
| `BOOLEAN` (stored as INTEGER 0/1) | `BOOLEAN` (true/false) | Postgres has real booleans |
| `DATETIME` | `TIMESTAMPTZ` | Postgres timestamps with timezone |
| `BLOB` | `BYTEA` | Postgres binary type |
| `REAL` | `DOUBLE PRECISION` | Or `NUMERIC` for exact decimal |

### 3.2 Default value translations

| SQLite | Postgres |
|---|---|
| `DEFAULT CURRENT_TIMESTAMP` | `DEFAULT NOW()` (or `DEFAULT CURRENT_TIMESTAMP` — both work) |
| `DEFAULT 0` (boolean column) | `DEFAULT false` |
| `DEFAULT 1` (boolean column) | `DEFAULT true` |

### 3.3 Pragma equivalents

SQLite's `PRAGMA wal_autocheckpoint`, `PRAGMA synchronous`, `PRAGMA journal_mode`, `PRAGMA foreign_keys` have no direct Postgres equivalents. Postgres tunes via `postgresql.conf`, `ALTER SYSTEM`, role privileges, and connection string parameters — not per-connection pragmas.

The pragma calls in [src/db/mod.rs](src/db/mod.rs) and [src/context.rs](src/context.rs) need backend-specific paths: SQLite path keeps the pragmas; Postgres path skips them entirely. The Postgres pool factory in [src/db/postgres.rs](src/db/postgres.rs) already does this correctly (no pragma calls).

### 3.4 Boolean column literals in queries

Aurora-Locus has queries like:

```sql
SELECT * FROM actor WHERE invitesDisabled = 0
```

The `0` is for false because SQLite stores boolean as INTEGER. Postgres needs:

```sql
SELECT * FROM actor WHERE invitesDisabled = false
```

The `Any` driver (discussed in §4.2) doesn't fix this — boolean literals in query strings are evaluated by the database, not by the driver. Audit needed across all 17 files in §4.4 for any `= 0` / `= 1` patterns on boolean columns. Replace with `= false` / `= true` (Postgres tolerates the latter on INTEGER columns; SQLite tolerates `false`/`true` as boolean literals since version 3.23.0, which Aurora-Locus's `sqlite` feature requires).

### 3.5 Postgres-specific features Aurora-Locus could use

The migration is an opportunity. Things to consider but not require for v0.2:

- **`JSONB` columns** for `detail` fields currently stored as `TEXT` with JSON content (admin event details, OAuth metadata, moderation record notes). JSONB enables querying inside the JSON, indexing on JSON fields, more efficient storage. Worth considering but not required.
- **`tsvector` + GIN indexes** for full-text search on accounts, posts, etc. Out of scope; revisit if/when search workloads demand it.
- **Partitioning** for high-volume tables (sequencer events, audit log). Out of scope; revisit at scale.
- **`LISTEN/NOTIFY`** for cross-instance event broadcasts (cache invalidation, sequencer event distribution). Useful primitive for multi-instance deployments; consider in Phase 4.

These can be added in later schema migrations after the v0.2 cycle. Keeping the v0.2 schema 1:1 with SQLite (modulo the type translations) keeps backend parity simpler to reason about during the initial migration.

---

## 4. File-by-file audit

The 22 files importing `SqlitePool` or `sqlx::sqlite::*`, classified by which database surface they touch and what work is required.

### 4.1 Group A: `account_db` consumers — Postgres-targeted (15 files)

These files all receive an `account_db` pool (or a clone) at construction. Their queries hit the global state database. All need backend-agnostic pool support.

| File | Module | Description |
|---|---|---|
| [src/account/manager.rs](src/account/manager.rs) | account | AccountManager — account/session/app-password/invite operations |
| [src/oauth/client.rs](src/oauth/client.rs) | oauth | OAuth client registration and management |
| [src/oauth/token_rotation.rs](src/oauth/token_rotation.rs) | oauth | Refresh-token rotation with grace period |
| [src/oauth/device.rs](src/oauth/device.rs) | oauth | Device authorization grant flow |
| [src/blob_store/store.rs](src/blob_store/store.rs) | blob_store | Blob metadata, quotas, temp uploads |
| [src/blob_store/quarantine.rs](src/blob_store/quarantine.rs) | blob_store | Blob takedown/quarantine state |
| [src/admin/reports.rs](src/admin/reports.rs) | admin | User-submitted reports |
| [src/admin/appeals.rs](src/admin/appeals.rs) | admin | Appeals queue |
| [src/admin/moderation.rs](src/admin/moderation.rs) | admin | Account moderation actions |
| [src/admin/events.rs](src/admin/events.rs) | admin | Admin event log |
| [src/admin/invites.rs](src/admin/invites.rs) | admin | Invite code management |
| [src/admin/roles.rs](src/admin/roles.rs) | admin | Admin role grants/revokes/audit |
| [src/admin/labels.rs](src/admin/labels.rs) | admin | Label application |
| [src/sequencer/sequencer.rs](src/sequencer/sequencer.rs) | sequencer | Event sequencer (firehose source) |
| [src/mailer/tracking.rs](src/mailer/tracking.rs) | mailer | Email send tracking |

### 4.2 Group B: `did_cache_db` consumers — Postgres-targeted (1 file)

| File | Module | Description |
|---|---|---|
| [src/identity/cache.rs](src/identity/cache.rs) | identity | DID document and handle cache |

### 4.3 Group C: Per-actor SQLite consumers — Stays SQLite (2 files)

| File | Module | Description |
|---|---|---|
| [src/actor_store/store.rs](src/actor_store/store.rs) | actor_store | Per-actor repository database manager |
| [src/actor_store/transaction.rs](src/actor_store/transaction.rs) | actor_store | Transaction handle for atomic per-actor operations |

The proto-blue SDK's `SqliteRepoStorage` bridge in [src/actor_store/repo_storage.rs](src/actor_store/repo_storage.rs) sits beneath these and routes through the same per-actor SQLite pool. No changes needed to the actor-store layer.

### 4.4 Group D: Infrastructure and test (4 files)

| File | Module | Description | Postgres impact |
|---|---|---|---|
| [src/db/mod.rs](src/db/mod.rs) | db | Pool creation, migration runner, connection testing for SQLite path | Refactor for backend dispatch (top-level wrapper that delegates to either `db::create_pool` or `db::postgres::create_pool`) |
| [src/context.rs](src/context.rs) | (root) | AppContext construction; opens `account_db` and `did_cache_db` | Refactor for backend selection from config |
| [src/cli/health.rs](src/cli/health.rs) | cli | Health-check subcommand database probe | Backend-agnostic via the abstraction layer |
| [src/identity/resolver.rs](src/identity/resolver.rs) | identity | Test code only (`SqlitePool::connect(":memory:")` in tests) | Tests stay SQLite |

(Various other test code throughout the codebase uses `SqlitePool::connect(":memory:")` for unit-test fixtures. These all stay SQLite. Postgres testing happens in dedicated integration tests; see §6.5.)

### 4.5 Counts summary

| Surface | File count | Action |
|---|---|---|
| Group A (`account_db` consumers) | 15 | Refactor to backend-agnostic pool |
| Group B (`did_cache_db` consumers) | 1 | Refactor to backend-agnostic pool |
| Group C (per-actor SQLite) | 2 | No change |
| Group D (infrastructure) | 4 | Refactor `db/mod.rs` and `context.rs`; the rest follow |
| **Total** | **22** | |

The 17 files in Groups A and B are the bulk of the work. Group D is the architectural plumbing that enables them. Group C stays untouched.

---

## 5. Backend abstraction strategy

The 17 manager structs in Groups A and B each hold a database handle. Today that handle is `SqlitePool`. To support Postgres, the handle type needs to be either generic, an enum, or a backend-agnostic wrapper. Three approaches were considered.

### 5.1 Approach 1: Generic over `sqlx::Database`

Convert all `db: SqlitePool` fields to `db: Pool<DB: Database>`, parameterizing the manager structs over the database type. Theoretically clean.

**In practice:** Causes substantial type-system friction. sqlx's compile-time query checking (`query!` and `query_as!` macros) doesn't play well with truly-generic database types — the macros want to know the concrete database to type-check parameter bindings. Different drivers also have different parameter binding syntax (`?N` vs `$N`), which the macros can't reconcile generically.

**Verdict:** Avoid. The type-system burden exceeds the benefit.

### 5.2 Approach 2: `enum DbPool { Sqlite(SqlitePool), Postgres(PgPool) }`

A wrapper enum with method-level dispatch. Each manager struct holds `DbPool`; each query method matches on the enum and runs the appropriate driver-specific query.

**In practice:** Concrete and works, but means **every query is written twice** — once for SQLite, once for Postgres. With ~80 queries across the 17 files, that's ~160 query implementations to write and keep in sync. Real per-method work, real ongoing maintenance burden.

**Verdict:** Workable but expensive. Reject in favor of approach 3 unless approach 3 fails for specific queries (where approach 2 becomes the per-query escape hatch).

### 5.3 Approach 3: `sqlx::Any` driver (recommended baseline)

sqlx ships an `Any` driver that abstracts SQLite/Postgres/MySQL behind a single type. Queries use a unified parameter syntax (`?N` works for SQLite, internally converted to `$N` for Postgres at query time). Connection pools are `AnyPool` instead of `SqlitePool` or `PgPool`. Most simple queries work without modification.

**Caveats:**
- Not all features are uniformly supported — Postgres-specific features (advisory locks, `RETURNING` clauses, `JSONB`, `LISTEN/NOTIFY`) need driver-specific paths
- Compile-time query checking is weaker — `query!` macros against `AnyPool` can't validate against a specific schema, so query mistakes that the macros would catch on `SqlitePool` become runtime errors
- Boolean literals in query strings (`= 0`/`= 1`) are evaluated by the database, not the driver — these still need backend-aware fixes per §3.4

**Verdict:** Recommended baseline. Most of Aurora-Locus's queries are simple enough that `Any` handles them. Driver-specific fallbacks via approach 2 are added only where features diverge.

### 5.4 Approach 4: Hybrid (the actual recommendation)

Use `Any` driver for the majority of code paths; provide driver-specific implementations only where features diverge. Each manager struct holds `AnyPool`; ~95% of queries use the unified path; the small number of queries that need Postgres-only features have explicit dispatch via approach 2 (match on the underlying driver and run the appropriate query).

This mirrors bsky-PDS's Kysely strategy (TypeScript): a unified ORM with backend-specific escapes, applied to the Rust + sqlx context. The result: most query code stays simple; complexity is localized to the small number of queries that genuinely need backend-specific behavior.

**Concretely:**

```rust
// In src/db/mod.rs (refactored)
pub enum DbBackend { Sqlite, Postgres }

pub struct DbConfig {
    pub backend: DbBackend,
    pub url: String,
    pub max_connections: u32,
    // ...
}

pub async fn create_pool(config: &DbConfig) -> PdsResult<AnyPool> {
    sqlx::any::install_default_drivers();
    match config.backend {
        DbBackend::Sqlite => AnyPoolOptions::new()
            .max_connections(config.max_connections)
            .connect(&format!("sqlite://{}?mode=rwc", config.url))
            .await,
        DbBackend::Postgres => AnyPoolOptions::new()
            .max_connections(config.max_connections)
            .connect(&config.url)
            .await,
    }
    .map_err(PdsError::Database)
}

// Manager structs change from:
//   pub struct AccountManager { db: SqlitePool, ... }
// To:
//   pub struct AccountManager { db: AnyPool, ... }
```

The 15 Group A files plus 1 Group B file become `AnyPool` consumers. Most queries work unchanged (SQLite's `?N` is internally converted to Postgres's `$N` by `Any`). Per-query driver-specific paths are added only where needed.

**Why not just use `AnyPool` directly without the `DbBackend` enum:** sqlx's `AnyPool::connect(url)` can theoretically infer the backend from the URL scheme (`sqlite:` vs `postgres:`). In practice, this works but loses the explicit configuration ergonomics — operators benefit from a config option that says "I want Postgres" rather than relying on URL parsing alone. The `DbBackend` enum is a small ergonomic improvement that keeps backend selection explicit at the config level.

---

## 6. Implementation phases

The work splits into five phases. Phases 1 and 2 are foundational and gating; Phase 3 is the bulk of the per-file work; Phases 4 and 5 are production-ready polish.

### Phase 1 — Postgres schema

**Goal:** Land a working Postgres schema that creates cleanly on Postgres 14+.

**Deliverables:**
1. Translate [migrations/0001_initial.sql](migrations/0001_initial.sql) to `migrations/postgres/0001_initial.sql` per the type/default/literal translation rules in §3
2. Verify the schema creates cleanly on Postgres 14, 15, and 16 (test against Docker images of each)
3. Set up sqlx-cli for Postgres migration management — `sqlx migrate run --source migrations/postgres` with `DATABASE_URL` pointing at Postgres
4. Document any non-obvious schema decisions where SQLite and Postgres diverge (e.g., `BIGSERIAL` choice rationale, JSON column type selection if any change from `TEXT`)

This phase is gating for everything else. Without a Postgres schema there's nothing for [src/db/postgres.rs](src/db/postgres.rs)'s `run_migrations` to apply.

**What's not in scope for Phase 1:** Schema redesign for Postgres-native features (JSONB, tsvector, partitioning) — keep the schema 1:1 with SQLite for v0.2, defer feature-specific Postgres optimization to later cycles.

### Phase 2 — Backend selection in config and `AppContext`

**Goal:** Make `AppContext` open the correct backend based on configuration.

**Deliverables:**
1. Add a `[database]` section to `ServerConfig`: `backend: "sqlite" | "postgres"` (default: `sqlite` for back-compat); `url` (file path for SQLite, `postgres://...` for Postgres); pool sizing parameters
2. Refactor [src/db/mod.rs](src/db/mod.rs) to wrap both [src/db/mod.rs](src/db/mod.rs)'s existing SQLite `create_pool` and [src/db/postgres.rs](src/db/postgres.rs)'s `create_pool` behind a unified entry point that returns `AnyPool`
3. Refactor [src/context.rs](src/context.rs)'s `AppContext` construction to use the new entry point — `AppContext.account_db: AnyPool` and `AppContext.did_cache_db: AnyPool` instead of concrete `SqlitePool`
4. Verify `cargo test --all-features` still passes against the SQLite path (the existing test suite is the regression check)

Per-actor `ActorStore` initialization is unchanged — always SQLite, never touched by this phase.

After Phase 2, Aurora-Locus can be configured to open either a SQLite or Postgres `account_db` and `did_cache_db`, but the manager structs still hold `SqlitePool` types — they won't compile with the new `AnyPool` until Phase 3 completes per-file. The intermediate state is "configuration plumbing in place; per-file refactoring in progress."

A note on intermediate state: Phase 3 is large enough that Phase 2 + Phase 3 may not land in a single PR cycle. The intermediate strategy is to keep the SQLite path as the default in config (so existing deployments keep working) while Phase 3 chips through individual files. Each Phase 3 sub-phase can ship independently.

### Phase 3 — Query layer compatibility

**Goal:** Convert the 16 Group A + B files to work against `AnyPool` with both backends.

This is the bulk of the work. Each file goes through:
1. Change `db: SqlitePool` to `db: AnyPool`
2. Audit all queries for SQLite-isms (boolean literals per §3.4, `datetime()` calls if any, anything else surfaced by spot-check)
3. Add Postgres-specific paths only where genuinely needed (advisory locks, `RETURNING` clauses, etc.)
4. Verify both SQLite and Postgres test paths pass for the file's logic

**Sub-phase ordering** (rough complexity, simplest first):

- **3.1** [src/identity/cache.rs](src/identity/cache.rs) — simple cache logic, smallest file. Establishes the pattern.
- **3.2** [src/admin/labels.rs](src/admin/labels.rs) — simplest admin module (~158 lines).
- **3.3** [src/admin/{reports,invites,events,roles}.rs](src/admin/) — similar admin pattern, batch as one PR.
- **3.4** [src/admin/{moderation,appeals}.rs](src/admin/) — more complex admin logic, related to each other.
- **3.5** [src/blob_store/{store,quarantine}.rs](src/blob_store/) — blob metadata and quarantine.
- **3.6** [src/oauth/{client,token_rotation,device}.rs](src/oauth/) — OAuth flow components.
- **3.7** [src/mailer/tracking.rs](src/mailer/tracking.rs) — single file, contained.
- **3.8** [src/sequencer/sequencer.rs](src/sequencer/sequencer.rs) — high-volume; verify performance characteristics on both backends.
- **3.9** [src/account/manager.rs](src/account/manager.rs) — largest, most complex; lands last after pattern is established.

Each sub-phase ships as its own chainlink issue. Order is a recommendation; sub-phases that block on each other (e.g., if admin/moderation imports patterns from admin/labels) sequence accordingly.

**Risk profile:** Medium per-file; high in aggregate. The largest risk is integration tests that don't exercise both backends — see Phase 5.

### Phase 4 — Multi-instance support (optional for v0.2)

**Goal:** Enable running multiple Aurora-Locus instances against a single Postgres backend.

**Deliverables:**
1. Use Postgres advisory locks for sequencer writer election — only one instance writes events to the sequencer at a time, others either route to the leader or stand by
2. Document the multi-instance deployment pattern in [ARCHITECTURE.md](ARCHITECTURE.md)
3. Optional: `LISTEN/NOTIFY` for cross-instance cache invalidation (DID cache, label cache)

**Risk:** Medium. New territory; the single-instance path needs to keep working unchanged, and the leader-election logic is the kind of distributed systems primitive where bugs surface in production rather than in tests.

**v0.2 inclusion:** Worth shipping if Phase 3 lands cleanly with time remaining. Defer to v0.3 if Phase 3 takes longer than expected. The Postgres backend itself is the primary v0.2 deliverable; multi-instance is the upside that operators of large deployments will want but smaller deployments don't need.

### Phase 5 — Production primitives

**Goal:** Operational tooling for Postgres-backed Aurora-Locus deployments.

**Deliverables:**
1. `aurora-locus backup --postgres` variant that calls `pg_dump` (or wraps the equivalent) for consistent snapshots
2. `aurora-locus restore --postgres` variant for restoring from a snapshot
3. WAL archiving documentation for point-in-time recovery — this is an operator-side concern (postgresql.conf settings, archive command) but Aurora-Locus's docs should reference it
4. Integration test suite that runs against real Postgres (Docker-spawned) — covers all 16 Group A + B modules end-to-end
5. CI configuration to run integration tests on both SQLite and Postgres on every commit

**Risk:** Medium. CI integration test setup is real work but well-trodden territory. The biggest risk is integration test flakiness against a containerized Postgres — needs careful handling of test database lifecycle (create/seed/teardown per test).

---

## 7. Out of scope

These are explicitly excluded from the v0.2 cycle to keep scope bounded.

**Per-actor stores on Postgres.** The hybrid model (global state on configurable backend, per-actor state always SQLite) is the architecture, not a transitional state. Migrating per-actor stores to Postgres is not part of this work and not a future direction worth pursuing.

**Wholesale ORM replacement.** Staying with sqlx; not switching to Diesel, SeaORM, or any other ORM. The `Any` driver approach is sqlx-native and the existing query code doesn't need to be rewritten against a new abstraction.

**Schema redesign for Postgres-native features.** The v0.2 schema is 1:1 between SQLite and Postgres backends (modulo the type translations from §3). Future cycles can introduce JSONB, tsvector, partitioning per-table where it pays off; not in this cycle.

**Migration tooling for live SQLite → Postgres data movement.** Operators with existing SQLite deployments who want to move to Postgres need a one-shot migration script. That's separate work, tracked as its own concern. v0.2 supports both backends from a fresh install but doesn't include the live-migration path. Operators who need it can use generic SQL dump/load tooling in the meantime, with the caveat that boolean column values need transformation (0/1 → false/true) during the dump.

**Sharding across multiple Postgres instances.** Aurora-Locus operates against a single Postgres backend; sharding across multiple Postgres clusters is not supported and not planned. If scale demands it later, it's a separate architectural conversation.

---

## 8. Pre-implementation checks

Items to verify before chainlink issue creation.

| Assumption | How to verify |
|---|---|
| sqlx 0.8's `Any` driver supports both `?N` and `$N` parameter binding correctly | Test a sample query via `AnyPool` against both Postgres and SQLite |
| Boolean column queries with `= 0` / `= 1` work on Postgres BOOLEAN | Test in Phase 1 schema verification — Postgres should reject these and require `false`/`true` |
| sqlx-cli supports separate migration directories per backend | Verify `sqlx migrate run --source migrations/postgres` against a fresh Postgres database |
| [src/actor_store/store.rs](src/actor_store/store.rs)'s per-DID `SqlitePool` lazy cache is unaffected by Workstream B | Confirm during Phase 2; the actor store should not appear in any Phase 3 PR |
| The 16 Group A + B files don't have schema-cross-cutting transactions that need redesign for Postgres semantics (different MVCC, different default isolation levels) | Spot-check during Phase 3 per file — most queries are single-table, but the sequencer and OAuth flow may have multi-table transactions worth examining |
| [src/db/postgres.rs](src/db/postgres.rs)'s existing scaffold integrates cleanly with the unified `db::create_pool` wrapper | Verify during Phase 2 — the existing dead-code module should require minimal modification to be reachable from `AppContext` |

---

## 9. Open questions

These are genuinely open and want resolution during or before Phase 1 implementation. Recommendations are stated where the doc is leaning a particular way.

### 9.1 Single Postgres database vs separate databases per surface

[src/db/postgres.rs](src/db/postgres.rs) currently takes a single `database_url`. Aurora-Locus has two shared surfaces (`account_db` and `did_cache_db`). Options:

- **Single Postgres database, schema-namespaced** — both `account_db` and `did_cache_db` point at the same Postgres database, with table prefixes or separate schemas to keep them logically distinct. Simpler operationally; one Postgres instance to manage.
- **Two Postgres databases** — separate `database_url` for each surface. More flexible (different backup policies, different scaling per surface) but doubles the operator burden.

**Recommendation:** Single Postgres database for v0.2, with the option to split via configuration in v0.3 if real demand emerges. The DID cache is small relative to `account_db` and benefits from being co-located. Splitting later is straightforward (point `did_cache_db` at a different `database_url`); merging later is hard.

### 9.2 Postgres version floor

What's the minimum Postgres version Aurora-Locus supports?

**Recommendation:** Postgres 14+. Postgres 14 ships with `BIGSERIAL`, `JSONB`, `TIMESTAMPTZ`, advisory locks, and `LISTEN/NOTIFY` — everything the v0.2 schema needs. Postgres 13 is end-of-life in 2025-Q4; targeting 14 as the floor avoids supporting an EOL version. Operators on Postgres 15 or 16 work seamlessly.

### 9.3 Connection pool sizing defaults

[src/db/postgres.rs](src/db/postgres.rs) currently defaults to `max_connections: 100, min_connections: 10`. Are these right for Aurora-Locus?

**Recommendation:** Reduce defaults to `max_connections: 25, min_connections: 5` for v0.2. 100 max connections is appropriate for high-traffic deployments but excessive for hobbyist deployments and risks exhausting Postgres's `max_connections` limit (default 100) if multiple Aurora-Locus instances run against the same database. 25 leaves headroom for other applications and matches more typical web-app defaults. Operators with high-traffic deployments can tune up via env var.

### 9.4 Test strategy: per-file SQLite tests + integration Postgres tests

Phase 5 calls for integration tests against real Postgres. Should unit tests also test Postgres, or stay SQLite-only?

**Recommendation:** Unit tests stay SQLite via `:memory:` databases — they're fast, parallelizable, and exercise the query logic. Integration tests run against real Postgres to catch backend-specific behavior (boolean literals, advisory locks, transaction isolation, etc.). The integration test suite exercises every Group A + B module end-to-end against Postgres.

This split mirrors how bsky-PDS handles its test strategy: unit tests are fast and isolated; integration tests are slower and exercise real infrastructure. Aurora-Locus benefits from the same pattern.

### 9.5 Should `[src/db/postgres.rs]` stay or be merged into `db/mod.rs`?

The existing scaffold lives in [src/db/postgres.rs](src/db/postgres.rs) with `PostgresConfig`, `create_pool`, and `run_migrations`. After Phase 2, the unified `db::create_pool` wrapper in [src/db/mod.rs](src/db/mod.rs) will dispatch to backend-specific factories. Should the Postgres factory stay as its own module file, or be inlined into `db/mod.rs`?

**Recommendation:** Stay as its own file. Postgres-specific configuration (the `PostgresConfig` struct with its env-var loading, connection-lifetime tuning, etc.) is cleanly contained in [src/db/postgres.rs](src/db/postgres.rs); inlining would bloat `db/mod.rs`. Mirror the pattern with `src/db/sqlite.rs` (extracting the existing SQLite logic from `db/mod.rs`) so both backends are visible at the same level. After this restructure, `db/mod.rs` becomes just the dispatch layer plus shared types.

---

## 10. Closing

Aurora-Locus's database architecture has more nuance than "one big SQLite database" — three surfaces (global state, DID cache, per-actor repos) with distinct access patterns warrant distinct storage choices. The Postgres backend work respects that: shared surfaces become configurable for production-grade deployments while per-actor state stays SQLite where it belongs.

The work is bounded:

- 1 schema translation (SQLite → Postgres, ~479 lines)
- 1 hybrid backend selection layer ([src/db/mod.rs](src/db/mod.rs) wraps both backends)
- 16 files convert from `SqlitePool` to `AnyPool`-backed managers
- Per-actor stores untouched

The result is an Aurora-Locus that runs comfortably as a hobbyist deployment on SQLite (preserving the existing low-friction setup story) and scales to production deployments on Postgres (enabling multi-instance writers, real backup tooling, shared cache state). Both deployment paths are first-class.

The scaffold for the Postgres path already exists in [src/db/postgres.rs](src/db/postgres.rs) — what remains is wiring, schema, and per-file refactoring. None of the phases require redesigning Aurora-Locus's existing infrastructure; they extend what's there.

Status as of this assessment: **architecture and scope identified, ready for chainlink issue creation against Phases 1, 2, and the Phase 3 sub-phases.** Phases 4 and 5 issue creation can wait until Phase 3 is mostly complete and the patterns are established.
