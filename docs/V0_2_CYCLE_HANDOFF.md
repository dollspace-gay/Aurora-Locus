# Aurora-Locus v0.2 cycle handoff

**Status:** v0.2 cycle complete; PR-ready handoff for upstream review.
**Companions (the design corpus):** [AURORA_DESIGN.md](AURORA_DESIGN.md) (server-side design), [AURORA_ADMIN_UI_DESIGN.md](AURORA_ADMIN_UI_DESIGN.md) (admin UI), [AURORA_ENDPOINT_INVENTORY.md](AURORA_ENDPOINT_INVENTORY.md) (endpoint reference).
**Audience:** doll, reviewing the v0.2 PR.

This document is a cycle-history layer on top of the design corpus. The corpus describes the as-shipped design; this handoff describes how the cycle reached it — what shipped, in what order, with what audit history, and what remains for v0.3. It is a navigation aid for review, not a substitute for the corpus.

---

## §1 Cycle summary

The v0.2 cycle ran against the upstream baseline `c2d6fd2` (the proto-blue migration commit). It delivered four substantive workstreams plus the supporting documentation refactor and five rounds of adversarial audit.

**Workstream A — Proto-blue migration.** Replace the bundled `Rust-Atproto-SDK` with `proto-blue 0.2.6`. Twelve source files migrated; two surfaces (`PasswordHasher`, blob mime/size utilities) extracted locally. `Repository` refactored against proto-blue's storage/signer injection model. Documented in [AURORA_DESIGN.md §6](AURORA_DESIGN.md).

**Workstream B — Postgres backend.** Selective Postgres for the two shared-state databases (`account_db`, `did_cache_db`); per-actor `repo.sqlite` always stays SQLite. Sixteen files refactored from `SqlitePool` to `AnyPool`. Postgres schema translated. Multi-instance support via sequencer leader election (advisory locks) and LISTEN/NOTIFY-driven cache invalidation. Backup/restore wrappers + WAL archiving operator guides. CI runs against both backends. Documented in [AURORA_DESIGN.md §5](AURORA_DESIGN.md) (Phase 4 multi-instance) and [§7](AURORA_DESIGN.md) (the per-file coupling work). The hybrid model (per-actor SQLite, shared-state configurable) is the architecture, not a transitional state — see [§8.3](AURORA_DESIGN.md).

**Admin/moderation Phases 1–2 (parity).** Phase 1 closed the bsky-PDS-2025-Q1 parity gaps in `com.atproto.admin.*`: five new endpoints (`disableInviteCodes`, `getAccountInfo`, `searchAccounts`, `updateAccountSigningKey`, polymorphic `updateSubjectStatus`) plus per-endpoint shape cleanup driven by the cycle-opening lexicon-shape audit. Of the eleven audited endpoints, ten now ship clean and one (`getSubjectStatus`) is mostly clean with a single Aurora extension (`suspended`) that omits from the wire when not populated. Phase 2 relocated ~30 operator-flavored endpoints from `com.atproto.admin.*` to `tools.aurora.ops.*` with `atproto:admin.server` scope, leaving `com.atproto.admin.*` as the slim parity surface bsky-PDS exposes. Documented in [AURORA_DESIGN.md §3](AURORA_DESIGN.md) (lexicon-shape audit) and [§4.3.4](AURORA_DESIGN.md) (`tools.aurora.ops.*` namespace).

**Admin/moderation Phase 3 (Aurora extensions).** Ship `tools.aurora.{describeCapabilities,moderator,admin,superadmin}.*` — the four-namespace extension surface bsky-PDS doesn't expose. Twenty-five new endpoints across ten sub-phases. The substantive primitives are the hash-chained audit log (`audit_chain_entry` with snapshot-at-decision), the unified action surface (`emitEvent` + six batch endpoints), the retention-bounded subscription channel (`mod_event_seq` + `subscribeModEvents` over WebSocket polling), and the metadata-only forensic export with chain-anchored bundle hash. Documented in [AURORA_DESIGN.md §4](AURORA_DESIGN.md) and [AURORA_ADMIN_UI_DESIGN.md §8](AURORA_ADMIN_UI_DESIGN.md).

**S3 blob storage activation.** Surfaced and worked during the cycle: AWS SDK dependencies activated, `PDS_BLOBSTORE_S3_*` env-var loading wired into `AppContext`, MinIO compatibility fields (`force_path_style`, `upload_timeout_ms`) added. Ships activated in v0.2.

**Documentation refactor.** Twelve cycle-authored markdown files consolidated into the three-document corpus. The handoff you're reading is a fourth document, sitting on top of the corpus rather than inside it.

The cycle's positioning shift, from where Aurora-Locus started: production-deployable with Postgres and S3 (was hobbyist-only with SQLite + disk); full bsky-PDS-2025-Q1 admin/moderation parity (was substantial but not lexicon-conformant); operator-vs-extension namespace separation under four `tools.aurora.*` families (was ~30 admin endpoints in one namespace); first-class Aurora extension surface with audit chain, snapshots, batch ops, live event tail, forensic export, runtime settings (was none of these). The README refresh tracking this positioning shift is open as #79 — see §4 below.

For the canonical scope statement and design principles, read [AURORA_DESIGN.md §1](AURORA_DESIGN.md).

### §1.1 Design principles in one sentence each

The cycle's design decisions are shaped by six principles documented across the corpus. Quick-reference summary:

1. **Server authority is total; the client is untrusted.** Every authority-bearing decision (role check, scope check, action authorization) is enforced server-side. The admin UI is a peer client of the same APIs operator tools and external clients call. Per [AURORA_ADMIN_UI_DESIGN.md §3.1](AURORA_ADMIN_UI_DESIGN.md).
2. **Subject-shape determines action set.** `Subject::{Repo,Record,Blob}` is the polymorphic vocabulary; lexicon is the source of truth for which actions apply where. Per [AURORA_DESIGN.md §4.1.1](AURORA_DESIGN.md).
3. **Snapshots-at-decision and audit chain are co-equal substrate.** The chain says *who decided what when*; the snapshot says *what the subject looked like at decision time*. Together they answer the forensic question. Per [§4.4](AURORA_DESIGN.md).
4. **Real-time is for signal arrival; everything else polls.** `subscribeModEvents` is for surfaces where event-arrival latency is itself the signal; UI surfaces refresh on poll or user action everywhere else. Per [§4.3.1](AURORA_DESIGN.md).
5. **Decoupling is structural, not nominal.** Aurora-Locus interoperates with the broader ATProto ecosystem and external moderator tooling without naming, preferring, or detecting any specific external system. Verified by cycle-end grep sweep across `docs/`, `src/`, `static/` (zero hits for any external-tool proper noun).
6. **PDS authority is bounded by network posture.** Aurora-Locus exposes the full administrative surface a PDS legitimately controls; deployment posture (paired-with-external-labeler vs independent) determines the practical reach of those actions, not the API contract.

These principles surface throughout the cycle's decisions. Where you see something architecturally distinctive in the corpus or in the PR diff, one of the six is usually the load-bearing reason.

### §1.2 The "Rust opportunity" that motivates the extensions

Aurora-Locus's Phase 3 extensions ride a small set of architectural advantages over bsky-PDS, documented at [AURORA_DESIGN.md §2.1](AURORA_DESIGN.md):

- The **sequencer is a Rust + axum + WebSocket primitive** that already streams a firehose; extending it to a moderation channel is incremental work.
- **Postgres's transaction model** makes batch atomicity natural for multi-subject operations.
- The existing `AuditLogEntry` substrate is **richer than what bsky-PDS exposes**.
- **Per-actor SQLite isolation** means cross-subject batch operations don't serialize through a shared lock.

None of these are specific to Rust per se — they're consequences of architectural choices Aurora-Locus made and bsky-PDS did not. The cycle's extension surface (`audit_chain_entry`, `subscribeModEvents`, six batch endpoints, forensic export) leans into these affordances rather than working around them.

---

## §2 Audit history

Five rounds of adversarial audit ran during and after the cycle. The first three drove substantive mid-cycle work; rounds four and five validated the implementation pre-PR.

| Round | Severity counts | Outcome |
|---|---|---|
| 1 | 1 critical (multibase key conversion), several high (DPoP JWK, signing key) | Pre-cycle work; closed via #47–#52 (multibase fix, DPoP fix). Informed Aurora-Locus's existing security posture before the v0.2 cycle. |
| 2 (mid-cycle) | 8 blocks of integrity / security / wire-format issues | Closed via Block 1–7 commits across the cycle. Touched audit chain transitive verification (#97), forensic export bundle hash (#99), DPoP end-to-end (#100), admin router XSS (#101), `PDS_ADMIN_DIDS` shadow-grant (#95), `/admin/debug.html` production gating (#107), `subscribeModEvents` AuditEntry variant (#105), operator-flavored scope tightening (#96), batch ops failures-array semantics (#112), batch ops per-subject snapshots (#111), `emitEvent` `SendEmail` Admin+ gate (#114). |
| 3 | 1 load-bearing (LB-1 chain-with-mutation atomicity), 6 confirmed-real (CR-1–CR-8 mix), several plausible/surface | LB-1 closed across Sessions 10–12 (#122, #128, #129) — twelve audit-cited sites + seventeen same-pattern sites. CR-1 (Subject::Blob round-trip), CR-2 (per-subject snapshots, became #111), CR-3/4/5/6 (deferred to v0.3 as #123/#124/#125/#126), CR-7 (account-state.json), CR-8 (chainVerifiedThrough failing-sequence) all closed. Documentation refactor (#116) consolidated the design corpus during this round. |
| 4 | **0 load-bearing**, 4 confirmed-real (F1, F2, F3, F4), 2 plausible (P1, P2), 5 surface (S1–S5) | All confirmed-real and plausible closed in fixup commits dab20e5 + 96acb1d + cb388df via chainlinks #132–#142. Zero deferrals. |
| 5 | **0 load-bearing**, 4 confirmed-real (CR-1, CR-2, CR-3, CR-4), 3 plausible (P-1, P-2, P-3), 6 surface (SL-1–SL-6) | All confirmed-real except CR-2 closed; all plausible except P-1 closed; all surface closed. Fixups in commit e9e687a via chainlinks #143–#153. CR-2 deferred as #155 (PDS_ADMIN_DIDS dead-config decision); P-1 deferred to existing #113 (batch ops end-to-end atomicity). |

**Two consecutive zero-load-bearing rounds** is the readiness signal. Audit 4 and audit 5 each surfaced findings, but every finding was either closed in the fix-up session or deferred with an explicit chainlink. The rounds verified the Phase 3 audit-chain substrate, the documentation corpus consistency, the cycle-end decoupling discipline (no external-tooling proper nouns in `docs/`, `src/`, `static/`), and the wire-format alignment between as-shipped code and design corpus.

The audit reports themselves are not in the repo as standalone artifacts — they were transient triage documents that fed the post-audit fixup commits. The CHANGELOG and chainlink history is the canonical record. CHANGELOG entries grouped under the audit-N fixup framing (and the commit messages of `audit-N fix-up:` commits) trace each finding through to closure.

---

## §3 Workstreams

Each workstream below: one paragraph summary, the substantive design decisions citing the corpus, the chainlink references that landed the work, and any deferred items that map to the workstream.

### §3.1 Proto-blue migration (Workstream A)

The cycle began with the upstream baseline at `c2d6fd2` already containing the proto-blue migration. Pre-cycle commits (#1–#14) executed the swap from the bundled `Rust-Atproto-SDK` to `proto-blue 0.2.6`. The cycle inherited the migration as a starting point and worked on top of it.

**Substantive decisions documented in corpus:**
- Twelve source files migrated; two surfaces (`PasswordHasher` at [src/auth/password.rs](../src/auth/password.rs), blob mime/size at [src/blob_store/mime.rs](../src/blob_store/mime.rs)) extracted locally because proto-blue doesn't expose them. See [AURORA_DESIGN.md §6.1](AURORA_DESIGN.md).
- `Repository` refactored against proto-blue's storage/signer injection model. The `RepoStorage` trait is implemented for `ActorStore`; `Signer` wraps the k256 PLC key. See [AURORA_DESIGN.md §6.2](AURORA_DESIGN.md).
- Concurrent `jsonwebtoken 9 → 10` bump and MSRV bump to 1.85 (proto-blue's required edition is 2024). `axum` stayed at `0.7`. See [AURORA_DESIGN.md §6.3](AURORA_DESIGN.md).

**Why the migration was substantive even though the cycle inherited it.** proto-blue's storage/signer injection model is fundamentally different from `Rust-Atproto-SDK`'s direct-instantiation model. The pre-cycle work refactored `Repository` to take a generic `S: RepoStorage` parameter and a generic `Sgn: Signer` parameter, then wired `ActorStore` to implement `RepoStorage` for SQLite-backed per-actor storage and `K256Signer` to wrap the existing PLC rotation key. The two locally-extracted surfaces (`PasswordHasher`, blob mime/size) were extracted because proto-blue intentionally doesn't expose them — they're aurora-specific concerns. The cycle inherited a clean baseline; the work was real but executed before v0.2 substantive work began.

**Closed chainlinks:**
- Pre-cycle (proto-blue migration): #1–#14 (in the CHANGELOG's "Pre-cycle" section)
- During cycle: #117 (Cargo.toml version bump 0.1.0 → 0.2.0)

**Deferred:** none specific to this workstream. Chrono/AnyPool patterns surfaced during Workstream B's Postgres work and are tracked under #80, see §3.2.

---

### §3.2 Postgres backend (Workstream B)

Phase-by-phase delivery of selective Postgres for the two shared-state databases. The hybrid model (`account_db` and `did_cache_db` on configurable backend; per-actor `repo.sqlite` always SQLite) holds throughout. Phase 4 makes Aurora-Locus deployable as multiple instances against one Postgres backend without sequencer races or stale per-process caches. Phase 5 adds operator primitives.

**Substantive decisions documented in corpus:**
- The three-database split (`account_db`, `did_cache_db`, per-actor `repo.sqlite`) is the architectural decision shaping everything else. The first two benefit from Postgres in production; per-actor stays SQLite for single-file backup/export/deletion semantics. See [AURORA_DESIGN.md §2.3](AURORA_DESIGN.md) (assessment) and [§5.1](AURORA_DESIGN.md) (selective Postgres).
- `AnyPool` chosen over per-backend pools (`SqlitePool` / `PgPool`) for the unified runtime substrate. The hybrid AnyPool + per-query escapes pattern is documented in [§7.2](AURORA_DESIGN.md).
- Sequencer leader election via `pg_advisory_xact_lock(SEQUENCER_LEADER_LOCK_KEY)`. Lock held for the leader's lifetime on a dedicated connection (separate from the application pool — borrowing would invisibly steal one application slot). Pool sizing model `pool_size + 2`. See [§5.3](AURORA_DESIGN.md), [§5.4.1](AURORA_DESIGN.md).
- Cache invalidation via Postgres LISTEN/NOTIFY. The writing instance issues `NOTIFY aurora_cache_invalidate, '<payload>'` after the modifying transaction commits; listening instances asynchronously invalidate matching local cache entries. Six-step exponential backoff (1s, 2s, 4s, 8s, 16s, 30s) on disconnect-recovery. See [§5.4.2](AURORA_DESIGN.md).
- Schema translations: SQLite-isms → Postgres equivalents documented in [§7.3](AURORA_DESIGN.md). All timestamp columns are `TEXT` in both backends (chrono types don't implement sqlx::Any traits); booleans are `INTEGER` in SQLite, `BOOLEAN` in Postgres.
- Backup/restore wrappers around `pg_dump` / `pg_basebackup`, plus WAL archiving operator guide. CI runs against both SQLite and Postgres on every commit.

**Why selective Postgres rather than per-actor Postgres.** The "three logically distinct database surfaces" framing in [AURORA_DESIGN.md §2.3](AURORA_DESIGN.md) is the load-bearing decision. `account_db` and `did_cache_db` benefit from Postgres in production deployments — concurrent writes, fan-out reads, the shared-state property Postgres handles well. Per-actor `repo.sqlite` doesn't: Postgres can't naturally do "one database per user" without either schema-per-actor (operationally awful at scale) or shared tables with `actor_did` columns (which loses the actor-isolation property bsky-PDS deliberately preserves). Per-actor SQLite gives single-file backup/export/deletion semantics — operator wipes one user's data by removing one file. The cycle's selective Postgres is therefore a hybrid model, not a transitional state, and it's documented as out-of-scope-by-design at [§8.3](AURORA_DESIGN.md).

**Why AnyPool rather than per-backend pools.** The cycle's coupling-audit ([AURORA_DESIGN.md §7.1](AURORA_DESIGN.md)) found 22 files importing `SqlitePool` or `sqlx::sqlite::*`. Four candidate approaches were evaluated ([§7.2](AURORA_DESIGN.md)): keep SQLite-specific types and add per-backend traits; use `sqlx::Any` everywhere; introduce a custom abstraction layer; or hybrid AnyPool baseline + per-query escapes for backend-specific features. The hybrid (Approach 4) was adopted. Each manager struct holds `db: AnyPool`. Most queries use the unified `?N` parameter syntax (sqlx internally rewrites for Postgres). Backend-specific paths exist only where genuinely needed — sequencer leader election via `pg_advisory_xact_lock`, LISTEN/NOTIFY, audit-chain serialization with optional Postgres advisory lock. This kept the surface area of "code that has to think about the backend" small.

**Why the connection-budget math matters.** Each Aurora-Locus instance uses `pool_size + 2` connections against Postgres ([§5.3](AURORA_DESIGN.md)). The +2 are the dedicated sequencer-leader-election lock connection and the dedicated LISTEN listener connection. Both are long-idle by design. Pre-#103 fix, the leader borrowed a `PoolConnection` for its lifetime — invisibly stealing one application slot. Operators sizing managed-Postgres connection limits should account for `(pool_size + 2) × instance_count`, where `instance_count` is the deployment's horizontal-scale factor.

**Closed chainlinks:**
- Phase 1 (schema): #74
- Phase 2 (backend selection in config and AppContext): #75
- Phase 3 (query layer compatibility, 16 files SqlitePool → AnyPool): #76
- Phase 4 (multi-instance support, leader election + LISTEN/NOTIFY): #77, #103 (sequencer leader uses dedicated connection)
- Phase 5 (production primitives, CI + backup/restore): #78
- Test fixtures: #110 (multi_instance_test.rs PostgresLockProvider API drift)

#76, #77, #78 were closed during handoff generation — see §8 surfacing notes.

**Deferred to v0.3:**
- **#80 Document AnyPool/chrono patterns established in v0.2 cycle.** Six portable patterns surfaced during Phase 3: timestamp binding via `to_rfc3339`/parse helpers, boolean decode via `i64 != 0`, `last_insert_id` unreliable on SQLite via AnyPool (use `INSERT ... RETURNING`), Any-incompatible column types (TEXT for timestamps, INTEGER for booleans), test-fixture TempDir leak gotcha, `FromRow` auto-derive incompatibility with `chrono::DateTime` fields. Deserves a developer-facing doc.
- **#81 Investigate AnyPool last_insert_id() returning None on SQLite.** Workaround in v0.2 is `INSERT ... RETURNING id`. Spike to determine whether this is sqlx documented behavior, a config-flag miss, or a sqlx bug worth filing upstream.

---

### §3.3 Admin/moderation parity (Phase 1)

Phase 1 closed the bsky-PDS-2025-Q1 parity gaps in `com.atproto.admin.*`. Five new endpoints shipped; eleven existing endpoints went through per-endpoint shape cleanup driven by the cycle-opening lexicon-shape audit ([§3 of AURORA_DESIGN.md](AURORA_DESIGN.md)). Of the eleven, ten now ship clean and one (`getSubjectStatus`) is mostly clean with a single Aurora extension that omits from the wire when not populated.

**Substantive decisions documented in corpus:**
- The lexicon-shape audit at cycle open identified four minor-drift and seven major-drift endpoints. Phase 1 closed every wire-breaking drift the audit identified. The only residual deviation from spec is `getSubjectStatus`'s `suspended` Aurora extension (omitted when None) and its blob-branch 501 (per-blob status state isn't tracked yet). See [§3.1](AURORA_DESIGN.md), [§3.2](AURORA_DESIGN.md), [§3.3](AURORA_DESIGN.md) for the as-shipped state.
- Polymorphic `updateSubjectStatus` (Phase 1.6) replaced the imperative-action model with the lexicon's declarative status-patch model. Subject dispatch covers `repoRef`, `repoBlobRef`, `strongRef` (with the blob and record branches having defined edges per [§3.1 getSubjectStatus entry](AURORA_DESIGN.md)). Returns `subject + takedown?` per spec.
- `account` (at-identifier) parameters introduced as the lexicon-conformant primary input; legacy `did` retained as deprecated back-compat field on three endpoints (`disableAccountInvites`, `enableAccountInvites`, `updateAccountEmail`).
- Cursor pagination (typed enum, base64url-encoded) wired on `getInviteCodes`/`listInviteCodes`. Legacy `includeDisabled` removed; disabled-only filtering relocates to `tools.aurora.ops.*` per Phase 2.
- Procedures with no declared output now return `Result<StatusCode, ...>` (OK no body) per spec; legacy `{success, did, message}`-style envelopes dropped.

**Closed chainlinks:**
- #56 Phase 1.1 — Lexicon-shape audit of 11 existing endpoints
- #57 Phase 1.2 — `updateAccountSigningKey` endpoint
- #58 Phase 1.3 — `disableInviteCodes` plural endpoint
- #59 Phase 1.4 — `getAccountInfo` endpoint
- #60 Phase 1.5 — `searchAccounts` endpoint
- #61 Phase 1.6 — `updateSubjectStatus` polymorphism (structural rewrite)
- #62 Phase 1.7 — `account`/`did` parameter rename sweep
- #63 Phase 1.8 — `sendEmail` required-field flips
- #64 Phase 1.9 — `getAccountInfos` param encoding + handle field
- #65 Phase 1.10 — `getInviteCodes`/`listInviteCodes` pagination
- #66 Phase 1.11 — Response body sweep (remove non-spec payloads)

**Deferred to v0.3:**
- **#67 Fix PlcClient::keys_match prefix-stripping bug.** Surfaced during #57 work. Aurora's CLI rotation flow only ever passes multibase-form so the bug never triggers there; the XRPC handler in #57 works around it. Underlying fix: strip the full `did:key:` prefix before stripping the multibase `z` prefix.
- **#69 Fix audit log column misuse: send_email passes subject as ip_address.** Surfaced during #63 work. The `send_email` handler writes the email subject into the column meant for IP addresses. CC preserved the misplaced behavior in #63 since the brief was scoped to required-field flips. Sweep audit for similar issues warranted.
- **#70 Add record-level moderation infrastructure (set/clear takedown_ref).** Surfaced during #61 work. The `actor_store.record` table has a `takedown_ref` column but no setter or clearer methods. `updateSubjectStatus` parses `strongRef` subjects correctly but returns 501 NotImplemented for the strongRef path because there's no way to apply or remove the takedown at the storage layer. ~30-40 lines of actor_store code + handler integration + tests.

---

### §3.4 Admin/moderation namespace cleanup (Phase 2)

Phase 2 relocated ~30 operator-flavored endpoints from `com.atproto.admin.*` to `tools.aurora.ops.*`, leaving `com.atproto.admin.*` as the slim parity surface bsky-PDS exposes and surfacing operator extensions under their own namespace with `atproto:admin.server` scope. The namespace-keyed scope-check middleware (Phase 2.2) is the substrate that makes the per-namespace scope contracts enforceable.

**Substantive decisions documented in corpus:**
- Three-tier scope hierarchy (`atproto:admin.*` / `atproto:admin.moderation` / `atproto:admin.server`) matches the existing role hierarchy. `atproto:admin.*` (full) implicitly satisfies the others via `AtProtoScope::includes`.
- `tools.aurora.ops.*` houses operator-flavored endpoints: system health, sequencer ops, blob ops, federation ops, rate-limit ops, validation/jobs/nonce ops, stats/accounts/metrics. Auth: `atproto:admin.server`. See [AURORA_DESIGN.md §4.3.4](AURORA_DESIGN.md).
- Two net-new operator endpoints under `tools.aurora.ops.*`: `listAccounts` (broader filters than `searchAccounts`), `getInstanceMetrics` (operator-flavored aggregates).
- Per-NSID override mechanism in [src/oauth/scope.rs:848-865](../src/oauth/scope.rs#L848-L865): four operator-flavored endpoints under `tools.aurora.admin.*` (`triggerPasswordReset`, `exportAccountForensic`, `getRuntimeSetting`, `setRuntimeSetting`) require `AdminServer` scope explicitly, replacing (not augmenting) the namespace default of `AdminModeration`. Pinned by test at line 624-636 of the same file.

**Why namespace relocation rather than scope-only tightening.** The pre-Phase-2 state had ~30 operator endpoints under `com.atproto.admin.*` with mixed-flavor scope (some accepted AdminServer, some AdminModeration, the rule wasn't structurally enforced). The cycle considered two options: (a) keep the existing namespace and tighten scopes per-endpoint, or (b) relocate operator endpoints to a dedicated namespace where the scope contract is namespace-keyed. Option (b) shipped because it makes the operator-vs-moderation distinction structurally visible — an operator looking at the route table sees the separation immediately; a future contributor adding a new operator endpoint reaches naturally for `tools.aurora.ops.*` and inherits AdminServer scope by default. The four operator-flavored carve-outs under `tools.aurora.admin.*` (per-NSID override) are the exception that proves the rule: when an endpoint is operator-flavored but lives in a moderation namespace for cohesion reasons, the override is explicit and visible.

**Why bsky-PDS parity for `com.atproto.admin.*`.** The cycle preserved the parity surface bsky-PDS exposes — same NSIDs, same wire shapes (post Phase 1 cleanup) — so existing bsky-PDS-targeted operator tooling works against Aurora-Locus without modification. Anyone running both deployments can use the same admin scripts. The Aurora extension surface lives entirely under `tools.aurora.*`, not as additions to `com.atproto.admin.*` — keeping the parity surface clean was a deliberate boundary.

**Closed chainlinks:**
- Phase 2.2 namespace scope-check middleware: tracked via commit a056017 (chainlink #83 era; the chainlink itself is the v0.3-followup tracker that now houses Phase 2 polish items)
- Phase 2.3.1–2.3.9 sub-phase commits for individual endpoint relocations (per-cluster: blob ops, sequencer, federation, rate limit, health/metrics, etc.)
- #95 PDS_ADMIN_DIDS env var no longer grants admin authority
- #96 Operator-flavored NSIDs require AdminServer scope only (CR-3 / Block 5)
- #98 Admin UI grant/revoke role: subject → did field-name fix

**Deferred to v0.3:** none specific to Phase 2 itself. Polish items from operator usage are tracked under #82 / #83 family — see §3.5 below.

---

### §3.5 Admin/moderation Aurora extensions (Phase 3)

Phase 3 ships the four-namespace Aurora extension surface under `tools.aurora.{describeCapabilities,moderator,admin,superadmin}.*`. Twenty-five new endpoints across ten sub-phases. The substantive primitives are the hash-chained audit log, the unified action surface, the retention-bounded subscription channel, and the metadata-only forensic export. Cross-cuts §3.6 (audit chain) and §3.7 (multi-instance).

**Substantive decisions documented in corpus:**
- Four-namespace structure: `tools.aurora.describeCapabilities` (capability probe), `tools.aurora.moderator.*` (Moderator+ reads), `tools.aurora.admin.*` (Admin-tier actions; four operator-flavored carve-outs at AdminServer scope), `tools.aurora.superadmin.*` (SuperAdmin role-management). See [AURORA_DESIGN.md §4.3](AURORA_DESIGN.md).
- `Subject::{Repo,Record,Blob}` polymorphic vocabulary ([§4.1.1](AURORA_DESIGN.md)). `$type`-discriminated wire format. Blob variant carries optional `record_uri` for round-tripping through chain rows (#121).
- `ModEventAction` discriminated enum ([§4.1.2](AURORA_DESIGN.md)). Subject-aware in v0.2 (`TakedownAccount` vs `TakedownRecord` are distinct variants); compositional revisit deferred to v0.3 as #125. Wire format `{"kind": "TakedownAccount"}` for unit variants; `{"kind": "ApplyLabel", "val": "...", "neg": false}` for variants with inline data.
- Unified action surface (`emitEvent`) translates the API-shaped `ModEventAction` to the storage-shaped `ModerationEventType` at write time. Writes both the moderation_event row and the audit chain entry inside one transaction. See [§4.3.2](AURORA_DESIGN.md).
- Six batch endpoints (`batchTakedownAccounts`, `batchSuspendAccounts`, `batchRestoreAccounts`, `batchTakedownRecords`, `batchApplyLabel`, `batchRemoveLabel`). Two-tier atomicity: chain-entry-atomic (moderation_event + chain entry land together or neither lands), per-subject best-effort. Per-subject failures land in `failures: BatchFailure[]` array. End-to-end per-subject atomicity is deferred to v0.3 as #113.
- Aggregations ([§4.3.2](AURORA_DESIGN.md)): `getModerationMetrics` (event counts, appeal counts, takedown counts, top moderators) and `getQueueStats` (pending counts, age percentiles).
- Retention-bounded subscription channel: `mod_event_seq` mirrors the wire-emitted subset of `moderation_event` columns inside the same transaction; `subscribeModEvents` reads from `mod_event_seq`. Configurable retention via `PDS_MOD_EVENT_RETENTION_DAYS` (default 7). `OutdatedCursor` wire variant signals when a client's cursor is older than the oldest retained row. See [§4.4.4](AURORA_DESIGN.md) and [AURORA_ADMIN_UI_DESIGN.md §8.5](AURORA_ADMIN_UI_DESIGN.md).
- Runtime settings (Phase 3.10): `getRuntimeSetting` and `setRuntimeSetting` against the `runtime_settings` table. Two known keys (`moderation-mode`, `moderation-mode-redirect-url`); `setRuntimeSetting` validates against an allowlist (#119). Two-tier configuration model (Runtime + Default with RecoveryMode override); file-tier addition deferred to v0.3 as #124.

**Why two-tier batch-op atomicity rather than three-tier.** The original design contemplated end-to-end per-subject atomicity: one transaction wrapping the chain entry, the moderation_event row, and every per-subject actor-table mutation. As-built, the chain entry is atomic with the moderation_event row but per-subject mutations are best-effort. The reason is `account_manager.takedown_account` opens its own transaction internally — it isn't transaction-composable from the batch handler's perspective. Refactoring the manager APIs to expose `_in_tx` variants for every per-subject mutation would have been a substantial cross-cutting change; the cycle scoped two-tier atomicity for v0.2 and explicitly tracked end-to-end atomicity as #113 for v0.3. The `failures: BatchFailure[]` response field surfaces per-subject failures transparently — clients see what happened without the chain entry pretending the failed subjects landed.

**Why `mod_event_seq` is separate from `moderation_event`.** `moderation_event` is the unbounded historical aggregate — the system of record for forensic queries. `mod_event_seq` is a retention-bounded mirror of just the columns the wire format emits, designed for the streaming hot path. Pre-fix, `subscribeModEvents` read directly from `moderation_event`; operators running aurora-locus for a year accumulated a year of detail rows on the streaming path even though no client ever resumes from a cursor older than a few days. The separation lets operators with privacy-bound retention policies (e.g., GDPR data-minimization) configure tighter windows on the streaming surface without affecting forensic queryability of the historical aggregate. Default retention is 7 days via `PDS_MOD_EVENT_RETENTION_DAYS`; the `meta` column is intentionally not mirrored because the wire format doesn't carry it. The dual-write helper `insert_moderation_event_in_tx` keeps the two surfaces in sync atomically; the read-source migration is pinned by a negative-path test.

**Why the four operator-flavored carve-out.** Within `tools.aurora.admin.*` (default scope `AdminModeration`), four endpoints are operator-flavored rather than moderation-flavored: `triggerPasswordReset` (admin password override), `exportAccountForensic` (admin forensic surface), `getRuntimeSetting`/`setRuntimeSetting` (admin runtime configuration). The per-NSID override at [src/oauth/scope.rs:848-865](../src/oauth/scope.rs#L848-L865) requires `AdminServer` for these four explicitly — replacing rather than augmenting the namespace default. AdminModeration alone is insufficient. The carve-out pattern is documented and pinned by test at line 624-636 of the same file. Future endpoints should follow the same pattern: namespace default applies to most, per-NSID override for substantively different auth tier.

**Closed chainlinks (Phase 3 sub-phases):**
- Phase 3.1 (Lexicon design + module organization): tracked via commit 707ce0d
- Phase 3.2 `tools.aurora.describeCapabilities`: tracked via commit 5c242d9
- Phase 3.3 (Moderator-tier reads — `queryEvents`, `getEvent`, `queryStatuses`, `getSubjectContext`, `getSubjectHistory`): commit 588a6f2
- Phase 3.4 (Moderator-tier appeals reads — `listAppeals`, `getAppeal`): commit 7af7639
- Phase 3.5A `tools.aurora.admin.emitEvent`: commit 46041e9
- Phase 3.5B (Six batch endpoints + `triggerPasswordReset`): commit 8725834
- Phase 3.5C/D (UI substrate ActionPanel + BulkActionPanel + capability-routed substrate): commits ed9e712, af20170
- Phase 3.6 (SuperAdmin role management relocation): commit 4a2340d
- Phase 3.7A (Aggregations — `getModerationMetrics`, `getQueueStats`): commit f0cbb01
- Phase 3.7B (Dashboard wiring): commit d22cfa1
- Phase 3.8A (Audit chain + snapshot infrastructure + `getAuditTrail`): commit ce79ea5; see §3.6 below
- Phase 3.8C `tools.aurora.admin.exportAccountForensic`: commit ffdc863; see §3.6 below
- Phase 3.9 (`subscribeModEvents` + subscription substrate + real-time indicator): commit 9462fb7
- Phase 3.10 (Runtime settings + UI & modes settings page): commit 268882c

**Closed cycle-mid chainlinks (substantive Phase 3 fixes):**
- #102 `subscribeModEvents` input shape (subject_uri + array action_filter)
- #104 Moderation reason mapping rebranded as extended vocabulary
- #105 `subscribeModEvents` `AuditEntry` variant + `include_audit_chain` field
- #114 `emitEvent` `SendEmail` requires Admin+ role
- #115 `mod_event_seq` retention-bounded subscription channel
- #118 `getModerationMetrics` POST → GET (XRPC query convention) — wire-format breaking
- #119 `setRuntimeSetting` allowlist enforcement
- #132 F2 — `subscribeModEvents` `AuditEntry` shape: wrap in `entry` field; add missing fields
- #133 F3 — Tighten batch endpoint `audit_entry_id` from Option<String> to String
- #134 P2 — Tighten EmitEventOutput.audit_entry_id post-Phase-3.8

**Deferred to v0.3:**
- **#82 Admin UI display polish (parent).** Render-vs-shape mismatches surfaced during #109 functional verification. Three subissues:
  - #84 System Health page display fields render as empty
  - #85 Sequencer page state and position render as unknown/empty
  - #86 listRoles handle resolution — server-side enrichment
- **#83 v0.2 cycle aftermath (design / spec follow-ups, parent).** Three subissues:
  - #87 §8.15 canonical capability vocabulary cleanup (`invite-lineage-v1`, `reporter-context-v1` listed but not shipped — ship endpoints or remove from advertised set)
  - #88 WebSocket auth subprotocol path for `subscribeModEvents` (v0.2 ships polling fallback because browsers can't send Authorization headers on WebSocket upgrades; production-fidelity path needs design)
  - #89 Rate limiter exemption — dynamic /admin/* path scope (documentation/process work to prevent future bypass)
- **#90 mountSidebarFooter dead code at app.js:43-44.** UI cleanup.
- **#91 RateLimitConfig.enabled flag is not consumed by middleware.** Either wire it through or remove the unused knob.
- **#92 aurora_capability_extensions hardcoded match against route table.** Drift risk: future endpoint removal without pulling capability from advertised list. Add build-time or test-time route audit.
- **#93 OAuth admin login: session persistence for AS-only DIDs.** Security-flavored, HIGH priority for v0.3. Real-prod failure mode: an admin DID authoritative on an external AS but with no local PDS account hits a login → 401 loop. Pattern is identical to a dev-auth session-fix that was on commit 05757a9 (now removed). Fix shape: short-circuit AdminAuthContext to JWT validation when scope claim says admin, bypassing the session-table lookup for OAuth-issued tokens.
- **#94 Substrate JS test coverage gaps.** Subscription primitive (polling-mode rewrite), SettingsRoles page (groupRoles pre-pass), session role resolution (resolution chain fix) — none have JS test coverage.
- **#113 Batch ops end-to-end atomicity per subject (also P-1 from audit 5).** Two-tier atomicity ships in v0.2; true per-subject atomicity requires `account_manager` API restructure to expose transaction-composable operations. Two design candidates documented in the chainlink description.
- **#123 LB-3 (round 3 audit) — Runtime route enumeration in describeCapabilities.** Static hand-curated list works for v0.2; v0.3 may derive from route registry at startup.
- **#124 CR-3 (round 3 audit) — File-tier configuration for runtime settings.** Three-tier model (Runtime > File > Default) was originally specified; as-built is two-tier. v0.3 implements file tier as yaml config.
- **#125 CR-5 (round 3 audit) — Compositional ModEvent shape.** Subject-aware variants ship in v0.2; compositional (action × subject orthogonal) revisit for v0.3.
- **#126 CR-6 (round 3 audit) — TimeRange wrapper + u32 retype across moderation metrics/queue stats.** Flat `start`/`end` strings + `i64` ship in v0.2; typed-shape revisit for v0.3 (also accepts preset names like `last_24h`).
- **#130 emit_event dispatch_action: atomic with chain entry (LB-1 pattern completion).** One LB-1-shape site remains: `emit_event`'s `dispatch_action` runs the manager mutation outside the wrapping transaction. Now that `_in_tx` variants exist on every relevant manager (Sessions 11 + 12), the migration is trivial. v0.3 candidate.
- **#131 BlobQuarantine: _in_tx variants for unified transaction with chain entry.** Session 12's `update_subject_status` migration handled the blob branch via release-and-reopen pattern (functional but slightly fragile — brief window of partially observable state). v0.3 refactor: give BlobQuarantine `_in_tx` variants to eliminate the release/reopen.
- **#155 CR-2 (audit 5) — PDS_ADMIN_DIDS dead-config: parse-removal vs deprecation-warning decision.** Env var still parses but no code path consumes it (parsed value is dead since #95 closed the security issue). Decision needed: clean break vs. operator-courtesy startup warning.

---

### §3.6 Audit chain + forensic export

Audit chain and forensic export are co-equal substrate per [AURORA_ADMIN_UI_DESIGN.md §3.4](AURORA_ADMIN_UI_DESIGN.md): the chain says *who decided what when*; the snapshot says *what the subject looked like at decision time*. Together they answer the forensic question. Cross-cuts Phase 3.8 (audit chain) and Phase 3.5/3.7/3.10 (every administrative action emits a chain entry).

**Why the cryptographic chain matters in v0.2.** Pre-cycle, Aurora-Locus had an `admin_audit_log` table with no hash chain, no snapshots, and no tamper-evident replay. An attacker with database write access could rewrite log rows and the legacy `getAuditLog` reader would happily serve them. The cycle's audit-chain substrate (#97, #109) closes that exposure: every administrative decision now goes through `audit_chain::append_entry`'s serialized writer (in-process mutex + transaction wrap + Postgres advisory lock per #106), and `getAuditLog` reads from the chained table — so any inspection surface that operators have today is backed by §3.4 chain-of-custody. The legacy table was dropped via migration `0004_drop_admin_audit_log.sql` rather than carrying both surfaces in parallel, which would have left the door open for partial reads. The cycle's audit history (rounds 2 and 3) repeatedly stress-tested this substrate; both rounds 4 and 5 verified it as zero-load-bearing.

**Why the chain has three concurrency layers, not just one.** The pre-#106 implementation used only a `BEGIN`/`COMMIT` transaction wrap. Two concurrent admin actions racing through the chain both observed the same head and both computed the same next sequence; the second `INSERT` failed with a `UNIQUE(sequence)` constraint error while the underlying mutation had already executed — silent chain entry loss under bursty load. The fix layers three primitives: in-process `tokio::sync::Mutex` ahead of the transaction, the existing transaction wrap, and (on Postgres) `pg_advisory_xact_lock(AUDIT_CHAIN_LOCK_KEY)` as the transaction's first statement. SQLite gets serialization for free via its database-level write lock; the advisory-lock query is skipped on SQLite. Stress-tested with 20 concurrent writers producing contiguous sequences and clean linkage.

**Substantive decisions documented in corpus:**
- `audit_chain_entry` is the single system of record for administrative decisions. Hash-chain columns: `current_hash` = SHA-256 over canonical-serialized row content; `previous_hash` = prior row's `current_hash`. Per-row hash catches local tampering; chain-level walk (`verify_chain_range`) catches consistent-rewrite attacks. See [AURORA_DESIGN.md §4.4.2](AURORA_DESIGN.md).
- Three-layer concurrency control on `append_entry`: in-process `tokio::sync::Mutex`, `BEGIN`/`COMMIT` transaction wrap, Postgres `pg_advisory_xact_lock(AUDIT_CHAIN_LOCK_KEY)` as the transaction's first statement. Without these, two concurrent appends both observe the same head, both compute the same next-sequence, and the second `INSERT` fails on `UNIQUE(sequence)` while the underlying mutation has already executed — silent chain entry loss.
- `audit_chain::append_entry_in_tx` companion API (LB-1). Admin handlers can land the chain entry atomically with their underlying mutation. Pre-fix, `append_entry` opened its own transaction and committed before returning; a handler calling it after an actor-table UPDATE had a tear window where the mutation could land but the chain row could fail/crash. LB-1 closure across Sessions 10–12 (29 sites total: twelve audit-cited + seventeen same-pattern) brings every administrative call site to atomic chain-with-mutation flow.
- Snapshot capture (`audit_snapshot` table). Captures pre-decision state of the subject. Account snapshots include handle, takedown_ref, deactivated_at, active_action; record/blob snapshots capture URI/CID with richer per-record state deferred to v0.3. See [§4.4.3](AURORA_DESIGN.md).
- Chain-level verification fields on `getAuditTrail`: `chainVerified` (boolean) + `chainVerifiedThrough` (i64; on failure surfaces failing_sequence-1 saturating, pointing at the last verified row before the divergence).
- Pre-chain sentinel rows: defensive-only in v0.2. `migrations/0004_drop_admin_audit_log.sql` dropped the legacy `admin_audit_log` table outright rather than migrating its rows as `current_hash="pre-chain"` placeholders. The skip path remains correct as a future-compatibility hook for any later restoration of legacy data. Documented in [AURORA_DESIGN.md §4.4.2](AURORA_DESIGN.md) and [AURORA_ADMIN_UI_DESIGN.md §8.4](AURORA_ADMIN_UI_DESIGN.md).
- `getAuditLog` (parity-floor wire shape) reads from the same `audit_chain_entry` backing table as `getAuditTrail`. The two-endpoint shape on the Audit page is preserved as forward-compatibility scaffolding; the merge is structural rather than data-bearing in v0.2. See [AURORA_ADMIN_UI_DESIGN.md §5.3.8](AURORA_ADMIN_UI_DESIGN.md).
- Forensic export (`exportAccountForensic`): metadata-only in v0.2. Bundle contains `manifest.json`, `account-state.json`, `moderation-history.json`, `audit-entries.json` (when SuperAdmin requested), `audit-trail.json` (chain anchor reference; see below). CAR data and blob bytes deferred to v0.3.
- Chain-of-custody pattern: bundle hash is SHA-256 over the complete tar bytes (not just `manifest.json`). Recorded in the chain row's rationale and surfaced in the `X-Aurora-Bundle-Hash` response header at issuance time. Bundle integrity verifiable by recomputing SHA-256 over the downloaded tar.
- `audit-trail.json` ships a redirect-string (`chainAnchor`) pointing to the `X-Aurora-Audit-Entry-Id` header and the chain row's rationale (where the SHA-256 bundle hash is recorded), rather than carrying the chain entry id and bundle hash inline. Reason: chicken-and-egg cycle between bundle hash (must cover all bytes including this file) and chain entry id (only known after the chain row is appended, which itself records the bundle hash). Documented at [AURORA_ADMIN_UI_DESIGN.md §8.7](AURORA_ADMIN_UI_DESIGN.md).

**Closed chainlinks:**
- #97 Audit chain transitive verification + chain coverage on 9 endpoints
- #99 Forensic export bundle hash covers complete tar bytes
- #100 DPoP proof verification wired end-to-end
- #105 `subscribeModEvents` `AuditEntry` variant
- #106 Audit chain `append_entry` serializes concurrent writers
- #109 L-1 — Audit chain coverage for all administrative actions in `src/api/admin.rs`
- #111 Batch ops capture per-subject snapshots
- #112 Batch ops atomic semantics: document partial-success behavior
- #120 CR-8 — `chainVerifiedThrough` surfaces failing_sequence on chain verification failure
- #121 CR-1 (round 3) — Subject::Blob round-trips `record_uri`
- #122 LB-1 — `audit_chain::append_entry_in_tx` + `ModerationEventLogger::log_event_in_tx`
- #127 LB-1-followup (closed in v0.2 by Session 11/#128 instead of being deferred to v0.3)
- #128 Complete LB-1: all twelve administrative call sites atomic with chain entry
- #129 Extend LB-1 closure: seventeen same-pattern administrative call sites
- #136 F1 — Update §8.4 GetAuditTrailOutput to match as-built (items not entries; document chain-level fields)
- #138 P1 — Clarify pre-chain sentinel as defensive-only in v0.2
- #142 P1 follow-up — §5.3.8 Audit page prose
- #145 CR-4 (audit 5) — Document audit-trail.json redirect-string pattern in §8.7

**Deferred to v0.3:** Items #113, #130, #131 in §3.5 above are LB-1-pattern continuation work that remains for v0.3.

---

### §3.7 Multi-instance Postgres

Multi-instance support is the Phase 4 work under Workstream B. Cross-references with §3.2 above; this subsection focuses on the cross-instance coordination primitives.

**Substantive decisions documented in corpus:**
- Sequencer leader election via `pg_advisory_xact_lock(SEQUENCER_LEADER_LOCK_KEY)`. Standby instances retry every `PDS_SEQUENCER_LEADER_RETRY_MS` ms (default 2000; bounds 500-30000). On leader graceful shutdown the lock releases and a standby acquires immediately; on connection drop the lock auto-releases on session end (Postgres semantics). See [§5.4.1](AURORA_DESIGN.md).
- Cache invalidation via Postgres LISTEN/NOTIFY. Channel `aurora_cache_invalidate`. Payload schema documented in [§5.4.2](AURORA_DESIGN.md). Six-step exponential backoff (1s, 2s, 4s, 8s, 16s, 30s) on disconnect-recovery. Notifications emitted during disconnect window are lost; TTL fallback in each invalidatable cache covers the staleness gap (LocalRecordsCache: 5s).
- Pool sizing model: each Aurora-Locus instance uses `pool_size + 2` connections against Postgres. The application `AnyPool` (default 25), one dedicated for the leader-election advisory lock, one dedicated for the LISTEN listener. Operators sizing managed-Postgres connection limits should account for `(pool_size + 2) × instance_count`. See [§5.3](AURORA_DESIGN.md).

**Why advisory locks for sequencer leader election rather than a coordination service.** The sequencer must have exactly one writer at a time across all instances — concurrent writers would race on `seq` allocation and produce non-monotonic firehose output. Three approaches were considered: (a) external coordination service (etcd, Zookeeper, Consul) — adds operational dependency, complicates deployment; (b) Postgres advisory locks — uses the database the deployment already runs; lock auto-releases on session end (Postgres semantics) so a crashed leader frees the lock automatically without external timeout machinery; (c) in-database leader election table — requires polling and lease semantics. Approach (b) shipped: zero new operational dependencies, leader election piggybacks on the connection model the deployment already manages, and crash recovery is handled by Postgres itself. Standby instances retry every `PDS_SEQUENCER_LEADER_RETRY_MS` (default 2000ms, bounds 500-30000) — operator-tunable for fast failover vs gentle Postgres load.

**Why LISTEN/NOTIFY for cache invalidation rather than periodic full-cache refresh.** Cross-instance cache coherence has two failure modes: (a) instance A mutates state, instance B serves stale cached data until its TTL expires; (b) instance A mutates state, instance B never sees the mutation because the cached entry has effectively-infinite TTL. LISTEN/NOTIFY closes both: the writing instance issues `NOTIFY aurora_cache_invalidate, '<payload>'` after the modifying transaction commits; listening instances asynchronously receive the payload and invalidate matching local cache entries. The TTL fallback in each invalidatable cache (LocalRecordsCache: 5 seconds) covers the disconnect window — notifications emitted while a listener is reconnecting are lost, but the cache's own TTL expires within 5 seconds anyway. Six-step exponential backoff (1s, 2s, 4s, 8s, 16s, 30s, then capped at 30s) handles transient disconnects without thrashing. The connection is dedicated and long-idle by design — same connection-budget rationale as the leader-lock connection.

**Closed chainlinks:**
- #77 Phase 4 — Multi-instance support
- #103 Sequencer leader uses dedicated DB connection
- #110 multi_instance_test.rs PostgresLockProvider API drift

**Deferred to v0.3:** none specific to multi-instance Postgres. Distributed rate limiting (Redis or Postgres-CAS token bucket) and OAuth state / DPoP nonces multi-instance are out-of-scope — see §5.

---

### §3.8 Documentation refactor + audit fixups

The cycle's final pass consolidated twelve cycle-authored markdown files into the three-document corpus (`AURORA_DESIGN.md`, `AURORA_ADMIN_UI_DESIGN.md`, `AURORA_ENDPOINT_INVENTORY.md`) and ran two adversarial audits over the consolidated corpus. The handoff you're reading is a fourth document layered on top.

**Substantive decisions:**
- The corpus is the design surface; this handoff is a navigation layer. The corpus describes the as-shipped design without cycle-history annotation; the handoff describes the cycle's path to the as-shipped state.
- Decoupling discipline (per [AURORA_ADMIN_UI_DESIGN.md §3.6](AURORA_ADMIN_UI_DESIGN.md)): the design doc, the resulting code, the strings file, the audit log entries, and any test fixtures use abstract framing throughout — no named external moderator tooling proper nouns. Cycle-end audit verifies grep-clean across `docs/`, `src/`, `static/`. The S5 finding from audit 4 closed two stranded named-tooling references in the design doc; the post-fix sweep is clean.
- Operator-facing docs under `docs/operator/`, doll's pre-cycle docs (READMEs, parity assessments, ARCHITECTURE.md, etc.), and unrelated developer docs are untouched by the cycle. The cycle modified pre-cycle docs only where Block 6 / Session 7 updated [README.md](../README.md) and [QUICKSTART.md](../QUICKSTART.md) to remove the `PDS_ADMIN_DIDS` env-var auto-grant.

**Why the consolidation, given the cycle's authored docs were already comprehensive.** Twelve cycle-authored markdown files had accumulated by mid-cycle: cycle-opening assessments per workstream, per-phase design docs for Phase 1 sub-phases, per-phase design docs for Phase 3 sub-phases, audit reports, and reconciliation notes. The cycle-end refactor (#116) consolidated these into the three-document corpus for two reasons. First, **single-source-of-truth**: cross-document inconsistencies (the C1, C2, C4, C5, C6 reconciliation notes in the corpus capture five of these) were genuinely confusing for the cycle's own contributors and would have been worse for downstream readers. Second, **maintainability**: future cycles extending the design need a clean place to update; twelve files with overlapping scope created drift opportunities at every change. The handoff you're reading is a fourth document layered on top because cycle-history is structurally separable from design — the corpus is the as-shipped state; the handoff is how the cycle reached it.

**Why the rounds 4 and 5 audit pattern is strong evidence of readiness.** Both rounds independently re-audited the consolidated corpus and the post-LB-1 implementation. Both found zero load-bearing issues. The findings each round surfaced were either confirmed-real wire-format alignments (e.g., F2's subscribeModEvents AuditEntry shape, F3's batch audit_entry_id type), plausible clarification gaps (e.g., P-2's chain-visibility gate, P-3's blob-branch 501), or surface drift (stale citations, comment past-tense). Each finding was triaged within one fix-up session, with the explicit-deferral chainlinks (#155 for CR-2, #113 for P-1) showing v0.3 work as named choices rather than missed work. The rounds verified the substantive structures (audit chain transitive verification, two-tier batch atomicity, decoupling discipline) end-to-end without finding new substantive gaps.

**Closed chainlinks (refactor + audits):**
- #116 Documentation refactor — consolidate cycle design docs
- #132–#142 Audit 4 fixups (F1 wire shape, F2 subscribeModEvents AuditEntry, F3 batch audit_entry_id, F4 lexicon-shape rewrite, P1 pre-chain sentinel, P2 emit-event Option tighten, S1–S5 stale comments / external naming)
- #143–#153 Audit 5 fixups (CR-1 axum drift, CR-3 backoff schedule, CR-4 audit-trail.json reconciliation, P-2 chain-visibility gate clarification, P-3 blob-branch 501 documentation, SL-1–SL-6 stale citations / module docstring / env vars / inventory cite drift / override-mechanism cite / ModEventAction variant list)

**Deferred to v0.3:**
- **#79 End-of-cycle README refresh.** Aurora-Locus has substantially evolved during v0.2 (production-deployable Postgres, S3 blobs, full bsky-PDS admin/moderation parity, four-family operator-vs-extension namespace separation, first-class Aurora extension surface). The README should reflect the as-shipped product. Tone consideration in the chainlink: accurate without overclaiming.

---

## §4 Deferred to v0.3

Consolidated list of every open chainlink under EPIC #1. Grouped by category. Each entry preserves the chainlink's substantive description so v0.3 cycle planning can read directly from the handoff without re-querying the chainlink db.

### §4.1 Substantive feature work

**#70 Add record-level moderation infrastructure (set/clear takedown_ref).** *Surfaced during Phase 1.6 / #61.* The `actor_store.record` table has a `takedown_ref` column but no setter or clearer methods. Every existing INSERT/UPDATE path leaves it NULL. The new `updateSubjectStatus` handler parses `strongRef` subjects correctly but returns 501 NotImplemented when one is provided because there's no way to apply or remove the takedown at the storage layer. Required additions: `ActorStore::set_record_takedown` / `clear_record_takedown` + handler integration + tests. Estimated scope: ~30-40 lines + tests.

**#93 OAuth admin login: session persistence for AS-only DIDs.** *Security-flavored, HIGH priority.* `src/api/oauth_admin.rs` callback's else branch (around line 310-356) mints JWTs for admin DIDs without persisting any session record. Real-prod failure mode: an admin DID authoritative on an external AS but with no local PDS account hits a login → 401 loop. AdminAuthContext extractor tries to validate the JWT as a session token first, which fails because no session exists. Fix shape: change AdminAuthContext to short-circuit straight to JWT validation when the token's scope claim says admin, bypassing the session-table lookup for OAuth-issued tokens. Pattern is identical to a dev-auth session-fix that was on commit 05757a9 (now removed via reset --hard before this cycle's work landed).

### §4.2 Audit deferrals (round 3+)

**#113 Batch ops end-to-end atomicity per subject (also P-1 from audit 5).** *Followup to #112.* The v0.2 fix documents two-tier atomicity (chain-entry atomic, per-subject best-effort) and surfaces per-subject failures in a `failures` field, but does not implement true per-subject atomicity. Why non-trivial today: `account_manager.takedown_account` opens its own transaction internally — not transaction-composable from the batch handler's perspective. Two design candidates: (1) refactor `account_manager` API to expose transaction-composable operations (preferred — cleaner architecture, more API churn), (2) inline the actor-table SQL into the batch handlers (smaller diff, duplicates side-effect logic). Surface area to revisit when this lands: the `failures[]` field semantics change — when the whole batch is atomic, failures collapses into a tx-rollback-error on the response.

**#123 LB-3 (round 3) — Runtime route enumeration in describeCapabilities.** *Adding a new endpoint requires a corresponding edit to the capabilities list, no compile-time link.* v0.3 plan: derive the capabilities list from the route registry at startup. Either a procedural macro that walks `#[xrpc(...)]` annotations, or a runtime build step that introspects axum::Router.

**#124 CR-3 (round 3) — File-tier configuration for runtime settings.** *Originally three-tier (Runtime > File > Default); as-built two-tier with RecoveryMode override.* Operators wanting non-default values write the runtime_settings table directly. v0.3 plan: implement file-tier as a config struct deserialized from yaml at startup. SettingSource enum gains a File variant. Lookup order: runtime row > file > default.

**#125 CR-5 (round 3) — Compositional ModEvent shape.** *Subject-aware variants ship in v0.2 (TakedownAccount vs TakedownRecord distinct).* Compositional (action × subject orthogonal) was originally specified. Cost of flat shape: duplication when adding a new action — every subject variant gets its own enum entry. v0.3 plan: revisit the shape. Options range from keeping flat with documented policy to compositional with payload-discriminated nested struct to hybrid where common shapes are flat and parametric ones carry inline data.

**#126 CR-6 (round 3) — TimeRange wrapper + u32 retype across moderation metrics/queue stats.** *Flat `start`/`end` strings + `i64` ship in v0.2.* The flat-string approach was chosen because axum's Query extractor flattens nested structs awkwardly. v0.3 plan: introduce a TimeRange newtype with custom Deserialize that accepts either preset names (`last_24h`, `last_7d`, etc.) or `{start, end}` pairs. Bind to u32 where the count is genuinely u32-domain-safe.

**#130 emit_event dispatch_action: atomic with chain entry (LB-1 pattern completion).** *One LB-1-shape site remains.* `emit_event`'s `dispatch_action` runs the manager mutation outside the wrapping transaction, leaving the same orphan-window the LB-1 invariant fixes elsewhere. Now that AccountManager, ModerationManager, LabelManager, ReportManager, and InviteCodeManager all have `_in_tx` variants (Sessions 11 + 12), `dispatch_action` can be migrated trivially: route the manager mutation through the appropriate `_in_tx` variant inside the same transaction as `log_event_in_tx` + `append_entry_in_tx`.

**#131 BlobQuarantine: _in_tx variants for unified transaction with chain entry.** *Session 12's `update_subject_status` migration handled the blob branch via release-and-reopen pattern.* Functional but slightly fragile — there's a brief window between the wrapping tx release and the quarantine tx where partial state is theoretically observable. v0.3 refactor: give BlobQuarantine `_in_tx` variants (`quarantine_blob_in_tx`, `restore_blob_in_tx`) so the entire blob-status update flow lives inside one transaction with the chain entry.

**#155 CR-2 (audit 5) — PDS_ADMIN_DIDS dead-config: parse-removal vs deprecation-warning decision.** *#95 closed the security issue (env var no longer grants admin authority).* The env var still parses at config load but the parsed value is dead. Decision required: clean break (remove parsing entirely) vs. operator-courtesy (startup log warning when var is present but ignored, remove parsing in v0.4). Either path is small code work; the blocker is the decision.

### §4.3 Parity gaps

**#67 Fix PlcClient::keys_match prefix-stripping bug.** *Surfaced during Phase 1.2 / #57.* `PlcClient::keys_match` strips one leading `z` character and compares the remainders; for `did:key:z6Mk...` vs `z6Mk...` form inputs (which are equivalent representations of the same key) it returns false. Aurora's CLI rotation flow only ever passes multibase-form so the bug never surfaces there; the XRPC handler in #57 works around it by stripping `did:key:` prefix before comparison. Underlying fix: strip the full `did:key:` prefix before stripping the multibase `z` prefix.

**#69 Fix audit log column misuse: send_email passes subject as ip_address.** *Surfaced during Phase 1.8 / #63.* The `send_email` handler at `src/api/admin.rs` calls `admin_role_manager.log_action(...)` with `Some(&req.subject)` in the position the function signature names `ip_address`. The email subject is being written to a column meant for IP addresses. Underlying fix: pass `None` for `ip_address` (or actually capture the requesting client's IP if available), and pass the subject through a more appropriate column. Investigate whether other admin handlers have similar audit-log column misuse — a sweep audit may be warranted.

### §4.4 Cleanup / tooling

**#79 End-of-cycle README refresh.** *See §3.8 above.* Aurora-Locus has substantially evolved during v0.2; README should reflect the as-shipped product. Tone consideration: accurate without overclaiming.

**#80 Document AnyPool/chrono patterns established in v0.2 cycle.** *Six portable patterns surfaced during Phase 3 (#76).* Timestamp binding via `to_rfc3339`/parse helpers, boolean decode via `i64 != 0`, `last_insert_id` unreliable on SQLite via AnyPool (use `INSERT ... RETURNING`), Any-incompatible column types (TEXT for timestamps, INTEGER for booleans), test-fixture TempDir leak gotcha, FromRow auto-derive incompatibility with chrono::DateTime fields. Capture in a developer-facing doc (e.g., `docs/v0.2/anypool-patterns.md` or a CONTRIBUTING section).

**#81 Investigate AnyPool last_insert_id() returning None on SQLite.** *Companion to #80.* During Phase 3, every call to `AnyQueryResult::last_insert_id()` returned None on SQLite, breaking ~30 admin tests. Worked around by converting all sites to `INSERT ... RETURNING id`. Worth a small spike: is this sqlx documented behavior, a config-flag miss, or a sqlx bug worth filing upstream?

**#82 Admin UI display polish (parent).** Three render-vs-shape mismatches surfaced during #109 functional verification:
- **#84 System Health page display fields render as empty.** Database backend, memory, disk, validation failures lookups don't match handler response shape.
- **#85 Sequencer page state and position render as unknown/empty.** Same diagnostic pattern.
- **#86 listRoles handle resolution — server-side enrichment.** Settings → Roles displays raw DIDs instead of @handle. Server-side fix: mirror `aurora_moderator::resolve_handles` for the listRoles handler.

**#83 v0.2 cycle aftermath (parent).** Three design / spec follow-ups:
- **#87 §8.15 canonical capability vocabulary cleanup.** `invite-lineage-v1` and `reporter-context-v1` are listed but have no endpoint commitments. Two paths: ship the corresponding endpoints, or remove from §8.15 with a Section 14 note.
- **#88 WebSocket auth subprotocol path for subscribeModEvents.** Phase 3.9 originally implemented WebSocket; auth via Sec-WebSocket-Protocol was unreachable from browsers (no custom Authorization headers on WebSocket upgrades). Substrate switched to HTTP polling in commit b480353. Polling is fine for the admin UI's tab-level live tail; for higher-frequency feeds, WebSocket is desirable. Three plumbing options: cookie-based auth, query-param token auth, Sec-WebSocket-Protocol parsing on the server side.
- **#89 Rate limiter exemption — dynamic /admin/* path scope.** v0.2 exemption is path-prefix + GET-only. If future features add dynamic /admin/* paths, the exemption rule should remain narrowly scoped. Documentation/process work, not a code bug.

**#90 mountSidebarFooter dead code at app.js:43-44.** *UI cleanup.* Lines 43-44 run before mountSidebar()/mountSidebarFooter() are called; the `admin-role` element doesn't exist yet so `getElementById` returns null. mountSidebarFooter is the single source of truth for that element's content.

**#91 RateLimitConfig.enabled flag is not consumed by middleware.** *Setting `PDS_RATE_LIMITS_ENABLED=false` has no effect today.* Two paths: wire the flag through, or remove the env var and config field if disable-entirely is not actually wanted operationally.

**#92 aurora_capability_extensions hardcoded match against route table.** *Drift risk: future endpoint removal without pulling capability from advertised list.* Add build-time route audit (procedural macro or build.rs), test-time route audit (integration test hitting OPTIONS), or a simple pinned `capability_string → required_nsids` table with a unit test asserting consistency.

**#94 Substrate JS test coverage gaps.** *Three primitives lack JS tests.* Subscription primitive (polling-mode rewrite from commit b480353), SettingsRoles page (`groupRoles()` pre-pass from commit a6dcd68), session role resolution (resolution chain fix from commit ef4dda9).

---

## §5 Out of scope by design

Items deferred indefinitely — assessed and out-of-scope as architectural choices, not as v0.3 candidates by themselves. Pulled from [AURORA_DESIGN.md §8.3](AURORA_DESIGN.md) and [AURORA_ADMIN_UI_DESIGN.md §14](AURORA_ADMIN_UI_DESIGN.md).

**Server architecture (§8.3):**
- **Per-actor stores on Postgres.** The hybrid model (per-actor SQLite, shared-state configurable) is the architecture, not a transitional state.
- **Wholesale ORM replacement.** Staying with sqlx; not switching to Diesel, SeaORM, or any other ORM.
- **Sharding across multiple Postgres instances.** Aurora-Locus operates against a single Postgres backend.
- **Multi-region/multi-bucket S3 configurations.**
- **Blob storage backends beyond Disk and S3.**
- **Wholesale UI framework rewrite.** The admin UI extends the existing multi-page SPA pattern.

**Forensic and audit (§14.3):**
- **Time-bounded historical export.** `exportAccountForensic` produces current-state bundles. Reconstructing past account states for historical export requires sequencer replay infrastructure not in v0.2 scope.
- **Bulk forensic export.** Single-account only in v0.2.

**Render and content handling (§14.5):**
- **Maximally hardened SSR for record render.** v0.2 ships server-side render with sanitization and media proxy. The most hardened pattern (full SSR with no JS execution context, dedicated sandboxed render environment) is deferred.
- **Federated cross-PDS subject views.** Cross-PDS context fetching is not in v0.2 scope.
- **Rich text editing for rationale fields.** Rationale fields are plain `<textarea>`. Markdown rendering, rich text controls, @-mentions are deferred.

**Multi-tenant (§14.6):**
- **Multi-tenant or per-namespace UI configuration.** v0.2 assumes a single Aurora-Locus deployment serving a single set of operators.
- **Wholesale visual redesign.** v0.2 preserves the current visual identity. A future cycle could propose redesign as its own scope.

**Operator workflow (§14.2):**
- **Saved filter views, operator activity dashboards, notification feed in sidebar, command palette enhancements, dashboard widget customization.** All deferred. Operators using v0.2 share filter URLs as the saved-view substitute; future cycles add features as operator usage indicates value.

**Distributed primitives (§5.6 of AURORA_DESIGN.md):**
- **Distributed rate limiting.** Redis or Postgres-CAS token bucket required. Per-process limits ship in v0.2.
- **OAuth state and DPoP nonces multi-instance.** Per-process limitations preserved.

These exclusions are first-principles architectural choices. v0.3 cycle planning should not re-litigate them; if scale or product direction motivates revisiting, that's a separate architectural conversation.

---

## §6 Chainlink references

Numerical index for cross-referencing the handoff against the chainlink db. Closed and open status as of cycle close.

### §6.1 Closed (v0.2 cycle work)

| ID | Title | Workstream |
|---|---|---|
| #1–#14 | Pre-cycle proto-blue migration | Workstream A |
| #2–#4 | Admin endpoints (sendEmail, invite mgmt, account mgmt) | Phase 1 prep |
| #19–#21 | AppView proxy + checkSignupQueue + audit log endpoints | Phase 1 prep |
| #27–#28 | Federation + rate-limit endpoints | Phase 1 prep |
| #34–#36 | Identity rate limit + invites + email tokens | Phase 1 prep |
| #45 | Identity resolution retry | Pre-cycle |
| #47–#52 | Multibase fix + DPoP fix | Audit round 1 |
| #55, #71–#73 | S3 blob storage activation | Workstream B (S3 sub-track) |
| #56–#66 | Phase 1.1–1.11 (parity + lexicon-shape) | Phase 1 |
| #68 | Admin test fixtures: tempdir creation race | Phase 1 |
| #74–#78, #103, #110 | Postgres backend Phases 1–5 + sequencer dedicated connection + multi-instance test fix | Workstream B |
| #95–#96, #98, #104 | Audit round 2 fixes (security + scope tightening + UI subject→did + reason mapping) | Audits |
| #97, #99–#102, #105–#108, #111–#115, #117–#122, #128–#129 | Cycle-mid substantive work + audit round 3 closure | Phase 3 + Audits |
| #109 | L-1 audit chain coverage for all administrative actions | Phase 3.8 / audit round 3 |
| #116 | Documentation refactor | Refactor pass |
| #127 | LB-1-followup (closed by Session 11/#128 in v0.2) | Audit round 3 |
| #132–#142 | Audit 4 fix-up (F1–F4, P1–P2, S1–S5) | Audits |
| #143–#153 | Audit 5 fix-up (CR-1 axum, CR-3 backoff, CR-4 audit-trail.json, P-2/P-3, SL-1–SL-6) | Audits |

### §6.2 Open (v0.3 plan)

| ID | Title | Category |
|---|---|---|
| #67 | Fix PlcClient::keys_match prefix-stripping bug | Parity gap |
| #69 | Fix audit log column misuse: send_email passes subject as ip_address | Parity gap |
| #70 | Add record-level moderation infrastructure (set/clear takedown_ref) | Substantive feature |
| #79 | End-of-cycle README refresh | Cleanup |
| #80 | Document AnyPool/chrono patterns established in v0.2 cycle | Cleanup |
| #81 | Investigate AnyPool last_insert_id() returning None on SQLite | Cleanup |
| #82, #84–#86 | Admin UI display polish (parent + 3 subissues) | Cleanup |
| #83, #87–#89 | v0.2 cycle aftermath: design / spec follow-ups (parent + 3 subissues) | Audit deferrals |
| #90 | mountSidebarFooter dead code at app.js:43-44 | Cleanup |
| #91 | RateLimitConfig.enabled flag is not consumed by middleware | Cleanup |
| #92 | aurora_capability_extensions hardcoded match against route table | Cleanup |
| #93 | OAuth admin login: session persistence for AS-only DIDs | Substantive feature (security-flavored, HIGH priority) |
| #94 | Substrate JS test coverage gaps | Cleanup |
| #113 | Batch ops end-to-end atomicity per subject (also audit 5 P-1) | Audit deferral |
| #123 | LB-3 — Runtime route enumeration in describeCapabilities | Audit deferral (round 3) |
| #124 | CR-3 — File-tier configuration for runtime settings | Audit deferral (round 3) |
| #125 | CR-5 — Compositional ModEvent shape | Audit deferral (round 3) |
| #126 | CR-6 — TimeRange wrapper + u32 retype | Audit deferral (round 3) |
| #130 | emit_event dispatch_action: atomic with chain entry | Audit deferral |
| #131 | BlobQuarantine: _in_tx variants for unified transaction | Audit deferral |
| #155 | CR-2 — PDS_ADMIN_DIDS dead-config decision | Audit deferral (audit 5) |

Plus the EPIC parent (#1) and the workstream parent (#54 Admin/moderation parity + Aurora extensions), both of which remain open as multi-cycle epics.

---

## §7 Reading order for review

Suggested path for a reviewer wanting to do a thorough pre-merge read. Total reading load: ~7,400 lines across the corpus + this handoff (~1,000-1,200 lines).

1. **[AURORA_DESIGN.md §1](AURORA_DESIGN.md)** (cycle scope and intent, design principles, relationship to upstream) — ~50 lines, sets the frame
2. **This handoff §1 + §2** (cycle summary + audit history) — ~100 lines, tells you what shipped and how confident the implementation is
3. **[AURORA_DESIGN.md §6](AURORA_DESIGN.md)** (proto-blue migration record) — read this first if reviewing the SDK migration; otherwise skim
4. **[AURORA_DESIGN.md §3](AURORA_DESIGN.md)** (lexicon-shape audit, as-shipped) — verifies the parity floor; ten clean, one mostly-clean
5. **[AURORA_DESIGN.md §4](AURORA_DESIGN.md)** (Admin/moderation Phase 3 — the substantive cycle work) — the four-namespace structure, foundation types (Subject, ModEventAction), schema additions (audit_chain_entry, audit_snapshot, mod_event_seq), per-namespace API surface
6. **[AURORA_DESIGN.md §5 + §7](AURORA_DESIGN.md)** (Postgres Phase 4 multi-instance + per-file coupling work) — read together; §5 is the multi-instance design, §7 is the per-file refactor reality
7. **[AURORA_ADMIN_UI_DESIGN.md](AURORA_ADMIN_UI_DESIGN.md)** if reviewing UI — large doc (~6,250 lines); skim TOC, drill into §3 (architecture principles), §4 (information architecture), §6 (substrate primitives), §8 (forthcoming endpoint commitments — i.e., the as-built endpoint specs)
8. **This handoff §3 per workstream** as drilldown into the cycle's actual choices
9. **This handoff §4 + §5** (forward planning + out-of-scope discipline)
10. **[AURORA_ENDPOINT_INVENTORY.md](AURORA_ENDPOINT_INVENTORY.md)** as the endpoint surface reference for cross-checking

For a fast review (you trust the architectural choices, want to validate readiness): just §1 + §2 of this handoff + skim §3 by workstream. The audit history's two consecutive zero-load-bearing rounds is the readiness signal; the per-workstream summaries point at the substantive design decisions and their corpus locations.

For a deep review: read in the suggested order above. The corpus + this handoff together describe both the as-shipped state (corpus) and the cycle's path to it (handoff).

---

## §8 Surfacing notes

Items surfaced during handoff generation that weren't anticipated in the original prompt:

1. **CR-2 from audit 5 had no chainlink at handoff time.** The audit-5 fix-up commit (e9e687a) noted CR-2 as deferred "to v0.3 cleanup" but no chainlink was filed. The handoff prompt assumed both CR-2 and P-1 had explicit chainlinks. Filed during this session as #155 for completeness.

2. **Three Postgres-phase chainlinks (#76, #77, #78) were stale-open.** The Phase 3 query-layer work, Phase 4 multi-instance support, and Phase 5 production primitives all clearly shipped (per AURORA_DESIGN.md §5/§7 and per git commit history: b851678, 7cc5b1b, 72f8a6d, 494d155, 129bb13, fc57aa2 plus the Phase 5.0.x SQL placeholder sweeps). The chainlinks remained open during the cycle — likely because they were active-work trackers that nobody manually closed when the work shipped. Closed during handoff generation with closure comments citing the relevant commits, then accurately described as closed in §3.2 / §3.7. doll may want to audit the closure comments if comprehensive verification is desired.

3. **AURORA_DESIGN.md §8.2's "S3 blob storage activation" deferral line is stale.** The S3 work shipped (#71–#73 closed; AWS SDK active in Cargo.toml at line 57; `src/blob_store/s3.rs` active in `src/blob_store/mod.rs`). §8.2 describes "The S3 path remains commented out at the dependency and module-export levels" which contradicts the as-built state. Not a load-bearing inconsistency for this handoff (§1 of the handoff describes S3 as shipped); flagged for a future minor doc fix in v0.3.

4. **EPIC #1 and parent #54 remain open as multi-cycle wrappers.** Standard cycle hygiene; no action needed.

5. **CHANGELOG management throughout.** Chainlink auto-appended entries to the CHANGELOG `Changed` section as each chainlink closed. The CHANGELOG entries for #76, #77, #78 closures (from this session) and for #155 filing (from this session) are present.

---

## §9 PR description suggestions

This handoff is a companion to the upstream PR. The PR description should:

- Excerpt §1 (cycle summary) and §2 (audit history table) verbatim. They're the canonical readiness narrative.
- Link to this handoff (`docs/V0_2_CYCLE_HANDOFF.md`) for the full per-workstream drilldown and the v0.3 plan.
- Link to the corpus (`docs/AURORA_DESIGN.md`, `docs/AURORA_ADMIN_UI_DESIGN.md`, `docs/AURORA_ENDPOINT_INVENTORY.md`) for the as-shipped design.
- Note the two consecutive zero-load-bearing audit rounds as the implementation-readiness signal.
- Acknowledge the two explicit v0.3 deferrals from audit 5 (CR-2 → #155, P-1 → #113) so reviewers see them as named choices rather than missed work.
- Note that the cycle ran against `c2d6fd2` (the proto-blue baseline) and that all upstream tests + cycle-added tests pass (732/732 lib tests at last verification).

A reviewer following the PR can read the description in 2-3 minutes for the cycle shape, then drill into the handoff for any specific workstream they want to validate, then drill into the corpus for the design substance. The three layers (PR description → handoff → corpus) compose for fast scan or deep audit depending on the reviewer's posture.

---

*End of V0_2_CYCLE_HANDOFF.md*
