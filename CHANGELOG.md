# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Arc 10 — GC sweep for orphaned blob storage (v0.4-cycle)

Four-step cycle (Step 0 recon, Steps 1-4 implementation)
introducing an optional background reconciliation mechanism
for blob storage. Identifies and (operator-opt-in) deletes
storage entries with no corresponding row in the
authoritative `blob` table — the rare case where Arc 4's
`DeferredAction` queue's best-effort cleanup fails to land.

Tracked via chainlink #57. Design at
[`docs/V04_DESIGN.md`](docs/V04_DESIGN.md) §9.

The sweep is **off by default** in v0.4. Existing deployments
gain no new background task; operators opt in via
`PDS_GC_SWEEP_ENABLED=true` (env-var) or the `gc_sweep` config
section (file tier). When enabled, `dry_run: true` is the
safe default — operators promote to destructive mode only
after observing the report cadence in production.

#### Added

- **`BlobBackend::list_all_blobs(cursor, page_size) ->
  BlobListPage`** trait method exposing paginated storage
  walks. Both backends (`DiskBlobBackend`, `S3BlobBackend`)
  implement against their native primitives — `DiskBlobBackend`
  uses `tokio::fs::read_dir` with a synthesized
  `"{shard}/{filename}"` cursor; `S3BlobBackend` passes the
  S3 `ContinuationToken` through. [Arc 10 Step 1]
- **`src/blob_store/gc.rs`** — sweep primitive. Reconciles
  blob storage against `blob` and `temp_blob_metadata`;
  classifies each candidate via the two-stage `classify_blob`
  function (precedence: `Authorized > InFlight > age`);
  applies actions per `SweepParams`. Library-callable via
  `BlobStore::run_gc_sweep`. [Arc 10 Step 2]
- **Scheduled background job
  `JobScheduler::gc_sweep_job`** at configurable cadence
  (default 24h). Gated on `gc_sweep.enabled`; matches
  `temp_blob_cleanup_job`'s shape (interval-loop + structured
  tracing logs of the 9-counter report). [Arc 10 Step 3]
- **CLI subcommand `aurora-locus gc-sweep`** for operator-
  initiated one-off sweeps. Offline-only: acquires
  `LivenessLock` and fails fast if a PDS is running against
  the same DB. Five overrides (`--dry-run`, `--report-only`,
  `--max-deletes`, `--threshold-secs`, `--page-size`); no
  `--no-dry-run` (destructive mode requires editing config +
  restarting). [Arc 10 Step 3]
- **`GcSweepConfig`** config section with six fields:
  `enabled`, `interval_secs`, `dry_run`,
  `max_deletes_per_run`, `freshness_threshold_secs`,
  `page_size`. Env-var loading via `PDS_GC_SWEEP_*` prefixes;
  zero `interval_secs` and `page_size` rejected at startup;
  unparseable values surface as validation errors. New
  `parse_bool_env` and `parse_usize_env` helpers added
  alongside the existing typed parse helpers. [Arc 10 Step 3]
- **Four `validate-config` warnings** for risky `gc_sweep`
  configurations, gated on `gc_sweep.enabled = true`:
  `dry_run: false` general advisory; `dry_run: false` AND
  `max_deletes_per_run > 100000` (blast-radius);
  `freshness_threshold_secs < 600` (in-flight false-positive
  risk); `interval_secs < 3600` (cadence vs. throughput).
  [Arc 10 Step 3]
- **Three Prometheus metrics** in `src/metrics.rs`:
  `gc_sweep_orphans_found_total` (counter),
  `gc_sweep_orphans_deleted_total` (counter),
  `gc_sweep_duration_seconds` (histogram). Cap-hit and
  duration-vs-interval signals are derivable from these
  three; no separate cap-hit counter. [Arc 10 Step 2]
- **`docs/operator/blob-gc-sweep.md`** — operator-facing
  reference: what the sweep does, when to enable it, the
  mandatory dry-run shakedown procedure, both configuration
  paths, the CLI subcommand, metrics + derivable signals,
  troubleshooting, and the full configuration reference
  table. [Arc 10 Step 4]

#### Rationale

- The Arc 4 `DeferredAction` queue handles the common case
  where storage and DB go out of sync; the sweep is for
  edge cases where the queue's retries are exhausted, the
  PDS was forcibly terminated mid-cleanup, or manual
  operator action created divergence.
- Off-by-default + dry-run-default + safety-cap-default
  preserve existing deployment behavior; opt-in destructive
  mode requires explicit operator action through both
  config changes (no CLI-flag override of `dry_run` in the
  destructive direction).
- Stateless mode for v0.4 — each sweep starts fresh from the
  first storage page. Stateful mode (persistent cursor
  between runs) is a v0.6 candidate if operational
  telemetry shows probabilistic coverage is insufficient.
- Tracking-surface-driven classification per Step 0 Q9:
  `temp_blob_metadata` is authoritative for in-flight
  uploads; the 1-hour freshness threshold is belt-and-braces
  for the rare race where a row hasn't committed yet at the
  moment storage lists the CID.

#### Verification

- `cargo test --lib` — 991 passed (Arc 9 baseline 951 + 21
  `blob_store::gc::tests` + 8 `config::gc_sweep_tests` + 11
  miscellaneous Arc 10/11 additions across other modules).
- Synthetic IN-clause benchmark
  (`tests/blob_in_clause_benchmark.rs`, `--ignored`) — 1
  passed; query at `page_size=500` against 100k synthetic
  blob rows runs in ~7ms (well under the 50ms threshold).
  Plan stays `SEARCH blob USING COVERING INDEX
  sqlite_autoindex_blob_1 (cid=?)`.
- `cargo test --test distributed_substrate_test` — 11.
- `cargo test --test contract_phrases` — 14.
- `cargo test --test grant_admin_test` — 8.
- `cargo clippy --lib --no-deps -- -D warnings` — 0 errors.
- `cargo build --release --bin aurora-locus` — succeeds; no
  new warnings introduced.

#### Out of scope (v0.6 candidates)

- Stateful sweep mode with persistent cursor between runs.
- Graceful cancellation handle for the scheduled job
  (shared with the broader `JobScheduler` shutdown story).
- Live S3 integration tests against an S3 mock
  (`aws-sdk-mock` or LocalStack testcontainers).
- Promote `now: DateTime<Utc>` parameter on `run_sweep` /
  `run_gc_sweep` to full `Clock` injection if telemetry
  surfaces a need.
- `--config-show` CLI mode that prints resolved params
  without acquiring the `LivenessLock` first.
- Per-account orphan-rate metric (the current metrics are
  process-wide totals).
- Postgres-side `EXPLAIN ANALYZE` benchmark equivalent to
  the SQLite synthetic benchmark (requires
  `testcontainers` scaffolding equivalent to
  `tests/distributed_substrate_test.rs`).

### Arc 11 — Dev curl framework (v0.4-cycle)

Single-step cycle item shipping a localhost-only HTTP namespace
for development workflow. Compiled into debug builds only via
`#[cfg(debug_assertions)]`; release builds do not include the
surface and the routes do not exist on production binaries.

Tracked via chainlink #56. Pulled forward from end-of-v0.4 to
mid-cycle because Arc 9 Phase B's stop-PDS / `cargo run --
grant-admin` / restart-PDS cycle was unbearable; the new
framework collapses each admin operation to a single HTTP POST
against the running PDS.

#### Added

- **`dev.aurora.*` HTTP namespace** under
  `src/api/dev_routes.rs`, gated by `#[cfg(debug_assertions)]`
  at the module level. Five endpoints:
  - `dev.aurora.grantAdmin` — POST `{did, role, notes?}`; grant
    admin role without stopping the PDS. Routes through the
    same `AdminRoleManager::grant_role` the CLI uses, minus the
    PDS-liveness lock.
  - `dev.aurora.revokeAdmin` — POST `{did, role?, reason?}`;
    revoke an active admin grant via
    `AdminRoleManager::revoke_role`.
  - `dev.aurora.listAdmins` — GET; enumerate every
    `admin_roles` row, active and revoked, ordered by
    `granted_at DESC`. Surfaces revoked history that the
    manager's `list_active_roles` filters out.
  - `dev.aurora.createAccount` — POST `{handle, email,
    password}`; bypass handler-layer invite-code +
    email-verification gates. Preserves DB-invariant checks
    (handle/email uniqueness, password hashing, DID generation,
    repository init). Returns `accessJwt` directly.
  - `dev.aurora.mintToken` — POST `{did}`; mint a fresh
    local-session JWT. Admin authority is queried from
    `admin_roles` at request time by `AdminAuthContext` Layer 1
    (`src/auth.rs:230-332`), so a grant followed by `mintToken`
    is sufficient to get an admin-capable token without a
    `createSession` cycle.
- **Conditional router mount in `src/api/mod.rs`** under the
  same `#[cfg(debug_assertions)]` gate, with a comment block
  documenting the List C status (NEVER registered in
  `RouteRegistry`, never advertised by `describeCapabilities`).
- **`docs/internal/dev-routes.md`** — operator-facing doc with
  verified curl examples for each endpoint, a typical
  five-step workflow, and a verification recipe confirming the
  surface is absent from release builds.

#### Threat model

The `#[cfg(debug_assertions)]` gate IS the auth. Localhost
development is the trusted environment; release builds never
include the surface, so production deployment risk is zero. The
path namespace is List C by design — operators running release
builds against these paths see 404. The CLI counterpart
(`cargo run -- grant-admin`) remains the offline-only path
that holds the PDS-liveness lock; the dev HTTP surface is
explicitly for use against a running PDS.

#### Verification

- `cargo build --lib` — succeeds with dev_routes module.
- `cargo build --release --lib` — succeeds without dev_routes.
- `nm target/release/aurora-locus | grep dev_routes` — zero
  symbols (the module is stripped at compile time).
- `cargo clippy --lib --no-deps -- -D warnings` — zero errors
  (Arc 9 Step 1 baseline preserved).
- `cargo test --lib` — 951 passed (Arc 9 baseline preserved;
  Arc 11 adds no unit tests — the surface is dev-only and Phase
  B exercises validate end-to-end).

#### Out of scope (v0.6 candidates)

- `dev.aurora.inspectState` — read substrate state via HTTP.
- `dev.aurora.triggerReaper` — fire background reapers manually.
- `dev.aurora.inspectInMemory` — read in-process state
  (`DPopNonceStore.nonces`, governor counters).

### Arc 9 — Hygiene pass (v0.4-cycle)

Four-step cycle (Step 0 recon, Steps 1-4 implementation) bundling
eight items from `docs/v04-candidates.md` whose individual scope
didn't warrant separate arcs: clippy lint cleanup, `AppContext`
Debug derive, test-clock primitive for identity::cache,
`validate_config.rs` audit closure, subscribe-parity-test
closure-as-done, `AURORA_ADMIN_UI_DESIGN.md` prose audit,
`file-tier-config.md` value-format consolidation, and
`exportAccountForensic` shape rationalization.

Tracked via chainlink #55. Design at
[`docs/V04_DESIGN.md`](docs/V04_DESIGN.md) §8.

#### Added

- **`Clock` trait abstraction** (`src/identity/clock.rs`) with
  `SystemClock` (production) and `MockClock` (`#[cfg(test)]`-gated).
  Initially adopted by `identity::cache` to make
  `test_stale_handle_detection` and `test_stale_did_doc_detection`
  deterministic; the prior implementation slept against real
  wall-clock TTLs and flaked under suite-wide load. Broader
  adoption across the ~218 other `Utc::now()` call sites in `src/`
  is a v0.6 candidate. [Arc 9 Step 2, Item 12]
- **Manual `impl Debug for AppContext`** (`src/context.rs`) with
  per-field redaction. `Arc<dyn IdentityResolverApi>` and
  `Option<Arc<dyn DistributedStore>>` print as opaque
  `<dyn TraitName>` placeholders because the underlying traits
  lack a `Debug` supertrait; secret-bearing fields (`config`,
  `mailer`, DPoP/nonce stores, `local_records_cache`) print as
  opaque `<TypeName>`. The regression-gate test
  `app_context_debug_redacts_sensitive_fields` (in
  `src/api/aurora_subscribe.rs`) asserts no known sentinel
  secret value appears in the Debug output. No `derive_more`
  helper crate added. [Arc 9 Step 2, Item 8]
- **`audit_chain::audit_entry_from_row`** — shared row-to-AuditEntry
  converter used by `exportAccountForensic` to keep its
  `audit-entries.json` payload lock-step with `getAuditTrail`'s
  per-item shape. The existing `getAuditTrail` loop retains its
  inline construction (the stable contract surface stays
  byte-identical); the new parity test pins the two paths
  against each other. [Arc 9 Step 4, Item 2]
- **`schemaVersion: "2"` field** on the forensic-export bundle's
  `manifest.json`, marking the audit-entries wire-format
  migration. Consumers dispatch on this field; the binary
  always emits v2 going forward. [Arc 9 Step 4, Item 2]
- **`Per-key value formats` section** in
  `docs/operator/file-tier-config.md` documenting
  `moderation-mode` and `moderation-mode-redirect-url`
  validation rules with a four-step "Adding a new runtime
  setting" procedure tying the source-side allowlist to the
  operator-facing doc. [Arc 9 Step 3, Item 19]

#### Changed

- **`exportAccountForensic` bundle's `audit-entries.json`** now
  uses the canonical `AuditEntry` wire shape — same as
  `getAuditTrail`'s `items[]`. Previously diverged in field
  names (`id` raw-i64 → stringified, `createdAt` → `timestamp`),
  types (`snapshotId`/`eventId` raw-i64 → stringified), and
  membership (missing `subjectRef`, `verified`, `cascadeSubjects`,
  `cascadeSnapshotIds` now all present). Manifest's
  `schemaVersion` bumped to `"2"` to signal the change.
  **BREAKING** for any consumer scripted against the v1 forensic
  bundle shape. [Arc 9 Step 4, Item 2]
- **Identity-cache time source** migrated from direct
  `chrono::Utc::now()` to `Arc<dyn Clock>` injection. Production
  semantics unchanged via `SystemClock`; tests inject `MockClock`
  for deterministic TTL-boundary assertions. The six `Utc::now()`
  call sites inside `DidCache` (`get_did_doc`, `cache_did_doc`,
  `get_handle`, `cache_handle`, `cleanup_expired` ×2) now read
  from the injected clock. [Arc 9 Step 2, Item 12]
- **`SubscribeMessage::AuditEntry`'s `entry` field** changed from
  `AuditEntry` to `Box<AuditEntry>` (resolution for the
  `large_enum_variant` lint flagging the enum at ~344 B vs ~40 B
  largest peer). Wire shape preserved via serde's transparent
  `Box<T>` (de)serialization; the existing parity test
  `audit_entry_wire_shape_matches_get_audit_trail_items`
  re-confirmed after the refactor. [Arc 9 Step 1, Item 7]
- **`PaginationParams::effective_limit`** uses `.clamp(1, MAX_LIMIT)`
  instead of the prior `.min().max()` chain (clippy
  `manual_clamp`). [Arc 9 Step 1, Item 7]
- **`com.atproto.admin.updateHandle` error mapping** collapsed two
  adjacent `if matches!` arms (Validation / Conflict both → 409)
  into a single `|` pattern (clippy `if_same_then_else`). [Arc 9
  Step 1, Item 7]
- **`docs/AURORA_ADMIN_UI_DESIGN.md`**: comprehensive prose audit
  historicizing v0.2-era framing across ~20 sections. Header
  reframed to `Cycle: v0.2 (with v0.3 + v0.4 additive amendments
  — see §15 for v0.4 specifics)`. Four stale `AURORA_DESIGN.md`
  cross-references updated to `V02_DESIGN.md`. "v0.3 may add" /
  "v0.3 evaluates" framings throughout §2, §5, §6, §8.7, §9.5,
  §14 rewritten to acknowledge that v0.3 + v0.4 didn't absorb
  the items; future-cycle aspirations now route through
  `docs/v05-candidates.md`. §15 stays current. [Arc 9 Step 3,
  Item 15]

#### Removed

- **3 dead-code helper functions** in `src/api/aurora_admin.rs`
  (`require_repo_did`, `subject_uri_cid`, `require_blob_cid`),
  each superseded by `_pds` variants visible at
  `aurora_admin.rs:1018+`. The companion `subject_columns` is
  still used and stays. [Arc 9 Step 1, Item 7]
- **`docs/AURORA_DESIGN.md`** — rename-closure to
  `docs/V02_DESIGN.md`. The file had been pending deletion in the
  working tree since Arc 7's mid-cycle rename to the
  cycle-archive naming convention. Cross-references in
  `docs/AURORA_ADMIN_UI_DESIGN.md` updated to point at the
  renamed file. [Arc 9 Step 3, Item 15]

#### Fixed

- **24 clippy `-D warnings` errors cleared**: 3 `dead_code`, 1
  `manual_clamp`, 1 `if_same_then_else`, 5
  `doc_lazy_continuation` (in `aurora_admin.rs`, `registry.rs`,
  `config.rs` ×3), 1 `useless_format`, 10 `redundant_closure`
  (`|e| internal(e)` → `internal`), 1 `large_enum_variant`, 2
  `doc_overindented_list_items` (in `oauth/token.rs`).
  `cargo clippy --lib --no-deps -- -D warnings` now produces
  zero errors; no new lints introduced. [Arc 9 Step 1, Item 7]
- **`test_stale_handle_detection` flakiness** resolved by
  migrating from `tokio::time::sleep` against real-wall-clock
  TTLs to programmatic `MockClock` advancement. 10/10 flakiness
  loop passes deterministically; total runtime drops from ~22s
  to ~0.15s combined with the sibling
  `test_stale_did_doc_detection` (also migrated). [Arc 9 Step
  2, Item 12]

#### Documentation

- **`src/cli/validate_config.rs`**: audit-date comment confirming
  all 18 emitted warnings classified as still valid as of Arc 9
  Step 2. No rephrasing or removal needed. Re-audit anchor when
  major auth, federation, or storage features change. [Arc 9
  Step 2, Item 17]

#### Closure-as-done items

- **Item 14 (Subscribe parity test)**: closed as already-done.
  The existing serde-shape unit test
  `audit_entry_wire_shape_matches_get_audit_trail_items` IS the
  parity test; it has passed throughout Arc 9's cycle work
  (including across the Item 7 `Box<AuditEntry>` refactor). The
  v0.3-cycle "tooling-side issue" referenced in
  `docs/v04-candidates.md:166-169` couldn't be located in any
  tracked design corpus. If a WebSocket-integration-level
  parity test was the original intent, that's v0.6 candidate
  territory (new tokio-tungstenite + axum-test scaffolding).

#### Known limitations (v0.4)

- **`Clock` adoption is scoped to `identity::cache`**. ~218 other
  `Utc::now()` call sites in the codebase remain on direct
  wall-clock. Broader adoption is a v0.6 candidate gated by
  whether other tests show flakiness signal.
- **`getAuditTrail` retains its inline row-to-AuditEntry
  construction**. The shared `audit_chain::audit_entry_from_row`
  helper is currently used only by `exportAccountForensic`.
  DRYing `getAuditTrail` onto the helper is a v0.5+ refactor
  candidate; the new parity test pins the duplication so drift
  is caught immediately.

### Arc 8 — Runtime route enumeration (v0.4-cycle)

Four-step cycle (Step 0 recon, Steps 1-4 implementation + docs)
replacing the hand-curated `aurora_capability_families()` /
`aurora_capability_extensions()` functions with a runtime
`RouteRegistry` substrate populated during route registration and
queried by `tools.aurora.describeCapabilities` at request time.
Byte-identical wire output preserved across the migration —
single-source-of-truth advertisement that can no longer drift
from the actual route table.

Tracked via chainlink #54 (closes chainlink #123 from v0.3).
Design at [`docs/V04_DESIGN.md`](docs/V04_DESIGN.md) §7.

#### Added

- **`RouteRegistry` substrate** (`src/api/registry.rs`) with
  `RouteEntry`, `Family` enum, `FamilyKind`, `CapsBuilder`, and
  `RouteRegistryBuilder` typestate. The registry is built at
  startup, consumed by handlers at request time, and is the
  single source of truth for the capability advertisement.
  [Arc 8 Step 1]
- **`aurora_route_builder()` constructor** + `.route_with_caps()`
  / `.route()` / `.merge()` / `.build()` chain. Each
  `.route_with_caps()` call emits a `RouteEntry` alongside the
  axum `Router` registration; pass-through `.route()` registers
  without contributing a registry entry (List C routes).
  [Arc 8 Step 1]
- **`WIRE_EXTENSION_ORDER` constant** (`src/api/registry.rs`)
  pinning the capability-extension wire-output order across the
  migration. The `<kebab-family>-v<integer>` versioning contract
  doc-comment lives here (moved from the deleted
  `aurora_capability_extensions` function). [Arc 8 Step 2-3]
- **`ADMIN_TIER_PATH_REGEX` constant** —
  `^/xrpc/tools\.aurora\.(admin|moderator|superadmin|ops)(\.|$)`
  — centralized in `src/api/registry.rs` per V04_DESIGN.md
  §7.3.6's "shared-constant requirement." `admin_tier_regex()`
  returns a `&'static Regex` cached via `OnceLock`. The starting
  regex was missing `ops` (admin-tier by authority but advertised
  through the existing curated list); Step 0 Q6 added it.
  [Arc 8 Step 1]
- **`Arc<RouteRegistry>` field on `AppContext`** populated by
  `crate::api::routes()`'s builder pair and threaded through
  `AppContext::new(config, route_registry)`. Test fixtures pass
  an empty default; `api::admin::tests::create_test_context`
  passes the populated registry from `super::routes()` so the
  snapshot test exercises the real wire output. [Arc 8 Step 1-3]
- **Structural-invariant assertions on
  `test_admin_route_registry_completeness`** (renamed from
  `describe_capabilities_snapshot` per V04_DESIGN.md §7.4.4):
  the byte-for-byte literal stays in place as contract
  protection; the new structural assertions (every family
  namespace appears, extensions match `WIRE_EXTENSION_ORDER`
  element-for-element) give human-readable diagnostics when
  the registry drifts. [Arc 8 Step 4]

#### Changed

- **`describeCapabilities` handler** (`src/api/admin.rs`) now
  reads from `ctx.route_registry` via a `build_families_value`
  helper plus `RouteRegistry::advertised_extensions()`.
  Byte-identical wire output preserved (the
  `test_admin_route_registry_completeness` snapshot literal
  is unchanged across Steps 1-4). [Arc 8 Step 3]
- **`api::admin::routes()` return type** changed from
  `Router<AppContext>` to `(Router<AppContext>, Arc<RouteRegistry>)`.
  All 56 admin-tier routes use `.route_with_caps()` with
  canonical-introducer capability attribution; ≈35 List C
  routes (`com.atproto.admin.*` plus `describeCapabilities`)
  use pass-through `.route()`. [Arc 8 Step 2]
- **`api::routes()` return type** changed in lockstep to
  `(Router<AppContext>, Arc<RouteRegistry>)` propagating
  admin's registry tuple up. [Arc 8 Step 2]
- **`AppContext::new(config)`** → `AppContext::new(config,
  route_registry: Arc<RouteRegistry>)`. 8 callsites updated
  (`main.rs`, 6 in-source test fixtures, 1 integration test).
  [Arc 8 Step 2]
- **`server::build_router(ctx)`** → `build_router(ctx, api_router)`
  and **`server::serve(ctx)`** → `serve(ctx, api_router)`
  accepting the pre-built `Router<AppContext>`. The startup
  flow in `main.rs` is now: build `api::routes()` →
  `AppContext::new` with the registry → `server::serve` with
  the router. [Arc 8 Step 2]
- **`CapabilityExtension.name`** type changed from
  `&'static str` to `String` (registry returns owned strings;
  `Box::leak`-per-request would accumulate leaked memory).
  [Arc 8 Step 3]
- **Contract-phrase test** renamed
  `aurora_capability_extensions_has_versioning_pattern` →
  `wire_extension_order_has_versioning_pattern`, with the anchor
  moved from `fn aurora_capability_extensions(` in
  `src/api/admin.rs` to `pub const WIRE_EXTENSION_ORDER:` in
  `src/api/registry.rs`. Per Step 0 OQ1 disposition (b).
  [Arc 8 Step 3]
- **Direct `regex` crate dependency** (`Cargo.toml`) added for
  `ADMIN_TIER_PATH_REGEX`. The crate was already in the tree
  transitively via `tracing-subscriber`'s env-filter feature;
  the direct dep makes the substrate's use explicit. [Arc 8
  Step 1]

#### Removed

- **`aurora_capability_families()`** (~75 lines) and
  **`aurora_capability_extensions()`** (~27 lines) from
  `src/api/admin.rs`. Replaced by registry-driven generation;
  the snapshot test confirms byte-identical wire output.
  [Arc 8 Step 3]
- **`// TODO(#123, v0.4): runtime route enumeration deferred …`**
  anchors at the call sites — Arc 8 is the v0.4 cycle's
  resolution of chainlink #123. [Arc 8 Step 3]
- **`wire_extension_order_matches_curated_list_byte_identical`
  test** (the Step 2 substrate test that locked the wire-order
  constant against the curated list during the Step 1-2
  intermediate; collapsed after Step 3 removed the curated
  list). [Arc 8 Step 3]

#### Documentation

- **`docs/AURORA_ADMIN_UI_DESIGN.md` §8.15** rewritten with: the
  three-step capability-addition procedure (design-doc update
  → registry entry → `WIRE_EXTENSION_ORDER` insertion); the
  `.omitted()` flag mechanism for vocabulary-level
  intentionally-not-advertised capabilities; the admin-tier
  scope definition with the verified `ADMIN_TIER_PATH_REGEX`;
  and the representative-per-category List C rationale list
  (bsky-PDS-compat namespace, capability-registry meta-endpoint,
  public XRPC, health checks and well-known endpoints, admin
  UI static assets, public OAuth surface, internal OAuth
  bootstrap, Prometheus scrape). [Arc 8 Step 4]
- **`docs/V04_DESIGN.md` §7** cross-references to
  `AURORA_DESIGN.md §8.15` corrected to
  `AURORA_ADMIN_UI_DESIGN.md §8.15` — the file was mislabeled
  in the initial Arc 8 design prose (`AURORA_DESIGN.md` was
  renamed to `V02_DESIGN.md` mid-Arc-7). 10 occurrences fixed.
  [Arc 8 Step 4]

#### Known limitations (v0.4)

- **`RouteEntry.methods`** field exists but is left empty.
  `axum::routing::MethodRouter` doesn't expose its accepted
  methods publicly, and the current `describeCapabilities`
  wire output doesn't include methods. Populating the field
  would require either an explicit `methods: &[Method]`
  parameter at every `.route_with_caps()` call site or
  upstream axum changes. v0.6 candidate.
- **Method-name extraction** in `build_families_value` uses
  `path.rsplit('.').next()` to pull the trailing segment from
  `/xrpc/<namespace>.<method>` paths. The fallback returns the
  raw path; the snapshot test catches any future deviation
  from the `<namespace>.<method>` shape loudly rather than
  silently shipping a malformed wire entry.

### Arc 7 — Multi-instance auth state + rate limiting (v0.4-cycle)

Four-step cycle (Step 0 recon, Step 0.6 schema, Steps 1-4
implementation + docs) introducing the `DistributedStore`
trait substrate so Aurora-Locus's per-request authentication
state (DPoP JTI replay) and rate-limit buckets become
cross-instance-coherent. Backed by Postgres-CAS; the existing
in-process governor + in-memory JTI tracker remain functional
as the `single_instance_inmemory` opt-out and as
defense-in-depth in the default `distributed` mode.

Tracked via chainlink #53. Design at
[`docs/V04_DESIGN.md`](docs/V04_DESIGN.md) §6.

#### Added

- **`DistributedStore` trait abstraction**
  (`src/distributed/mod.rs`) with five async methods —
  insert / get / delete / cas / reap_expired — and three
  error variants (`KeyExists`, `UnsupportedTable`,
  `Database`). The trait is the consumer contract; backends
  plug in behind it. [Arc 7 Step 1]
- **`Lease` primitive** (`src/distributed/lease.rs`) —
  epoch-ms BIGINT-backed expiry abstraction matching the
  schema's portable arithmetic. saturating_add on
  construction defeats i64 overflow on extreme durations.
  [Arc 7 Step 1]
- **`PostgresCasStore`** (`src/distributed/postgres_cas.rs`)
  — Postgres-CAS substrate implementation via `sqlx::Any`
  so a single backend serves SQLite (dev) and Postgres
  (production) deployments. Per-table dispatch over
  `dpop_jti_replay` and `rate_limit_buckets`. Backend-
  specific unique-violation detection centralized in one
  helper. [Arc 7 Step 1]
- **`TtlCache` parse-result optimization layer**
  (`src/distributed/cache.rs`) — dashmap-backed concurrent
  cache for cryptographic parse caching per V04_DESIGN.md
  §6.3.4. Built but not yet wired to a consumer; ready for
  v0.6 DPoP parse-caching work. [Arc 7 Step 1]
- **`OAuthFlowStateAdapter`**
  (`src/oauth/flow_state_adapter.rs`) — sibling
  `DistributedStore` impl wrapping the existing
  `authorization_request` table without schema change. The
  trait's opaque value parameter is JSON-encoded
  `AuthorizationRequestData` on insert and
  `AuthorizationRequest` on read. [Arc 7 Step 2]
- **`DistributedStoreRegistry`**
  (`src/distributed/registry.rs`) — per-table dispatch
  facade implementing `DistributedStore`, routing consumers
  to the right impl by table name. Substrate is `Option`
  (skipped in `SingleInstanceInmemory` mode); OAuth adapter
  is mandatory (table lives in `account_db` regardless of
  mode). [Arc 7 Step 2]
- **`DistributedRateLimiter`** in `src/rate_limit.rs` —
  cross-instance rate-limit primitive built on the §6.3.5
  atomic UPDATE-with-arithmetic pattern. First-touch INSERT
  fallback with bounded retry on PK-collision races.
  Portable CASE-WHEN SQL (no Postgres-only `LEAST`).
  [Arc 7 Step 3]
- **`dpop_jti_replay` and `rate_limit_buckets` tables**
  (migration `0007_distributed_state.sql` + Postgres twin).
  Schema stays within `sqlx::Any`'s portable subset —
  TEXT primary keys, BIGINT epoch-millis timestamps. [Arc 7
  Step 0.6]
- **`PDS_DISTRIBUTED_STATE_MODE` config enum** —
  `distributed` (default), `single_instance_inmemory`,
  `redis` (forward-compat slot; rejected at startup).
  [Arc 7 Step 1]
- **Maintenance pool sizing env vars** —
  `PDS_MAINTENANCE_DB_MAX_CONNECTIONS` (default 15),
  `PDS_MAINTENANCE_DB_MIN_CONNECTIONS` (default 2),
  `PDS_MAINTENANCE_DB_ACQUIRE_TIMEOUT_SECS` (default 10).
  Dedicated pool isolates substrate load from the main
  application pool. [Arc 7 Step 1]
- **Three background reapers** in
  `src/jobs/mod.rs:JobScheduler::start`:
  `dpop_jti_replay_reaper_job` (300s),
  `oauth_authorization_request_cleanup_job` (300s, fold-
  in of pre-existing-but-unwired sweeper from Step 0 Q1
  finding), `rate_limit_buckets_reaper_job` (3600s, 7-day
  inactivity threshold). [Arc 7 Steps 1, 2, 3]
- **`docs/operator/multi-instance-deployment.md`** —
  485-line operator guide covering when multi-instance
  makes sense, Postgres prerequisites with connection-
  budget worked example, configuration env-var inventory,
  migration path, verification smoke checks, monitoring
  guidance, six known v0.4 limitations, and troubleshooting
  for the most common operator surprises. [Arc 7 Step 4]
- **`tests/distributed_substrate_test.rs`** — 11 cross-
  instance integration tests against testcontainers
  Postgres covering JTI replay rejection, OAuth state
  visibility + consume-and-replay-rejection, rate-limit
  exhaustion across instances, concurrent first-touch
  race resolution, and reaper sweeps visible to siblings.
  [Arc 7 Steps 1-3]

#### Changed

- **OAuth handlers route through the `DistributedStore`
  trait**: `src/oauth/authorize.rs:create_authorization_request`
  and `get_authorization_request` now call
  `store.insert / get` for cross-instance-relevant
  operations. `mark_code_as_used` in
  `src/oauth/consent.rs` does a secondary-key lookup
  (direct SQL, unchanged) then routes the consume through
  `store.delete`. The trait's atomic UPDATE-with-predicate
  IS the cross-instance single-use guarantee for OAuth
  code redemption. [Arc 7 Step 2]
- **DPoP JTI replay routes through the trait in
  `distributed` mode**: `DPopNonceStore.check_and_record_jti`
  in `src/federation/dpop.rs` calls
  `store.insert("dpop_jti_replay", ...)` when configured;
  `KeyExists` translates to "replay". In
  `single_instance_inmemory` mode the pre-Arc-7
  `HashMap<String, i64>` path runs unchanged.
  `check_and_record_jti`'s signature widened from
  `(jti, exp)` to `(jti, jkt, exp)` so the substrate's
  `jkt` observability column is populated; the verifier
  computes the JWK thumbprint earlier so it can be
  passed through. [Arc 7 Step 3]
- **Rate-limit middleware adds a PRIORITY-0 distributed
  pre-check**: in `distributed` mode the middleware runs
  the substrate's `try_consume` against a per-endpoint
  bucket BEFORE the existing governor's PRIORITY-1+
  checks. Returns 429 directly on substrate denial; falls
  through with a `tracing::warn!` on substrate-consult
  failure (non-fatal — request continues via the
  governor's per-instance defense). [Arc 7 Step 3]
- **Migration `0007_distributed_state.sql` retroactively
  cites chainlink #53** in its header comment, matching
  the existing `chainlink #NNN` convention from migrations
  0005 and 0006. [Arc 7 Step 1]
- **Background-task scheduling**: three new reapers
  spawned through the existing `JobScheduler::start`
  pattern, matching `dpop_nonce_cleanup_job`'s shape.
  No new shutdown-handling infrastructure; reapers run
  for process lifetime like the rest of the existing
  background tasks. [Arc 7 Steps 1-3]
- **`get_authorization_request` collapses "expired" and
  "not found" into a single `NotFound`**. Pre-Arc-7 the
  function raised separate `Authentication("Authorization
  request expired")` for the expired case; the trait's
  `get` filters lease-expired rows as `None` and the
  handler collapses both into 404. Documented in the
  function body — consent screen treats both identically.
  [Arc 7 Step 2]

#### Fixed

- **Pre-existing `authorization_request` schema/model
  mismatch surfaced and worked around**: the
  `AuthorizationRequest` model declared `id: i64` and
  `code_used_at: Option<DateTime<Utc>>` fields, but the
  `0001_initial.sql` migrations never created backing
  columns. Step 0 recon copied the model verbatim and
  missed the inconsistency; the existing direct-SQL paths
  that SELECTed those columns would have failed on
  Postgres but were latent because the pre-Arc-7 test
  suite never exercised them against real Postgres.
  Step 2's testcontainers tests are the first to hit it.
  Fixed in-scope (no schema migration per Step 2
  kickoff): the adapter's `get`/`delete` and
  `consent.rs:get_request_by_code` SELECT/UPDATE only
  the columns that actually exist; the model fields are
  populated with synthetic defaults (`0` / `None`). The
  dead model fields stay for API compat — no consumer
  reads them. v0.6 model audit should remove them.
  [Arc 7 Step 2]

#### Removed

- **`src/rate_limit_new/distributed.rs`** (pre-existing
  Redis-backed `DistributedRateLimiter`, 276 lines) —
  retired per Step 1 disposition. Was wired through
  `AppContext.distributed_rate_limiter` but marked
  `#[allow(dead_code)]` and never consulted by the
  rate-limit middleware. Replaced by Arc 7's trait
  surface; the `DistributedStateMode::Redis` enum
  variant preserves the forward-compat door for a future
  cycle's clean Redis backend against the trait.
  [Arc 7 Step 1]
- **`RateLimitConfig.use_redis` and `RateLimitConfig.redis_url`
  fields** plus the corresponding
  `PDS_RATE_LIMIT_USE_REDIS` / `PDS_RATE_LIMIT_REDIS_URL`
  env-var reads — only consumers were the now-deleted
  rate_limit_new module. [Arc 7 Step 1]
- **`crate::oauth::authorize::cleanup_expired_requests`
  function** — only caller was the JobScheduler, which
  now routes through `store.reap_expired("oauth_flow_state",
  ...)` directly. [Arc 7 Step 2]

#### Known v0.4 limitations (v0.6 candidates)

- Distributed rate-limit defaults hardcoded
  (100 tokens / 100 tokens/sec). Per-endpoint
  configurability TBD.
- 7-day `rate_limit_buckets` retention hardcoded.
  Operator-tunable threshold TBD.
- DPoP server-side nonce issuance stays in-memory
  (federation `getDpopNonce` endpoint). Substrate's DPoP
  scope is JTI replay only; the `dpop_jti_replay` table
  name reflects this.
- No DPoP parse-result cache wired in v0.4. `TtlCache`
  primitive is in place from Step 1 (`src/distributed/cache.rs`);
  no consumer yet. Deferred pending profiling.
- No dedicated Arc-7 Prometheus metric families
  (`aurora_distributed_store_operations_total`,
  `aurora_distributed_store_latency_seconds`,
  `rate_limit_substrate_fallthrough_total`). Monitoring
  uses existing `background_jobs_*` and
  `db_query_duration_seconds`; substrate-consult
  fall-through is via `tracing::warn!` only.
- Redis backend implementation deferred. Enum slot
  reserved; setting `PDS_DISTRIBUTED_STATE_MODE=redis`
  fails fast at startup.
- `AuthorizationRequest.id` and `code_used_at` vestigial
  model fields with no backing schema columns. Removing
  is a model audit; would touch every fixture and any
  external consumer that deserializes the JSON.

### Arc 6 — Aurora Admin UI v0.3 migration (v0.4-cycle)

Eight-step migration of the admin UI to v0.3 wire shapes, with
modal consolidation, role-management action UI, CLI sentinel
handling, and backend-side dual-shape observability. Arc 6 is
the v0.4 cycle's headline arc.

#### Added

- **`AuroraErrorTranslations` module** at
  `static/admin/scripts/api/error-translations.js`. Server
  structured-error-code → operator-friendly prose translation
  consumed by `client.js`'s 4xx rendering path. Seeded with the
  four v0.3 codes (`SubjectVariantMismatch`,
  `SubjectTargetMismatch`, `OrphanedAppeal`,
  `SubjectsArrayInvalidForAction`). [Arc 6 Step 1]
- **`AuroraModal.form` + `AuroraModal.destructiveConfirm`**
  static helper API on the existing modal substrate. Promise-
  returning; supports text/password/textarea/checkbox/select
  field types; live validation; typed-confirm gates; required
  rationale; ack checkboxes. [Arc 6 Step 4]
- **`chainVerified` indicator** on the audit page header with
  three-state semantics (✓ verified through entry M / ⚠
  verified through M, failure at M+1 / ✗ failed at entry 1)
  and click-to-expand inline detail panel surfacing the
  chain-walk CLI suggestion. [Arc 6 Step 3]
- **`cascadeSnapshotIds` cascade-subjects rendering** on
  audit-entry detail. Subjects route via
  `AuroraEntityRef.fromSubject`; snapshot ids render as inline
  `<code>`. Section omitted entirely when the entry has no
  cascade. [Arc 6 Step 3]
- **`subject_cid` filter** on the audit list. [Arc 6 Step 3]
- **`timeRange` preset dropdown** on the Dashboard moderation-
  metrics card (`last_hour` / `last_24h` / `last_7d` /
  `last_30d`). [Arc 6 Step 3]
- **`auditEntryId` toast click-through** on 11 success toasts.
  `AuroraToast` API extended with optional
  `action: { label, href }` argument; clicking navigates to
  `#mod/audit/<id>` via the existing hash router. [Arc 6 Step 3]
- **CLI sentinel rendering** for `cli:`-prefixed actor strings:
  non-clickable badges across all seven actor-rendering
  surfaces, applied via a single `EntityRef.account()` patch.
  [Arc 6 Step 6]
- **Role-grant affordance** on `SettingsRoles.js` and
  `SettingsRolesMembers.js`; both consume the new
  `AuroraModal.form` substrate with `did:` prefix validation +
  audit-entry click-through toast. [Arc 6 Step 5]
- **Role-revoke flow with canonical destructive-confirm**:
  typed-confirm gate `"REVOKE"`, required rationale,
  audit-entry click-through. The role-revoke is the canonical
  example in V04_DESIGN §5.3.3. [Arc 6 Steps 4 + 5]
- **Dual-shape acceptance** on backend admin endpoints:
  - `tools.aurora.admin.emitEvent` accepts both canonical v0.3
    `subjects: Vec<Subject>` and legacy v0.2
    `subject: Subject`.
  - `com.atproto.admin.updateSubjectStatus` accepts both
    canonical `record_uri` (snake_case) and legacy `recordUri`
    (camelCase) on the `RepoBlobRef` subject variant.

  Both reject requests sending both shapes simultaneously with
  a 400 + explicit error. [Arc 6 Step 7]
- **Metrics counter `aurora_legacy_wire_ingest_total`** with
  labels (endpoint, shape, field). Operators query Prometheus
  to track per-field migration progress. [Arc 6 Step 7]
- **Structured tracing event** `legacy_wire_shape_ingested`
  at INFO level with endpoint/shape/field structured fields.
  [Arc 6 Step 7]
- **JWT-deprecation middleware wired into the router stack.**
  Previously defined but never registered as a layer; counter
  + headers never fired. Step 8 wires it and replaces the
  broken extractor-extension detection with structural
  Authorization-header inspection (`token_looks_like_jwt`).
  [Arc 6 Step 8]
- **Operator doc**
  [`docs/operator/v03-wire-deprecation-rollout.md`](docs/operator/v03-wire-deprecation-rollout.md):
  dual-shape rollout reference for operators with custom UI
  builds or third-party admin tooling. [Arc 6 Step 7]
- **Operator doc**
  [`docs/operator/running-ui-tests.md`](docs/operator/running-ui-tests.md):
  how to run the admin UI test suite under bare Node ≥ 18.
  Resolves the "harness invocation isn't documented" friction
  that recurred across Arc 6 Steps 2-4. [Arc 6 Step 5]
- **`AURORA_ADMIN_UI_DESIGN.md` §15**: additive prose audit
  documenting Arc 6 changes against the v0.2-era reference
  doc. [Arc 6 Step 8]

#### Changed

- **13 native `confirm()` / `prompt()` call sites migrated** to
  `AuroraModal` helpers per the V04_DESIGN §5.3.3 classification
  (7 destructive → `destructiveConfirm`; 3 form-input → `form`;
  3 non-destructive yes/no → `form` with zero fields). The
  Sequencer.js generic dispatcher path converted blanket; the
  AccountDetail delete-account flow collapses a two-step
  prompt+confirm into a single typed-gated modal. [Arc 6 Step 4]
- **`SettingsGeneral` + `SettingsUiModes`** source-tier
  rendering: all four `SettingSource` values
  (Runtime / File / Default / RecoveryMode) render with
  informational suffixes via a shared `settingSourceSuffix()`
  helper. Runtime renders bare; the others get muted-italic
  `(default)` / `(file)` / `(recovery override)` tags. [Arc 6
  Step 2]
- **`BulkActionPanel` `MAX_BATCH_SIZE`** switched from singleton
  constant to per-action lookup
  (`{ DeleteAccount: 10, DeleteBlob: 25, default: 50 }`).
  `currentMaxBatchSize()` follows the selected action; existing
  bulk actions fall through to `default` → 50, preserving
  pre-Arc-6 behavior. [Arc 6 Step 2]
- **`ActionPanel.js` payload construction** now emits
  `subjects: [this.subject]` (v0.3 canonical) rather than
  `subject: this.subject` (v0.2 legacy). [Arc 6 Step 2]
- **`AuroraToast.show()` API** gained an optional
  `opts.action: { label, href }` argument. `isSafeActionHref`
  guard rejects non-same-origin hrefs defensively. [Arc 6
  Step 3]
- **JWT-deprecation middleware detection logic**: replaced the
  unworking `req.extensions().get::<AuthMethod>()` read (which
  was always empty pre-`next.run` and unreachable post-) with
  structural `token_looks_like_jwt(token)` Authorization-header
  inspection. [Arc 6 Step 8]

#### Removed

- **Dead `failures` field reading** in admin batch-handler
  response consumers. v0.3's Arc 4 Step 2 made batch handlers
  atomic (all-or-nothing); the `failures` field is no longer
  emitted on success responses. UI had no live consumer to
  remove (one false-positive grep hit on
  `tools.aurora.ops.getValidationFailures`, unrelated).
  [Arc 6 Step 2]
- **`affected_count` partial-success rendering branch** in
  `BulkActionPanel`. v0.3 atomic-batch semantics mean
  `affectedCount` is now "total subjects processed"; the prior
  `'Affected N subject(s), M skipped'` rendering with its
  `r.skipped` array branch is dead. Reworded to
  `'Processed N subject(s)'`. [Arc 6 Step 2]
- **Inverted-OK-Cancel toggle** in `AccountDetail.toggleInvites`:
  the prior native `confirm()` had OK to disable / Cancel to
  enable, a routinely-misread cognitive trap. Replaced with an
  explicit select-field `AuroraModal.form` where the operator
  picks the target state directly. [Arc 6 Step 4]

#### Deferred

- **Response-header emission** for legacy wire shapes
  (`Deprecation`, `Sunset`, `Warning`, `X-Wire-Migration-Guide`).
  The `emit_legacy_wire_headers()` helper substrate is in place
  at `src/api/middleware.rs`; wiring requires restructuring
  handler return types from `Json<EmitEventOutput>` to
  `Response`, which ripples through ~43 pre-existing test call
  sites across `emit_event` and `update_subject_status`. Counter
  + structured tracing log alone meet the operator-side
  observability goal of V04_DESIGN §5.3.6; deferred to v0.5+
  (federation-aligned since federated PDS consumers of those
  endpoints benefit from the client-side deprecation signal).
  Documented in
  [`docs/operator/v03-wire-deprecation-rollout.md`](docs/operator/v03-wire-deprecation-rollout.md).
  [Arc 6 Step 7]
- **Dual-link audit-trail UX on role-tier cards** (V04_DESIGN
  §5.3.2 option (c)). Requires extending `Audit.js` to parse
  hash query params on mount or extending the router to surface
  query params in route-match results — both substrate
  additions beyond Arc 6's scope. Carryover to v0.5+. [Arc 6
  Step 5]
- **Chain-indicator detail panel migration to AuroraModal**.
  Currently inline-expansion; migrating to `AuroraModal.form`
  would require extending `form` to accept Node body (chain-
  indicator detail has HTML content with code blocks). Two
  substantive changes, not one — deferred. [Arc 6 Step 4]
- **Backend error shape for `grant_role` / `revoke_role`**:
  handlers return `(StatusCode, String)` plain text rather
  than structured JSON, so Step 1's translation layer can't
  match them on those endpoints. Reshape is its own
  wire-shape work; carryover to v0.5+. [Arc 6 Step 5]
- **CI integration for UI tests**: the admin UI test suite
  (Node `node:test`, 12 tests, ~250ms total) is not invoked
  by `.github/workflows/ci.yml`. Low-cost to add; flagged for
  cycle-close audit. [Arc 6 Step 5]

_Future v0.4 cycle work lands here. See
[`docs/v04-candidates.md`](docs/v04-candidates.md) for the
named deferrals from v0.3 and the running candidate
accumulator._

## [0.3.0] - 2026-05-10

### Documentation
- **`ModEventAction` flat-shape commitment** (#125, Arc 5
  Step 4). The 16-variant flat enum is the v0.3 committed
  contract; compositional reshape (separating action-verb from
  subject-type into orthogonal axes) is a v0.4-or-later
  candidate gated on use-case surface. Subject set is a peer
  axis at the request level (per Arc 4's multi-subject
  `emitEvent`), not an enum-internal axis. Aurora Admin UI and
  third-party tooling can build switch tables on the
  discriminator without anticipating a structural reshape
  during the v0.3 series. Documented in `docs/AURORA_DESIGN.md`
  §4.1.2; cross-referenced from `docs/v04-candidates.md`.
- **#123 LB-3 runtime route enumeration handoff to v0.4**
  (Arc 5 Step 4). `tools.aurora.admin.describeCapabilities`
  continues to advertise a hand-curated capability list, with
  the v0.2 reconciliation conclusion (manual list stays)
  carried forward. The drift-detection test at
  `src/api/admin.rs:7223-7331`
  (`describe_capabilities_snapshot`) ensures any new route not
  in the hand-curated list fails CI. Code-level anchors at
  `src/api/admin.rs:2939` and `:3041` (immediately preceding
  `aurora_capability_families` and `aurora_capability_extensions`
  respectively) carry `TODO(#123, v0.4)` comments for v0.4
  discoverability. Full handoff at `docs/V03_DESIGN.md` §9.8;
  v0.4 candidate accumulator at `docs/v04-candidates.md`.
### Changed
- **`TimeRange` wrapper + selective `u32` retype** (#126). Three
  bundled changes close the v0.3 cycle's TimeRange + numeric-typing
  cleanup:

  - **`TimeRange` newtype** (`crate::admin::TimeRange`). New
    validated `(start, end)` primitive constructible from either
    a preset name string (`"last_hour"`, `"last_24h"`, `"last_7d"`,
    `"last_30d"`) or an explicit `{start, end}` object with RFC
    3339 timestamps. The wrapper rejects inverted ranges
    (`start > end`) at deserialize time so handlers can trust the
    value without re-validating; equal start/end is allowed
    (zero-duration ranges are valid empty queries). Validation is
    centralized — handlers no longer carry ad-hoc range checks.

  - **`getModerationMetrics` request shape**. The handler accepts
    both wire shapes per Arc 5 §9.4.3's backward-compat
    requirement, dispatched via a custom `Deserialize` on the
    request struct (recon Q3(b) decision):
    - **Canonical**: `timeRange` field carrying a preset name.
    - **Legacy**: peer `start` and `end` RFC 3339 timestamp fields
      (the v0.2 wire shape).
    Exactly one shape per request. Mixed (`timeRange` plus
    `start`/`end`) and missing-both error envelopes name the
    canonical field FIRST and surface preset alternatives —
    typo'd preset names produce errors mentioning `timeRange`,
    NOT misdirecting toward the legacy fields. The §9.5.9
    untagged-enum misdirection risk is mitigated by the explicit
    dispatcher.

  - **`GetQueueStatsOutput` selective `u32` retype**. Six count
    and age fields (`open_reports`, `pending_appeals`,
    `under_review_reports`, `under_review_appeals`,
    `average_age_open_reports_seconds`,
    `oldest_open_report_age_seconds`) retype from `i64` to `u32`
    per recon Q4 — domain-safely non-negative AND bounded < 2^32
    (a u32 seconds counter spans ~136 years; ample for any
    realistic age). `queue_attention_total` stays `i64` to
    preserve the sum-overflow guard. The handler converts SQL
    `i64` reads to `u32` via a saturating helper. JSON wire shape
    unchanged (still emitted as non-negative integers); strict-
    typed Rust consumers gain the narrower type. No
    generated-client impact (Aurora-Locus has no OpenAPI/JSON-
    Schema codegen consumer per recon Q4).

  Operator note: existing v0.2 `getModerationMetrics` callers
  continue to work without modification. New callers should
  prefer the canonical `timeRange` field.

### Added
- **File-tier runtime configuration** (#124). Aurora-Locus
  resolves `runtime_settings` keys through a four-tier
  hierarchy from highest to lowest precedence: recovery-mode
  env-var override (`AURORA_RECOVERY_MODE`, `moderation-mode`
  reads only), runtime row, **file-tier YAML**, compiled-in
  default. The new file tier sits between operator runtime
  control and the compiled-in defaults — load-once-at-startup
  YAML at `<data_directory>/runtime.yaml` (override via
  `PDS_RUNTIME_FILE`) for deployment-stable values that don't
  need the runtime API surface. Unknown keys (vs.
  `KNOWN_RUNTIME_KEYS`) and invalid per-key values
  warn-and-skip; malformed YAML produces a clear startup error
  with the file path. The `getRuntimeSetting` response's
  `source` field gains a fourth value, `"File"`, distinguishing
  file-tier-resolved reads from runtime/default. The field
  becomes a typed `SettingSource` enum with a custom
  `Serialize` impl emitting the existing string literals —
  wire-additive, no contract amendment (the `source` field is
  open per Arc 2's contract framing). New dependency
  `serde_yaml = "0.9"`. Operator setup at
  `docs/operator/file-tier-config.md`. Reload-on-SIGHUP is a
  v0.4 follow-up; `setRuntimeSetting` remains the in-process
  hot path for setting changes.

### Removed
- **`PDS_ADMIN_DIDS` configuration** (#155). The `admin_dids`
  field on `AuthConfig` and the `PDS_ADMIN_DIDS` env-var parsing
  in `src/config.rs` are removed. Admin authority is gated solely
  by the `admin_role` table (per #95); the dead config and its
  `validate_config` warning ("admin panel will not be accessible")
  predated #95 and gave operators incorrect guidance — a populated
  `admin_dids` list never conferred admin authority on its own.
  Operators with `PDS_ADMIN_DIDS` set in their environment should
  remove the variable; it's no longer read. The first SuperAdmin
  is bootstrapped by inserting a row directly into `admin_role`
  (see README "First Admin User"); subsequent grants flow through
  `tools.aurora.superadmin.grantRole` and the audit chain.

### Documentation
- **`AURORA_DESIGN.md` S3 framing corrected** (#156). Two stale
  lines in `docs/AURORA_DESIGN.md` claimed S3 backend support
  "has not landed in v0.2" and listed S3 activation as deferred
  to v0.3. S3 actually shipped in v0.2: AWS SDK dependencies are
  live, `src/blob_store/s3.rs` is exported from
  `src/blob_store/mod.rs`, and `AppContext` selects between Disk
  and S3 via `BlobstoreConfig` from `PDS_BLOBSTORE_*` env vars.
  §2.2's "Status post-cycle" and §8.2's deferred-to-v0.3 entry
  are updated to reflect the as-built reality.

### Changed

- **Wire-format breaking change (v0.3 / Arc 4 multi-subject + atomicity unification).**
  Five intertwined cycle changes ship together:

  - **`emitEvent` multi-subject reshape** (chainlinks #122, #130).
    `tools.aurora.admin.emitEvent` accepts `subjects: Vec<Subject>`
    on input and returns `snapshots: Vec<SnapshotRef>` paired
    1:1-by-index. Single-subject callers migrate by wrapping in a
    one-element array (`subjects: [s]`); multi-subject callers fan
    out across the supported action vocabulary: account state
    (`TakedownAccount`/`SuspendAccount`/`RestoreAccount`/`DeleteAccount`),
    label (`ApplyLabel`/`RemoveLabel`), blob lifecycle
    (`QuarantineBlob`/`RestoreBlob`/`DeleteBlob`), record takedown
    (`TakedownRecord`), and `UpdateSubjectStatus`. Embedded-id
    variants (`ResolveReport`, `DismissReport`, `ResolveAppeal`,
    `EscalateAppeal`) and `SendEmail` reject `subjects.len() > 1`
    with HTTP 400 `SubjectsArrayInvalidForAction`. Per-action
    `MAX_BATCH_SIZE` caps: `DeleteAccount` = 10 (irreversible),
    `DeleteBlob` = 25 (storage-irreversible), all others = 50.
    `dispatch_action` is fully tx-bound — every match arm runs
    via `_in_tx` manager variants inside the wrapping tx, so
    per-subject mutation failure aborts the whole tx atomically
    with the chain entry.

  - **Whole-batch atomicity across all `batch*` handlers**
    (#113). The `failures: Vec<BatchFailure>` field is
    removed from `BatchAccountsOutput`, `BatchLabelOutput`, and
    `BatchRemoveLabelOutput`; the v0.2 `BatchFailure` struct is
    retired. `batch_takedown_accounts` and `batch_restore_accounts`
    drop their per-subject SAVEPOINT recovery patterns in favor of
    `?`-propagation through the wrapping tx. Every batch handler
    now has the same atomicity contract: chain entry,
    `moderation_event`, and per-subject mutations either ALL land
    or NONE do. `affected_count` always equals
    `cascade_subjects.len()` for successful responses; the
    partial-success state is no longer observable. Errors surface
    the failing subject's index and identifier in the response
    body (`failingSubject`, `failingSubjectId` keys).
    `batch_remove_label` keeps its `skipped: Vec<Subject>` field —
    no-op, semantically distinct from a failure. Affected operator
    endpoints: `batchTakedownAccounts`, `batchSuspendAccounts`,
    `batchRestoreAccounts`, `batchTakedownRecords`,
    `batchApplyLabel`, `batchRemoveLabel`.

  - **`BlobQuarantine`, `BlobStore`, `AppealManager` `_in_tx`
    variants** (#131). Step 0.5 introduced the missing
    in-tx manager methods so every `dispatch_action` arm has a
    tx-bound execution path. `update_subject_status`'s
    release-reopen-tx pattern collapses to a single in-tx call.
    `DeleteBlob`'s backend storage delete runs post-commit via the
    `DeferredAction` queue; orphan storage on backend-delete
    failure is accepted (future GC sweep tracked as v0.4 follow-up).

  - **Chain row shape commitment** (per §8.3.3). Single-subject
    events now populate BOTH the flat
    `subject_did`/`subject_uri`/`subject_cid` columns AND
    `cascade_subjects: [s]`; multi-subject events use
    synthetic-primary (NULL flat columns, populated cascade).
    External consumers can rely on `cascade_subjects` always
    containing every subject regardless of arity. Pre-Arc-4
    chain rows (with empty cascade on single-subject events)
    remain valid and verifiable; the mixed-corpus state is
    expected. `docs/operator/audit-chain-verification.md`
    Section D worked examples and the side-script's deterministic
    hashes are updated to reflect the new shape.

  - **URI-level record-takedown semantics for
    `tools.aurora.admin.batchTakedownRecords`** (Arc 4 §8.4.3).
    Cascade entries carry empty-string CIDs by deliberate
    convention; the URI is the identifying field and the takedown
    covers all versions of the record at that URI. Single-subject
    `emitEvent{TakedownRecord}` retains CID-level semantics
    (specific record version, real CID populated). Operators
    choosing between paths select based on whether they need
    version-specific or URI-level coverage. Documented at
    `BatchRecordsInput` and the `Subject::Record` variant doc
    comments, pinned by
    `batch_takedown_records_produces_uri_level_cascade_with_empty_cids`.

  Stability commitment: `tools.aurora.admin.emitEvent` is the
  sixth committed surface under the v0.3 contract-lockdown
  framework, pinned by the doc-comment on `EmitEventOutput`
  (literal phrase "emitEvent multi-subject contract is
  committed"). Drift caught by `tests/contract_phrases.rs`
  (seventh phrase added). Operator summary at
  `docs/operator/contract-stability.md` (now six committed
  surfaces).

### Added
- **Arc 3: Audit-trail read contract.**
  `tools.aurora.admin.getAuditTrail` is committed as the fifth
  stability surface under the v0.3 contract-lockdown framework.
  The endpoint exposes `cascadeSnapshotIds` on the wire
  (load-bearing for independent chain verification per
  `docs/operator/audit-chain-verification.md`), ships a
  seven-filter set (`actor_did`, `action`, `subject_did`,
  `subject_uri`, `subject_cid`, `after_created`, `before_created`)
  with `subject_cid` newly added in this cycle, and pins
  pagination/verification semantics via doc-comment commitment
  on `GetAuditTrailOutput` (literal phrase
  "audit-trail read contract is committed"). The wire-to-canonical
  bridge documentation enables external consumers to recompute
  SHA-256 hashes independently from response data, with all
  transformation rules verified against production behavior by
  `tests/audit_chain_canonical_verification.rs` (six worked
  examples with reproducible hashes plus seven production-roundtrip
  tests). Drift caught by `tests/contract_phrases.rs` (sixth
  phrase added) and the structural lint from Arc 2.

  Operator summary at `docs/operator/contract-stability.md`
  (now five committed surfaces).

- **Arc 2: Contract lockdown.** Aurora-Locus v0.3 commits to four
  stability contracts on its admin-and-capability surfaces:
  Subject vocabulary stability (canonical Aurora `Subject` and
  createReport `ReportSubject` — two distinct surfaces;
  internally-tagged for the former, untagged-at-the-enum-level for
  the latter), `describeCapabilities` response shape stability,
  capability string versioning convention
  (`<kebab-family>-v<integer>`), and action-ID surfacing
  (`auditEntryId` and optionally `eventId` on Aurora-namespace
  handlers writing audit chain entries). Each contract is committed
  in a doc comment at the canonical source location and pinned by
  snapshot tests (`Subject` and `ReportSubject` wire-format
  snapshots in their respective modules; `describe_capabilities_snapshot`
  in `src/api/admin.rs`) plus a structural lint
  (`tests/admin_handler_contract.rs`) and a phrase-presence test
  (`tests/contract_phrases.rs`). Operator summary at
  `docs/operator/contract-stability.md`. Drift becomes a loud
  CI failure: any future PR that silently removes a commitment
  phrase from its canonical location, breaks a wire-format
  snapshot, or drops the audit-entry-ID field from a typed
  `*Output` struct (without an explicit allowlist entry) fails
  the appropriate test.

### Changed
- **Wire-format breaking change (v0.3 / Arc 2 contract lockdown).**
  `tools.aurora.superadmin.grantRole` and `tools.aurora.superadmin.revokeRole`
  responses are now typed structs with `rename_all = "camelCase"` wire fields,
  replacing the prior ad-hoc `serde_json::json!(...)` shape. The action-ID
  field renames from `audit_entry_id` → `auditEntryId`; on `grantRole` the
  embedded role record's wrapper field also renames `admin_role` → `adminRole`
  (the inner struct's snake_case fields are unchanged). Aligns these two
  handlers with the action-ID contract committed in
  `crate::admin::audit_chain` per `docs/V03_DESIGN.md` §6.3.4 — every
  Aurora-namespace admin handler that writes a chain entry now surfaces
  `auditEntryId` on a typed `*Output` struct. Drift is caught by
  `tests/admin_handler_contract.rs`. Per Arc 2 Step 0.5 prereq recon, the
  only in-tree consumer of `audit_entry_id` was a single unit test in
  `src/api/admin.rs` (updated alongside this change); the admin UI invokes
  these endpoints but discards the response, so no UI-side coordination
  needed.

### Added
- Design corpus updated to reflect as-built reality on five surfaces. Forensic export's `account-state.json` is always included with operational fields; sensitive fields remain `includeAccountMetadata`-gated (CR-7). `describeCapabilities` advertises a hand-curated capability list; runtime route enumeration deferred to v0.3 (#123). Runtime settings configuration is two-tier (Runtime + Default with RecoveryMode override) rather than the originally-specified three-tier; file-tier addition deferred to v0.3 (#124). `ModEventAction` is subject-aware (`TakedownAccount` vs `TakedownRecord` etc.) rather than the originally-specified compositional shape; compositional revisit deferred to v0.3 (#125). `getModerationMetrics` and `getQueueStats` use flat `start`/`end` strings and `i64` rather than the `TimeRange` wrapper and `u32`; typed-shape revisit deferred to v0.3 (#126).
- Live subscription channel separated from the historical aggregate. New `mod_event_seq` table mirrors the subset of `moderation_event` columns the `Event` wire variant emits (the `meta` column is intentionally not mirrored — the wire format doesn't carry it). Every `moderation_event` INSERT also writes a `mod_event_seq` row inside the same transaction via `insert_moderation_event_in_tx`, so the two surfaces never diverge. `tools.aurora.admin.subscribeModEvents` now reads from `mod_event_seq`; `tools.aurora.moderator.queryEvents` and other historical reads continue to use `moderation_event` directly. Migration `0006_mod_event_seq.sql` (SQLite + Postgres). Per docs/AURORA_ADMIN_UI_DESIGN.md §3.5. (#115)
- New env var `PDS_MOD_EVENT_RETENTION_DAYS` (default 7) controls the retention window for the `mod_event_seq` live subscription channel. The unbounded `moderation_event` historical aggregate is unaffected. A new background cleanup job (24-hour interval) deletes `mod_event_seq` rows older than the window. Operators running long-lived deployments who want longer retention raise the env var; values that are missing, malformed, or non-positive fall back to the default. (#115)
- New `OutdatedCursor` wire-format variant in `subscribeModEvents`. Emitted on connect when the caller's `cursor` is older than the oldest retained `mod_event_seq.seq` (i.e., events have been pruned by the cleanup job). The frame carries `oldestAvailableSeq` (the lowest valid resume point) and a human-readable `message`; the WebSocket then closes cleanly with code 1000. Clients consuming `subscribeModEvents` need to handle this new `$type` value: re-bootstrap via `tools.aurora.moderator.queryEvents` for the missed window, then resubscribe. (#115)
- Batch op responses now expose a `failures: BatchFailure[]` array. Each `BatchFailure` carries the failing `subject` (DID for account batches, URI for record batches) and an operator-readable `reason`. Today populated only by `batchTakedownAccounts` and `batchRestoreAccounts` — the other four batch endpoints run their per-row mutations in a single transaction, so `failures` is empty and a per-row error aborts the whole batch with 500. Surface is on every batch response shape for parity. (#112)
- Batch ops now capture per-subject snapshots. The six `tools.aurora.admin.batch*` endpoints (`batchTakedownAccounts`, `batchSuspendAccounts`, `batchRestoreAccounts`, `batchTakedownRecords`, `batchApplyLabel`, `batchRemoveLabel`) capture an `audit_snapshot` row per subject before the underlying mutation runs and record the per-subject ids in a new `cascade_snapshot_ids` JSON column on `audit_chain_entry`, paired by index with `cascade_subjects`. Per §3.4, the chain says "operator X took action Y on subjects A, B, C"; the snapshot says "subjects A, B, C were in state S_A, S_B, S_C." Without per-subject snapshots, batch ops left the chain pointing at subjects whose state-at-decision couldn't be reconstructed. The new column is included in the canonical hash so `verify_chain_range` catches tampering with the snapshot linkage. Migration `0005_audit_chain_cascade_snapshot_ids.sql` (SQLite + Postgres). The wire response's per-subject `SnapshotRef.snapshotId` is now populated for batch entries (was always `null`). (#111)
- Audit chain coverage for every administrative action under `com.atproto.admin.*`. 26 call sites across 7 categories now write a hash-chained `audit_chain_entry` row on success: account moderation (`takedown`/`suspend`/`restore`/`sendEmail`/`enableAccountInvites`/`disableAccountInvites`), label moderation (`applyLabel`/`removeLabel`), report decisions (`updateReportStatus`), the consolidated `updateSubjectStatus` (one chain row per call even when both `takedown` and `deactivated` patches are supplied, per §3.4), invite-server ops (`createInviteCode`/`disableInviteCode`/`disableInviteCodes`), seven previously-audit-blind handlers (`updateAccountEmail`/`updateAccountHandle`/`updateAccountPassword`/`deleteAccount`/`updateAccountSigningKey`), and operator infrastructure (`pauseSequencer`/`resumeSequencer`/`resetSequencerCursor`/`rebuildSequencer`/`cleanupRateLimitState`/`triggerPdsDiscovery`/`cleanupNonceStores`). `submitReport` remains intentionally unchained — reports are user-facing, not administrative decisions. (#109)
- `getAuditTrail` response now carries `chainVerified` and `chainVerifiedThrough` fields. Per-row `verified` flags catch row-local tampering; the new chain-level fields catch the consistent-rewrite attack where `current_hash` was rewritten in step with the content but the linkage between rows was missed. Backed by a new `verify_chain_range` API that walks the chain checking both per-row hash and prior-row linkage. (#97)
- Audit chain coverage for nine previously-unhooked endpoints: the six batch ops (`batchTakedownAccounts`, `batchSuspendAccounts`, `batchRestoreAccounts`, `batchTakedownRecords`, `batchApplyLabel`, `batchRemoveLabel`), `triggerPasswordReset`, `grantRole`, and `revokeRole`. Each successful mutation now writes one chain entry; batch endpoints record the per-subject list in `cascade_subjects` on a single chain row matching the §3.4 "one decision = one chain entry" framing. (#97)
- `subscribeModEvents` streams audit chain entries in real time. Set `includeAuditChain: true` in the subscription input; new `AuditEntry` messages interleave with `Event` messages by timestamp order. Resume via the new `auditChainCursor` parameter, separate from the existing `cursor` for events. Visibility gated to Moderator+ role per §8.4; insufficient role silently omits `AuditEntry` messages without erroring per §3.6 non-enumeration. (#105)
- DPoP proof-of-possession now enforced on resource requests. Tokens issued with a bound DPoP thumbprint require the request to carry a DPoP proof whose JWK matches the bound thumbprint and whose `ath` claim equals base64url(SHA-256(access_token)) per RFC 9449 §4.3. Bearer-only tokens (no thumbprint) accept requests without a DPoP header — backward compat for unbound tokens. (#100)

### Changed
- v0.2 cycle handoff document (#154)
- Phase 5 — Production primitives (#78)
- Phase 4 — Multi-instance support (#77)
- Phase 3 — Query layer compatibility (Group A+B files) (#76)
- SL-6 — Replace stale ModEventAction variant list in §4.1.2 with code reference (#153)
- SL-5 — Tighten override-mechanism cite in endpoint inventory (#152)
- SL-4 — Fix listAccounts line cite drift in endpoint inventory (#151)
- SL-3 — Add Phase 4 / Phase 3.10 env vars to .env.example (#150)
- SL-2 — Update aurora_admin.rs module docstring; Phase 3.8 has shipped (#149)
- P-3 — Document getSubjectStatus blob-branch 501 in §3.1 (#147)
- P-2 — Note §8.5 audit-chain visibility gate is structurally always-true in v0.2 (#146)
- CR-4 — Document audit-trail.json redirect-string pattern in §8.7 (#145)
- SL-1 — Fix stale §4.5 cite in src/cache/invalidation.rs:244 (#148)
- CR-3 — Update §5.4.2 LISTEN/NOTIFY backoff schedule to as-built six-step (#144)
- CR-1 — Drop stale axum 0.7→0.8 dependency bump claim from §6.3 (#143)
- P1 follow-up — Bring §5.3.8 Audit page prose into line with v0.2 no-sentinel-rows reality (#142)
- S5 — Resolve §3.6 self-violation: remove named external-tooling references in design doc (#141)
- S3 — Rewrite src/oauth/scope.rs:818 comment to match AdminServer-only behavior (#140)
- S1 — Update listRecentEvents route comment to past tense post-Phase-3 (#139)
- P1 — Clarify pre-chain sentinel as defensive-only in v0.2 (#138)
- F4 — Rewrite §3 lexicon-shape audit against as-shipped state (#137)
- F1 — Update §8.4 GetAuditTrailOutput shape: rename entries → items and document chain-level fields (#136)
- S2 — Remove src/api/admin_panel.rs.bak from tree (#135)
- P2 — Verify and tighten EmitEventOutput.audit_entry_id post-Phase-3.8 (#134)
- F3 — Tighten batch endpoint audit_entry_id type from Option<String> to String (#133)
- F2 — subscribeModEvents AuditEntry shape: wrap in entry field and add missing AuditEntry fields (#132)
- **Wire-format breaking change.** `tools.aurora.admin.getModerationMetrics` is now a `GET` query, not `POST`. The endpoint is a pure read with no side effects, so XRPC's query convention applies; the prior `POST` shape contradicted §8.16's read-vs-mutate split. Inputs (`start`, `end`, `granularity`, `metrics`) move from a JSON body to query parameters. The `metrics: Vec<MetricType>` parameter takes repeated keys (`?metrics=takedowns&metrics=labels`), which requires `axum_extra::extract::Query` rather than `axum::extract::Query` — same pattern `getAccountInfos` uses. Clients sending `POST` get 405 Method Not Allowed. v0.2 has not shipped to upstream so no external clients need a coordinated upgrade. (#118)
- `audit_chain::append_entry` gains an `_in_tx` companion so admin handlers can land the chain entry atomically with their underlying mutation. Pre-fix, `append_entry` opened its own transaction and committed before returning, so a handler calling it after an actor-table UPDATE had a tear window where the mutation could land but the chain row could fail/crash, silently violating the §3.4 "every administrative decision gets a chain row" invariant. Five sites migrated (`emit_event`, `enable_account_invites`, `disable_account_invites`, `grantRole`, `revokeRole`); the seven remaining sites — six `batch*` handlers and `updateAccountEmail` — depend on manager-API `_in_tx` splits and are tracked under #127 for v0.3. New `AppendChainGuard` exposes the in-process serialization for caller-managed tx flow; existing pool-API `append_entry` is now a thin wrapper. `ModerationEventLogger::log_event_in_tx` companion lands moderation_event + mod_event_seq + audit_chain_entry in one tx. (#122)
- `Subject::Blob` round-trips `record_uri` through `audit_chain_entry`'s flat columns. Pre-fix, the chain producer destructured `Subject::Blob { did, cid, .. }` and dropped `record_uri` on the floor; reading the row back through `Subject::from_columns` then saw `(Some(did), None, Some(cid))` — a shape with no matching arm — and returned `None`, losing the subject identity entirely on every Blob entry that carried a record context. Producer now binds and stores `record_uri` in `subject_uri`; reader gains a `(Some, None, Some) → Blob with record_uri = None` arm that also handles legacy chain rows written before this fix. The L-2 canonical-hash invariant is preserved (the new value flows through the same hash path; existing chain rows verify unchanged). (#121)
- `getAuditTrail`'s `chainVerifiedThrough` field surfaces the failing sequence on chain verification failure. Pre-fix the handler used `verify_chain_range(...).is_ok()` and reported `chain_verified_through = head_seq` on success but `0` on any failure, collapsing every failure mode (per-row tamper at seq=N, linkage break at seq=N, gap at seq=N) into the same "verified through 0" signal. Operators investigating a chain failure now get `failing_sequence - 1` (saturating), pointing at the last verified row before the divergence. The change is purely informational — clients reading `chainVerified` get the same boolean — but operators auditing a tampered chain can localize the break instead of doing a manual binary search. (#120)
- `tools.aurora.admin.setRuntimeSetting` validates the `key` against an allowlist (`KNOWN_RUNTIME_KEYS`: currently `moderation-mode` and `moderation-mode-redirect-url`). Pre-fix, any key was accepted into the `runtime_settings` table; a typo or fabricated key would write a row that no reader ever consults. The allowlist makes drift loud — unknown keys return 400 with the known-keys list — without locking the surface against legitimate v0.3 additions (the constant lives next to the existing key constants). `getRuntimeSetting` is unaffected; the read side already returns the hardcoded default for keys with no row. (#119)
- Crate version bumped from `0.1.0` to `0.2.0` to match the v0.2 cycle. The bump propagates through `env!("CARGO_PKG_VERSION")` to `describeCapabilities.version` (admins see `0.2.0` in capabilities probes) and to the firehose `Hello` frame. (#117)
- Documentation refactored. Cycle-authored design docs consolidated into `docs/AURORA_DESIGN.md` (server-side design), with `docs/AURORA_ADMIN_UI_DESIGN.md` edited in place for as-built alignment and `docs/AURORA_ENDPOINT_INVENTORY.md` committed as a canonical post-cycle endpoint reference. Twelve cycle-authored markdown files become three canonical references; cross-document inconsistencies from the round-two audit (C1, C2, C4, C5, C6) reconciled with as-built reality winning in each case. Operator-facing docs under `docs/operator/` and Doll's pre-cycle docs (READMEs, parity assessments, ARCHITECTURE.md, etc.) untouched. (#116)
- `tools.aurora.admin.subscribeModEvents` cursor space changed. Cursors now identify rows in the new `mod_event_seq` table rather than `moderation_event.id`. The two sequences are independent — old cursors won't match any new `seq` value. Clients holding cursors from before this version receive an `OutdatedCursor` frame on next connect and re-bootstrap via `tools.aurora.moderator.queryEvents` for the missed window, then resubscribe with a fresh cursor (or omit cursor to start from the current tail). Not a wire-format break in the strict sense — `cursor` is still an integer — but a behavior change that clients depending on the old space will notice immediately. v0.2 has not shipped to upstream so no external clients are affected. (#115)
- `tools.aurora.admin.emitEvent` `SendEmail` action now requires Admin+ role. Pre-fix the role gate accepted Moderator+ for `SendEmail`, which contradicted the §3.2 Admin-tier definition (Admin tier covers passwords, emails, handles, signing keys, deletion). Moderators emitting `SendEmail` via `emitEvent` now get 403; Admins succeed unchanged. Other moderator-flavored events (e.g., `ApplyLabel`, `TakedownAccount`, `ResolveAppeal`) continue to accept Moderator+. Behavior change for any client that depended on Moderator-emitted `SendEmail`; v0.2 has not shipped to upstream so no external clients are affected. (#114)
- Batch op responses now carry a `failures` array; design doc updated to match implementation. The six `tools.aurora.admin.batch*` endpoints describe two-tier atomicity: the chain entry (moderation_event row + `audit_chain_entry` row) is atomic — both land or neither lands — but per-subject actor-state mutations (account_moderation rows, takedown_ref updates) are best-effort. Per-subject failures land in the new response field rather than rolling back the chain entry. `affected_count` semantics update from "subjects requested" to "subjects whose actor-table mutation actually applied"; clients that depended on `affected_count == cascade_subjects.length` need to read both. The chain row's `cascade_subjects` continues to record operator intent (every requested subject) so reconciliation between intent and effect happens via `failures` and `getAuditTrail`. `docs/AURORA_ADMIN_UI_DESIGN.md` §6.4 BulkActionPanel notes and §8.8–§8.13 endpoint specs updated. True end-to-end per-subject atomicity is tracked under #113 (v0.3). (#112)
- `com.atproto.admin.getAuditLog` now reads from the hash-chained `audit_chain_entry` table. The wire-format response is preserved for back-compat: `details` maps from the chain row's `rationale`, `admin_did` from `actor_did`, and `ip_address` is always omitted (the chain schema doesn't carry it). Filters (`adminDid`, `action`, `subjectDid`, cursor) work as before. Clients that need richer chain semantics — verification status, snapshot ids, sequence numbers — should migrate to `getAuditTrail`. (#109)
- `apply_account_status` and `apply_blob_status` (helpers under `updateSubjectStatus`) no longer write per-patch chain entries inline. They now return a list of patch-effect descriptors that the handler joins into a single chain row's rationale, satisfying the §3.4 "one decision = one chain entry" framing. Behavior at the wire surface is unchanged — operators see the same response — but the chain ledger now has one row per `updateSubjectStatus` call instead of one per patch within a call. (#109)
- **Wire-format breaking change.** The four operator-flavored endpoints under `tools.aurora.admin.*` (`triggerPasswordReset`, `exportAccountForensic`, `getRuntimeSetting`, `setRuntimeSetting`) now require `AdminServer` scope per UI design doc §8.6/§8.7/§8.16. OAuth tokens carrying only `AdminModeration` scope hitting these four endpoints return 403; the wildcard `atproto:admin.*` continues to satisfy via implicit-includes. The namespace default for `tools.aurora.admin.*` remains `AdminModeration`; the per-NSID override applies only to these four operator-flavored NSIDs. v0.2 has not shipped to upstream so the audience for this break is internal — no external clients need a coordinated upgrade. (#96)
- **Wire-format breaking change.** `subscribeModEvents` `actionFilter` is now an array. Clients sending it as a scalar string (`actionFilter: "AccountTakedown"`) fail to deserialize at the Query extractor; send as an array (`actionFilter: ["AccountTakedown"]`) instead. New `subjectUri` filter added for record-level moderation events. (#102)
- DPoP proof issuance no longer silently downgrades to Bearer on invalid proof. Pre-fix, `/oauth/token` emitted `warn!("allowing for development")` and issued an unbound Bearer token whenever the DPoP proof failed to verify; per RFC 9449 §5 invalid proofs must fail. Three-state semantics now hold: missing DPoP header → Bearer; valid DPoP proof → DPoP-bound token; invalid DPoP proof → 400. (#100)
- Sequencer leader-election advisory lock now runs on a connection dedicated to the lock, separate from the application pool. Pool sizing matches POSTGRES_PHASE_4 §5.1's `pool_size + 2` model (the +2 are the lock connection and the LISTEN connection). Pre-fix, the leader borrowed a `PoolConnection` for the leader's lifetime — invisibly stealing one application pool slot. Operators sizing pools should target `pool_size` for application work, not `pool_size + 1`. (#103)
- `subscribeModEvents` ships an `AuditEntry` wire variant alongside `Event` / `Hello` / `Heartbeat` / `Error` per §8.5. Existing consumers that match exhaustively on `$type` need to handle the new variant. (#105)
- Moderation reason classifier rebranded as an extended-vocabulary mapping. The structural detector now matches any non-canonical reason NSID (`<namespace>#reason<Suffix>`) other than the canonical `com.atproto.moderation.defs#reason` prefix; substring-keyed classification handles routing to internal categories. The change is structural — behavior at the test surface is preserved — but the detection now generalizes beyond a specific external system rather than naming one. (#104)

### Fixed
- All administrative call sites in `aurora_admin.rs` and `admin.rs` that perform a chain-write paired with an actor-state mutation now do both atomically. Round-three audit's LB-1 finding flagged twelve such sites (sessions covered by #122 and #128 closed them); this round closes seventeen additional sites with the same shape that the audit's curated list didn't enumerate: single-account mutations (`updateAccountHandle`, `updateAccountPassword`, `adminDeleteAccount`, `updateAccountSigningKey`), single-handler `takedownAccount`/`suspendAccount`/`restoreAccount` (via `ModerationManager`), single-record/blob `applyLabel`/`removeLabel`, `updateSubjectStatus`, `updateReportStatus`, `sendEmail`, `createInviteCode`/`disableInviteCode`/`disableInviteCodes`, `setRuntimeSetting`, `triggerPasswordReset`. New `_in_tx` variants land on `AccountManager` (handle, password, delete, activate/deactivate/reactivate, generate_password_reset_token), `ModerationManager` (apply_action, reverse_action — both thread the tx through to the AccountManager variants for actor-state side effects), `ReportManager` (update_status), and `InviteCodeManager` (create_invite, disable_code, disable_codes_batch). `ModerationManager.apply_action`'s takedown side-effect was previously best-effort (logged failure, returned Ok with the moderation row); it is now atomic with the moderation row — failure of the actor UPDATE aborts the transaction. `sendEmail` and `triggerPasswordReset` moved to chain-first ordering: chain entry commits before the mailer dispatch, so a mailer failure no longer leaves operator action un-audited. The §3.4 chain-of-custody invariant now holds at every administrative call site where the pattern applies; sequencer ops, cleanup ops, and forensic export remain on the non-tx variant because they emit chain entries without paired mutations. (#129)
- All administrative call sites in `aurora_admin.rs` and `admin.rs` now write the audit chain entry atomically with the underlying mutation. Pre-fix, twelve sites split the moderation_event commit and the chain append into separate transactions, leaving an orphan-window where a mid-handler crash could strand a state mutation without chain coverage. Session 10 closed five sites (`emit_event`, `enableAccountInvites`, `disableAccountInvites`, `grantRole`, `revokeRole`); this round closes the remaining seven (six batch handlers — `batchTakedownAccounts`, `batchSuspendAccounts`, `batchRestoreAccounts`, `batchTakedownRecords`, `batchApplyLabel`, `batchRemoveLabel` — plus `updateAccountEmail`). `AccountManager` and `LabelManager` gain `_in_tx` companion variants for `takedown_account`, `update_email`, `apply_label`, and `remove_label`; the batch helper `insert_batch_account_moderations` becomes tx-aware. The two batch handlers with per-subject best-effort semantics (`batchTakedownAccounts`, `batchRestoreAccounts`) use `SAVEPOINT`-backed inner transactions so #112's `failures[]` array is preserved while the wrapping tx still gives chain-entry atomicity. The §3.4 chain-of-custody invariant now holds at every administrative call site identified by the round-3 audit; round-three's load-bearing LB-1 finding closed. (#128)
- `tests/multi_instance_test.rs` now compiles. The Postgres testcontainer harness was stranded after #103's dedicated-lock-connection refactor changed `PostgresLockProvider::new` to take the database URL string instead of an `AnyPool`; the test still passed `pool.clone()` and broke the integration-test build. `cargo test --tests --no-run` now compiles all eight integration test binaries clean. The harness still requires Docker to actually run, but the binary at least builds. (#110)
- Forensic export `bundle_hash` now covers the complete tar bytes. Pre-fix, the recorded hash covered only `manifest.json`; the per-file payloads (account state, moderation history, audit entries, audit-trail manifest) were inside the tar but outside the chain commitment. A tampered tar still passed verification. The chain entry's rationale and the `X-Aurora-Bundle-Hash` response header now both record SHA-256 over the complete tar bytes. The in-tar `audit-trail.json` no longer carries `bundleHash` or `auditEntryId` (would create a self-referencing hash cycle); consumers correlate the export to the chain entry via the `X-Aurora-Audit-Entry-Id` response header and a `getAuditTrail` lookup. (#99)
- Audit chain `append_entry` serializes concurrent writers. Pre-fix, two concurrent admin actions racing through the chain both observed the same head and both computed the same next sequence; the second INSERT failed with a `UNIQUE(sequence)` constraint error while the underlying mutation had already executed — silent chain entry loss under bursty load. Three layers of serialization (in-process mutex, transaction wrapping, Postgres `pg_advisory_xact_lock`) now hold; stress-tested with 20 concurrent writers producing contiguous sequences and clean linkage. (#106)
- Three admin router fallback pages (not-found, forbidden, error) no longer flow URL-derived input through `innerHTML`. Pre-fix, the not-found page interpolated `window.location.hash` into a `<code>` element via template-literal concat; the error page interpolated `err.message` similarly. Both attacker-controllable; a successful XSS in the admin UI grants the attacker access to `localStorage.adminToken` and from there every admin XRPC the operator has scope for. All three pages now build DOM via `createElement` + `textContent`; static-source pin test asserts no `innerHTML` assignments remain in the routing module. (#101)
- Admin UI grant/revoke role calls now succeed. Three call sites (`SettingsRoles.js`, `SettingsRolesMembers.js` x2) were sending `{ subject, role, rationale }`; the server's request types deserialize from `did`, not `subject`. Every UI-driven role grant or revoke failed with a serde deserialization error before reaching the handler. Renamed the field on the client; static-source pin test catches reintroduction. (#98)

### Removed
- `admin_audit_log` table dropped via migration `0004_drop_admin_audit_log.sql` (SQLite + Postgres variants). `AdminRoleManager::log_action` (the sole writer), `get_audit_logs`, and `get_audit_log_count` are gone with it; the public `AuditLogEntry` model type is unexported. Operators no longer have two parallel audit surfaces — every administrative decision lands in `audit_chain_entry`, which is hash-chained, snapshot-bearing, and verifiable per §3.4 chain-of-custody. v0.2 has not shipped to upstream so legacy rows do not need migrating. (#109)
- `PDS_ADMIN_DIDS` env var no longer grants admin authority. Pre-fix, three codepaths (`AdminAuthContext` extractor, OAuth admin-callback role check, invite-disable creator/admin gate) treated env-var DIDs as having implicit admin role. The shadow path bypassed the `admin_role` table and the audit chain. Removed; admin authority comes from `admin_role` only. The `PDS_ADMIN_DIDS` config field stays — it remains useful for operator-side warnings — but reading it for behavior is gone. Bootstrap path documented in README's "First Admin User" section: one-time SQL insert, then all subsequent grants flow through `tools.aurora.superadmin.grantRole` and the audit chain. (#95)
- Admin UI no longer stores `adminRefreshToken` in localStorage. The login flow wrote it but no consumer ever read it; storing without consuming expanded the localStorage exfiltration surface for zero current value. Server still emits `refresh_token` in the OAuth callback response; the client now discards it. A real refresh-token flow is planned for v0.3 token-lifecycle design pass. (#108)

### Security
- Live subscription channel storage now bounded by operator-configured retention. Pre-fix, the `subscribeModEvents` channel read directly from `moderation_event`, which retains forever. Operators running aurora-locus for a year accumulated one year of detail rows on the streaming hot path even though no client ever resumes from a cursor older than a few days. Post-fix, the streaming surface uses the retention-bounded `mod_event_seq` mirror (default 7-day window via `PDS_MOD_EVENT_RETENTION_DAYS`); the historical aggregate retains forever as the system of record. Operators with privacy-bound retention policies (e.g., GDPR data-minimization) can configure tighter windows without affecting forensic queryability. (#115)
- Shadow audit surface eliminated (listed under Removed). Pre-fix, an operator inspecting `audit_chain_entry` would see only ~25% of administrative decisions; the rest landed in `admin_audit_log`, which had no hash chain, no snapshots, and no tamper-evident replay. An attacker with database write access could rewrite `admin_audit_log` rows and the legacy `getAuditLog` reader would happily serve them. Post-fix, every administrative decision goes through `audit_chain::append_entry`'s serialized writer (in-process mutex + transaction wrap + Postgres advisory lock per #106), and `getAuditLog` reads from the chained table — so any inspection surface that operators have today is backed by §3.4 chain-of-custody. (#109)
- DPoP proof verification wired end-to-end. Combined with the resource-request enforcement listed under Added, this closes the path where a stolen access token without its bound private key could be replayed. Pre-fix, even tokens bound to a DPoP thumbprint at issuance time were accepted on resource requests with no proof presented. Operators who depend on DPoP for token-binding should now see proper §4.3 enforcement. (#100)
- Audit chain forensic-export tampering window closed via the `bundle_hash` fix above. Operators receiving a forensic bundle can now verify integrity by recomputing SHA-256 over the downloaded tar and comparing to the chain row's rationale or to the `X-Aurora-Bundle-Hash` response header at issuance time. Both record identical hashes per §3.4 chain-of-custody. (#99)
- Audit chain `append_entry` race-loss eliminated, listed under Fixed. Operators who saw intermittent 500 INTERNAL_SERVER_ERROR responses on admin-tier mutations under load were experiencing this race; chain entries that should have been written were silently dropped. Post-fix, concurrent writers serialize cleanly. (#106)
- `/admin/debug.html` no longer reachable in production builds. The page renders the bearer token from localStorage as visible page text — useful locally for endpoint probing without DevTools, but a token-disclosure surface for any operator screen-access vector in production. Now gated behind `PDS_ENABLE_DEBUG_PAGES` env var (default off); production deployments 404 the page; local devs set the var to opt in. (#107)
- `PDS_ADMIN_DIDS` shadow grant of authority closed (listed under Removed). Operators who relied on the env var for admin access need to add a row to the `admin_role` table directly per the bootstrap path; subsequent grants flow through the SuperAdmin endpoints. (#95)

### Pre-cycle (proto-blue migration)
- Fix rotted integration tests in tests/ (#14)
- Update deps + run clippy/fmt (#13)
- Migrate from embedded Rust-Atproto-SDK to proto-blue crate (#1)
- Delete Rust-Atproto-SDK directory and verify build (#12)
- Adapt cli/rotate_keys.rs to format_resign_commit (#11)
- Update api/repo.rs callers and signer plumbing (#10)
- Rewrite actor_store/repository.rs against proto_blue::repo::Repo (#9)
- Implement Signer wrapper around k256 PLC key (#8)
- Implement RepoStorage for ActorStore (SQLite-backed) (#7)
- Adapt actor_store/car.rs to proto-blue read_car/blocks_to_car (#6)
- Migrate leaf imports: did_doc, identity, oauth, syntax, tid (#5)
- Vendor blob mime/size helpers into src/blob_store/mime.rs (#4)
- Vendor PasswordHasher into src/auth/password.rs (#3)
- Add proto-blue dep, remove path dep on Rust-Atproto-SDK (#2)
