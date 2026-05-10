# Aurora-Locus v0.2 Design Document

**Status:** v0.2 cycle complete; this is the as-shipped reference.
**Companion:** [AURORA_ADMIN_UI_DESIGN.md](AURORA_ADMIN_UI_DESIGN.md) — admin UI layer.
**Endpoint inventory:** [AURORA_ENDPOINT_INVENTORY.md](AURORA_ENDPOINT_INVENTORY.md).

This document consolidates the cycle's server-side design work — the cycle-opening assessments, the lexicon-shape audit, the per-phase design docs, and the migration records — into one navigable reference. It captures what v0.2 ships, why each piece was scoped the way it was, and where the cycle's design thinking deferred to v0.3.

---

## §1 Overview

### §1.1 Cycle scope and intent

The v0.2 cycle delivered four substantive workstreams against an upstream baseline (`c2d6fd2`, the proto-blue migration):

- **Workstream A — Proto-blue migration.** Replace the bundled `Rust-Atproto-SDK` with `proto-blue 0.2.6`. See §6.
- **Workstream B — Postgres backend.** Selective Postgres for the two shared-state databases; per-actor stores stay SQLite. See §5 and §7.
- **Admin/moderation Phase 1–2 (parity).** Close the bsky-PDS-2025-Q1 parity gaps under `com.atproto.admin.*`; relocate operator-flavored endpoints to `tools.aurora.ops.*`. See §3 (lexicon-shape audit driving the parity gaps) and §8.
- **Admin/moderation Phase 3 (Aurora extensions).** Ship `tools.aurora.{describeCapabilities,moderator,admin,superadmin}.*` with the audit-chain, snapshot-at-decision, retention-bounded subscription channel, and forensic-export infrastructure that bsky-PDS doesn't expose. See §4.

Plus the documentation refactor that produced this consolidated reference (the v0.2 cycle's final pass).

### §1.2 Design principles

The cycle's design is shaped by a small set of principles — most articulated explicitly in [AURORA_ADMIN_UI_DESIGN.md §3](AURORA_ADMIN_UI_DESIGN.md), summarized here for reference from the server-side perspective:

1. **Server authority is total; the client is untrusted.** Every authority-bearing decision (role check, scope check, action authorization) is enforced server-side. The admin UI is a peer client of the same APIs operator tools and external clients call.
2. **Subject-shape determines action set.** `Subject::{Repo,Record,Blob}` is the polymorphic vocabulary; lexicon is the source of truth for which actions apply where. Per [§4.1.1](#§411-subject-decision-b).
3. **Snapshots-at-decision and audit chain are co-equal substrate.** The chain says *who decided what when*; the snapshot says *what the subject looked like at decision time*. Together they answer the forensic question. Per [§4.4](#§44-schema-additions).
4. **Real-time is for signal arrival; everything else polls.** `subscribeModEvents` is for surfaces where event-arrival latency is itself the signal; UI surfaces refresh on poll or user action everywhere else. Per [§4.3.1](#§431-toolsauroramoderator).
5. **Decoupling is structural, not nominal.** Aurora-Locus interoperates with the broader ATProto ecosystem and external moderator tooling without naming, preferring, or detecting any specific external system.
6. **PDS authority is bounded by network posture.** Aurora-Locus exposes the full administrative surface a PDS legitimately controls; deployment posture (paired-with-external-labeler vs independent) determines the practical reach of those actions, not the API contract.

### §1.3 Relationship to upstream

Aurora-Locus is downstream of the bsky-PDS reference design. The cycle treats bsky-PDS-2025-Q1 as the parity floor and adds Aurora-specific extensions on top under `tools.aurora.*`. Files Doll authored pre-cycle (see [README.md](../README.md), [ARCHITECTURE.md](../ARCHITECTURE.md), [FEDERATION.md](../FEDERATION.md), the parity-assessment markdown files at the repo root, and the operator/OAuth guides under `docs/`) cover the upstream baseline and remain authoritative for those domains. The cycle did not modify those files except where Block 6 (Session 7) updated [README.md](../README.md) and [QUICKSTART.md](../QUICKSTART.md) to remove the `PDS_ADMIN_DIDS` env-var auto-grant.

This document is the cycle's design surface; it does not duplicate the upstream baseline. Where a topic is fully covered by Doll's pre-cycle docs, this doc references them rather than restating.

---

## §2 Cycle-opening assessments

The cycle began with three assessment documents drafted on 2026-04-30 (one day before substantive work began on 2026-05-01). Each surveyed a domain Aurora-Locus needed to address, identified the parity gaps and extension surface, and proposed an implementation phasing. Their substance is captured below; see commit `3dc921f` for the original drafts.

### §2.1 Admin and moderation parity

**Reference target:** bsky-PDS 2025-Q1 (matches Doll's other parity assessments under `*_ASSESSMENT.md`).

**State at cycle open.** Aurora-Locus already shipped a substantial admin and moderation surface — three-tier role hierarchy (`Moderator`/`Admin`/`SuperAdmin`), OAuth scope hierarchy (`atproto:admin.*` / `atproto:admin.moderation` / `atproto:admin.server`), an audit-log substrate, and ~30 endpoints under `com.atproto.admin.*`. The gap was not "build moderation from scratch" but "bring the existing surface to bsky-PDS parity and extend it with capabilities the architecture affords."

**Parity gaps closed in Phase 1:**

- `disableInviteCodes` (plural) — bulk operation for spam-ring cleanup
- `getAccountInfo` (singular) — modern naming alongside legacy `getAccount`
- `searchAccounts` — email-keyed account search per the published lexicon
- `updateAccountSigningKey` — wire the existing PLC rotation logic to a public XRPC endpoint
- Polymorphic `updateSubjectStatus` — accept `repoRef`/`strongRef`/`repoBlobRef` per the modern lexicon (and the structured `takedown`/`deactivated` patches per [§3 lexicon-shape audit](#§3-lexicon-shape-audit))

**Phase 2 — namespace cleanup:** ~30 operator-flavored endpoints relocated from `com.atproto.admin.*` to `tools.aurora.ops.*` with `atproto:admin.server` scope, leaving `com.atproto.admin.*` as the slim parity surface bsky-PDS exposes.

**Phase 3 — Aurora extensions:** the four-namespace structure (`tools.aurora.{describeCapabilities,moderator,admin,superadmin}.*`) detailed in [§4](#§4-admin-and-moderation-phase-3) below.

**The Rust opportunity** that motivates the extensions: the sequencer is a Rust + axum + WebSocket primitive that already streams a firehose; extending it to a moderation channel is incremental work. Postgres's transaction model makes batch atomicity natural for multi-subject operations. The existing `AuditLogEntry` substrate is already richer than what bsky-PDS exposes. Per-actor SQLite isolation means cross-subject batch operations don't serialize through a shared lock. None of these are specific to Rust — they're consequences of architectural choices Aurora-Locus made and bsky-PDS did not.

### §2.2 Blob storage (S3) feasibility

**State at cycle open.** Aurora-Locus's S3 blob storage support was partially scaffolded but actively disabled: `BlobBackend` trait, `BlobBackendType::S3` enum variant, ~320 lines of working `aws-sdk-s3` integration in `src/blob_store/s3.rs`, but Cargo dependencies commented out, module exports commented out, and `AppContext` always selecting the disk backend. The cited reason — "AWS SDK build issues on Windows" — does not justify disabling production capabilities for everyone; PDS deployments run on Linux.

**Gaps relative to bsky-PDS:**

1. **S3 backend not reachable.** Activation requires uncommenting AWS SDK dependencies and module exports.
2. **Configuration not wired into `AppContext`.** Need env-var loading matching bsky-PDS's `PDS_BLOBSTORE_S3_*` conventions plus mutex validation against `PDS_BLOBSTORE_DISK_LOCATION`.
3. **Two missing `S3Config` fields:** `force_path_style: bool` (required for MinIO) and `upload_timeout_ms: u64` (default 20000).

**Serving-path compatibility (already in place).** The blob-serving handler in `src/api/blob.rs` emits CDN-friendly headers (`Cache-Control: public, max-age=31536000, immutable`, content-addressed `ETag`, range-request handling) and the global CORS layer permits cross-origin embedding. CDN deployment is already supported architecturally; the deployment-side CDN configuration is operator concern.

**Out of scope for v0.2.** Signed URL generation; Aurora-driven CDN purge on takedown (deferred to a later admin/moderation extension); live disk → S3 migration tooling; multi-region/multi-bucket configurations; backends beyond Disk and S3.

**Status post-cycle.** S3 backend support shipped in v0.2: AWS SDK dependencies are live in `Cargo.toml`, `src/blob_store/s3.rs` is exported from `src/blob_store/mod.rs`, and `AppContext` selects between Disk and S3 via the `BlobstoreConfig` enum based on the `PDS_BLOBSTORE_*` env vars. The assessment's three-phase plan landed in full.

### §2.3 Postgres backend feasibility

**State at cycle open.** Aurora-Locus's Postgres support was partially scaffolded — `Cargo.toml` declared both `sqlite` and `postgres` features for sqlx, `src/db/postgres.rs` defined a `PostgresConfig` with env-driven configuration plus `create_pool` and `run_migrations` functions, and `migrations/postgres/` existed as a placeholder directory. None of this was wired into the application.

**The architectural decision shaping everything else.** Aurora-Locus operates **three logically distinct database surfaces**, each with different access patterns:

1. **`account_db`** — global state shared across all actors (accounts, sessions, OAuth, invites, moderation queue, labels, blobs, mailer tracking, sequencer events). High concurrent writes; fan-out reads.
2. **`did_cache_db`** — DID document and handle resolution cache. Read-heavy with periodic TTL eviction.
3. **Per-actor `repo.sqlite`** — one SQLite file per user under `data/actors/<did>/repo.sqlite`. Holds the actor's MST state, records, and repo blocks. Lazy pool creation per-DID via LRU cache.

The first two benefit from Postgres in production deployments. **Per-actor state stays SQLite** — Postgres can't naturally do "one database per user" without either schema-per-actor (operationally awful at scale) or shared tables with `actor_did` columns (which loses the actor-isolation property bsky-PDS deliberately preserves). Per-actor SQLite gives single-file backup/export/deletion semantics and matches bsky-PDS's design choice.

The cycle's scope was therefore **selective Postgres**: shared global state migrates to a configurable backend (SQLite for hobbyist deployments, Postgres for production); per-actor state always stays SQLite. Both deployment paths are first-class.

The assessment's five-phase plan (Phase 1 schema → Phase 2 backend selection → Phase 3 query-layer compatibility across 16 files → Phase 4 multi-instance support → Phase 5 production primitives) drove the cycle's Postgres work. See [§5](#§5-postgres-backend-phase-4) for the multi-instance design as shipped, [§7](#§7-sqlitepostgres-coupling-work) for the per-file coupling-audit findings.

---

## §3 Lexicon shape audit

**Scope.** Eleven `com.atproto.admin.*` endpoints that Aurora-Locus already implemented at name-parity with bsky-PDS, audited at cycle open for **shape** parity per [§2.1](#§21-admin-and-moderation-parity). The original audit was conducted against the atproto monorepo's published lexicons at the 2025-Q1 reference target. The cycle's Phase 1 sub-phases (#61–#65) closed the audited drifts; this section documents the as-shipped state.

**Aurora-Locus surface convention.** Lexicons are not stored as JSON files in this repo; request and response shapes are inline in Rust handlers in `src/api/admin.rs`. The "lexicon surface" enumerated below is the route table at [src/api/admin.rs:38-350](../src/api/admin.rs#L38-L350) plus handler modules under `src/api/`. Each endpoint cites the relevant request/response struct line range as currently shipped.

**Verdict legend:**

- **Clean** — every input/output field matches the lexicon in name, type, and required-ness; no extra fields beyond the spec, no extra response payload on procedures that declare none.
- **Mostly clean** — input/output fields match the lexicon, with one documented Aurora extension that is omitted from the wire when not populated (so spec-strict consumers see a spec-shaped payload).
- **Outstanding drift** — material wire-breaking divergence. None remain after the Phase 1 work.

### §3.1 Per-endpoint findings (as-shipped)

**deleteAccount** ([src/api/admin.rs:1331-1399](../src/api/admin.rs#L1331-L1399)) — *Clean.* Returns `Result<StatusCode, ...>` with no response body, matching the lexicon's no-output declaration.

**disableAccountInvites** ([src/api/admin.rs:4122-4137](../src/api/admin.rs#L4122-L4137), [4205-…](../src/api/admin.rs#L4205)) — *Clean.* Phase 1.7 (#62) introduced `account` (at-identifier) as the primary input field per lexicon; legacy `did` field retained for back-compat with deprecation note. Optional `note` field accepted and persisted to the chain rationale. Returns OK no body.

**enableAccountInvites** ([src/api/admin.rs:4122-4137](../src/api/admin.rs#L4122-L4137), [4140-4202](../src/api/admin.rs#L4140-L4202)) — *Clean.* Same shape as `disableAccountInvites`; shares the `AccountInvitesRequest` struct.

**getAccountInfos** ([src/api/admin.rs:2582-2589](../src/api/admin.rs#L2582-L2589), [2641-2750](../src/api/admin.rs#L2641-L2750)) — *Clean.* Phase 1.9 (#64) replaced the legacy comma-separated single-string `dids` encoding with the lexicon-conformant repeated-param form (`?dids=a&dids=b`) via `axum_extra::extract::Query`. `accountView.handle` flipped from `Option<String>` to `String` (the underlying `actor.handle` column is `NOT NULL`).

**getInviteCodes** ([src/api/admin.rs:525-543](../src/api/admin.rs#L525-L543), [605-637](../src/api/admin.rs#L605-L637)) — *Clean.* Phase 1.10 (#65) wired the lexicon's `sort` (`recent`/`usage`) + `limit` (1–500, default 100) + `cursor` (typed, base64url) parameters. Legacy `includeDisabled` removed; disabled-only filtering relocates to `tools.aurora.ops.*` per the assessment doc.

**getSubjectStatus** ([src/api/admin.rs:3789-3812](../src/api/admin.rs#L3789-L3812), [3842-…](../src/api/admin.rs#L3842)) — *Mostly clean (residual: `suspended` Aurora extension, blob-branch 501).* Subject union implemented correctly via polymorphic struct with `$type` discriminator. `takedown` and `deactivated` now use `skip_serializing_if = "Option::is_none"` and are omitted from the wire when not populated. Aurora-only `suspended` field remains as a documented extension on the response struct, also omitted when None — spec-strict consumers see a payload with no extra fields when the account isn't suspended. The `suspended` extension is the v0.3 candidate to either relocate to `tools.aurora.*` or fold into `takedown` semantics. The blob branch (`?blob=<cid>`) returns 501 NOT_IMPLEMENTED at [src/api/admin.rs:3882-3887](../src/api/admin.rs#L3882-L3887) because per-blob status state isn't tracked yet — analogous to `tools.aurora.moderator.queryStatuses`'s `subject_type=Record|Blob` short-circuit. Per [§8.2](#§82-deferred-to-v03), `subject_status` only tracks repo-level state today; per-record/per-blob status is a v0.3 candidate, at which point the 501 branch fills in.

**sendEmail** ([src/api/admin.rs:2247-2266](../src/api/admin.rs#L2247-L2266), [2279-…](../src/api/admin.rs#L2279)) — *Clean.* Phase 1.8 (#63) flipped `subject` from required→optional per the lexicon (placeholder used at SMTP layer when omitted). `senderDid` is documented as an Aurora-permissive extension: spec marks it required, Aurora defaults to the authenticated admin's DID when omitted; spec-compliant callers pass an explicit value. The Aurora extension is opt-in by the *caller*, not produced on the wire by the *server*, so consumers reading server output are unaffected.

**updateAccountEmail** ([src/api/admin.rs:1086-1099](../src/api/admin.rs#L1086-L1099), [1102-…](../src/api/admin.rs#L1102)) — *Clean.* Phase 1.7 (#62) introduced `account` (at-identifier) as primary input; legacy `did` retained for back-compat with deprecation note. Returns OK no body.

**updateAccountHandle** ([src/api/admin.rs:1176-1181](../src/api/admin.rs#L1176-L1181), [1184-1251](../src/api/admin.rs#L1184-L1251)) — *Clean.* Field names match lexicon; returns OK no body.

**updateAccountPassword** ([src/api/admin.rs:1253-1259](../src/api/admin.rs#L1253-L1259), [1262-1329](../src/api/admin.rs#L1262-L1329)) — *Clean.* Field names match lexicon; returns OK no body.

**updateSubjectStatus** ([src/api/admin.rs:3401-3422](../src/api/admin.rs#L3401-L3422), [3437-…](../src/api/admin.rs#L3437)) — *Clean.* Phase 1.6 (#61) replaced the imperative-action model (`suspend`/`takedown`/`restore` verb) with the lexicon's declarative status-patch model (`{takedown?: StatusAttr, deactivated?: StatusAttr}`). Subject dispatch covers `repoRef` (both patches), `repoBlobRef` (`takedown` only; `deactivated` rejected as inapplicable), and `strongRef` (`takedown` returns 501 pending a record-takedown setter; `deactivated` rejected as inapplicable to records). Response echoes `subject + takedown?` per spec; `deactivated` is intentionally not echoed since the lexicon doesn't carry it on the response.

### §3.2 Summary (as-shipped)

| Verdict bucket | Endpoints | Count |
|---|---|---|
| Clean (full shape parity) | `deleteAccount`, `disableAccountInvites`, `enableAccountInvites`, `getAccountInfos`, `getInviteCodes`, `sendEmail`, `updateAccountEmail`, `updateAccountHandle`, `updateAccountPassword`, `updateSubjectStatus` | 10 |
| Mostly clean (residual: `suspended` Aurora extension + blob-branch 501) | `getSubjectStatus` | 1 |
| Outstanding drift | (none) | 0 |

### §3.3 Recurring drift patterns (resolution status)

The five drift patterns enumerated at cycle open have all been substantially or entirely resolved by Phase 1:

1. **Procedures emitting non-spec response bodies.** *Resolved.* All audited procedures now return `Result<StatusCode, ...>` (OK with no body) per spec. The legacy `{success, did, message}`-style envelopes were dropped during Phase 1 alongside the LB-1 chain-entry refactor (#122) which restructured every admin handler.
2. **`account` (at-identifier) parameters renamed to `did` (DID-only string).** *Resolved* on `disableAccountInvites`, `enableAccountInvites`, `updateAccountEmail` via Phase 1.7 (#62). Each accepts `account` (lexicon-conformant, handle or DID) as the primary input with `did` retained as a deprecated back-compat field documented for removal in a later minor version.
3. **`Option<T>` fields serialised as always-present with sentinel values.** *Resolved.* `getSubjectStatus` and `getAccountInfos` now use `skip_serializing_if = "Option::is_none"` consistently. The two endpoints' response shapes match the lexicon's omission semantics.
4. **Pagination unimplemented on `getInviteCodes`.** *Resolved* via Phase 1.10 (#65). Sort/limit/cursor triple wired with typed `InviteCursor` enum that pins ordering. (`listInviteCodes` was the operator-flavored cousin and relocated to `tools.aurora.ops.*` during Phase 2.3 with proper pagination.)
5. **`updateSubjectStatus` structural divergence.** *Resolved* via Phase 1.6 (#61). The declarative status-patch model fully replaced the action-verb model.

**Format-validation note.** Aurora's request structs still don't enforce the lexicon's `format=did|handle|at-uri|cid|datetime` constraints at the serde layer; validation continues to be done ad hoc in handlers (e.g. `starts_with("did:")`). This isn't a shape divergence and it isn't user-visible on responses — it just means Aurora may accept inputs bsky-PDS would reject and vice versa at the format-validation layer. Tracked as a v0.3 candidate alongside the broader serde-layer-format-validation discussion.

---

## §4 Admin and moderation Phase 3

Phase 3 ships Aurora's extension surface — endpoints across four namespaces beyond the bsky-PDS-parity baseline established by [§2.1](#§21-admin-and-moderation-parity)'s Phases 1 and 2.

### §4.1 Foundation types

These types live in `src/admin/defs.rs` (mirroring how `tools.aurora.admin.defs` is the lexicon convention upstream) and are imported by per-namespace endpoint modules.

#### §4.1.1 Subject (decision B)

Three-variant `Subject` enum matching `com.atproto.admin.defs#repoRef` / `#strongRef` / `#repoBlobRef`. `$type`-discriminated serialization for wire compatibility with ATProto convention.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "$type")]
pub enum Subject {
    #[serde(rename = "com.atproto.admin.defs#repoRef")]
    Repo { did: String },
    #[serde(rename = "com.atproto.repo.strongRef")]
    Record { uri: String, cid: String },
    #[serde(rename = "com.atproto.admin.defs#repoBlobRef")]
    Blob { did: String, cid: String, record_uri: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubjectType {
    Account,  // matches Subject::Repo
    Record,
    Blob,
}
```

`SubjectType` is the filter-parameter form used by query endpoints that take a subject_type. Distinct from `Subject` (the value form) because filters need to be parsed from query strings without requiring a full subject identity.

#### §4.1.2 ModEvent (decision A)

Compositional, subject-agnostic event vocabulary for the API surface. Translation to/from the storage enum `ModerationEventType` (12 subject-aware variants — see [§4.4.5](#§445-storage-event-vocabulary)) happens at write time in `emitEvent` and at read time in `getAuditTrail` / event subscriptions.

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "kind")]
pub enum ModEventAction {
    // Representative variants — the full list will continue to evolve in v0.3.
    // See src/api/aurora_admin.rs::ModEventAction for the as-shipped enum
    // (currently includes Takedown/Suspend/Restore/DeleteAccount,
    // ApplyLabel/RemoveLabel, TakedownRecord, Quarantine/Restore/DeleteBlob,
    // Resolve/DismissReport, Resolve/EscalateAppeal, SendEmail,
    // UpdateSubjectStatus). Wire format: `{"kind": "TakedownAccount"}`
    // for unit variants; `{"kind": "ApplyLabel", "val": "spam", "neg": false}`
    // for variants with inline data.
    TakedownAccount,
    ApplyLabel { val: String, neg: bool },
    ResolveAppeal { #[serde(rename = "appealId")] appeal_id: i64, resolution: AppealResolutionDecision },
    // ... see code reference above
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppealResolutionDecision { Approve, Deny }
```

**API enum is subject-aware in v0.2.** `ModEventAction` is the wire-format shape — variants like `TakedownAccount` vs `TakedownRecord` are explicit per-subject-type rather than a compositional `Takedown` applied to any `Subject`. Clients constructing `ModEventAction` values choose the variant per the subject they're acting on.

> **Reconciliation.** Earlier drafts specified a `$type`-tagged compositional `ModEvent` enum (subject-agnostic — one `Takedown` variant applied to any `Subject`, with the lexicon as source of truth for which actions apply where) plus an `AppealResolution` enum with `Uphold | Reject | Modify`. As-built, Phase 3.5 collapsed the API enum into the storage shape (`ModEventAction` is `kind`-tagged with subject-aware variants); `AppealResolutionDecision` ships with `Approve | Deny`. The compositional revisit is a v0.3 candidate (chainlink #125); the wire-breaking nature of the change motivated its deferral.

**Why role grant/revoke aren't here** — they live at `tools.aurora.superadmin.*` ([§4.3.3](#§433-toolsaurorasuperadmin)) as dedicated endpoints rather than `ModEvent` variants. Keeping them out of `ModEvent` makes the SuperAdmin auth boundary structurally visible in the namespace.

#### §4.1.3 Pagination (decision E)

Standard pagination types reused across every list endpoint:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub cursor: Option<String>,  // None when no more pages
}

#[derive(Debug, Clone, Deserialize)]
pub struct PaginationParams {
    pub cursor: Option<String>,
    pub limit: Option<u32>,  // default 50, capped at 100
}
```

Cursor format: opaque to clients; internally a base64-encoded JSON `{after_created: DateTime<Utc>, after_id: i64}`. Composite cursor avoids collisions when multiple events share a timestamp (common during bulk operations).

#### §4.1.4 Errors (decision F)

Shared error vocabulary for Phase 3 endpoints:

```rust
#[derive(Debug, Clone, Copy)]
pub enum AuroraAdminError {
    SubjectNotFound,
    InvalidEvent,
    PermissionDenied,
    OutdatedCursor,
    UnknownEventVariant,
    AppealNotFound,
    BatchValidationError,
}
```

Wire format: ATProto error envelope `{"error": "<CodeName>", "message": "<optional human-readable>"}`. Per-endpoint error sets can extend (e.g., a procedure-specific validation error) but should reuse the shared vocabulary where it fits.

### §4.2 describeCapabilities surface and response shape

`tools.aurora.describeCapabilities` is the top-level capability probe — clients discover which Aurora extensions this instance supports without trial-and-error against endpoints. Type: query. Auth: any role with `atproto:admin.moderation` or higher scope.

**Response shape (as built):**

```json
{
  "families": {
    "tools.aurora.ops":        ["getStats", "listAccounts", ...],
    "tools.aurora.moderator":  ["queryEvents", "getEvent", "queryStatuses", ...],
    "tools.aurora.admin":      ["emitEvent", "batchTakedownAccounts", ...],
    "tools.aurora.superadmin": ["grantRole", "revokeRole"]
  },
  "extensions": [
    { "name": "subject-context-v1" },
    { "name": "moderator-activity-v1" },
    { "name": "audit-trail-v1" },
    { "name": "forensic-export-v1" },
    { "name": "mod-events-stream-v1" },
    { "name": "runtime-settings-v1" }
  ],
  "implementation": "aurora-locus",
  "version": "0.2.0"
}
```

Extension entries are objects with an optional `value` field for capabilities that need to advertise structured payload (e.g., `event-variants` carrying the supported `ModEvent` variant list). Bare-name extensions like `audit-trail-v1` declare behavioral commitments without payload.

> **Reconciliation (C2).** An earlier Phase 3 design draft modeled the response as `namespaces: Vec<NamespaceDescriptor>` with `NamespaceDescriptor { nsid, endpoints, version }`; the cycle-opening admin/moderation assessment used `families` keyed by NSID prefix plus a top-level `extensions` array. Implementation synthesized: `families` is a JSON object keyed by NSID prefix (assessment shape); a separate `extensions` array carries capability flags; top-level `implementation` and `version` are flat fields. The synthesized shape is what ships in v0.2 and is documented above.

The v0.2 implementation maintains a hand-curated capability list in `aurora_capability_families` keyed by namespace. New endpoints require an entry in that list to appear in the capability advertisement.

> **Reconciliation.** Earlier drafts of this section called for runtime route enumeration (walk the Router's table at server start, build the families dict from discovered routes) to eliminate the drift maintenance burden. The static-list approach was chosen for v0.2 because axum's Router introspection is non-trivial and the runtime-discovery refactor warrants its own session. v0.3 candidate (chainlink #123); the corpus reflects the as-shipped reality.

**§8.15 capability vocabulary** (the canonical extension names) is enumerated in [AURORA_ADMIN_UI_DESIGN.md §8.15](AURORA_ADMIN_UI_DESIGN.md). Two §8.15 capabilities are intentionally omitted from the v0.2 advertisement because the corresponding endpoints are not yet shipped (`invite-lineage-v1`, `reporter-context-v1`); they will be added when their handlers land.

### §4.3 Per-namespace API surface

Each Phase 3 sub-phase that adds handlers does so in a dedicated `src/api/aurora_<tier>.rs` module rather than extending `src/api/admin.rs` (which already exceeds 7,500 lines from Phases 1–2). Phase 3.3 established the pattern with `src/api/aurora_moderator.rs`; sub-phases 3.4, 3.5, 3.7, 3.8, 3.9 follow the same shape (one module per `tools.aurora.<tier>.*` namespace). Route registration still lives in `admin.rs::routes()` for visibility.

#### §4.3.1 `tools.aurora.moderator.*`

All require `atproto:admin.moderation` scope per the Phase 2 namespace check. Listed by sub-phase grouping:

**Reads (sub-phase 3.3).** `getSubjectStatus`, `getRecord`, `getRepo` — point-in-time-now subject views. Rich context (resolved handles, recent event summaries, blob metadata for embedded blobs, aggregate counts) so admin UI screens load complete subject state in one call rather than 5+ sequential queries.

**Queries (sub-phase 3.3 too).** `queryEvents` (paginated event timeline filterable by subject, type, actor, time range), `queryStatuses` (paginated current-status query — the moderation queue view; `subject_type=Record|Blob` short-circuits to empty pending per-record/per-blob status surfaces in v0.3).

**Appeals reads (sub-phase 3.4).** `listAppeals` and `getAppeal` with full event-history timeline. The action side of appeal lifecycle (resolving an open appeal) flows through `emitEvent`'s `appealResolve` event variant for consistency; the read side benefits from dedicated endpoints because admin UI appeal screens want appeals-specific shapes.

> **Reconciliation (C1).** An earlier Phase 3 design draft placed the audit-chain (`getAuditTrail`), aggregations (`getModerationMetrics`, `getQueueStats`), forensic export (`exportAccountForensic`), and live-event subscription (`subscribeModEvents`) under `tools.aurora.moderator.*`. [AURORA_ADMIN_UI_DESIGN.md §8](AURORA_ADMIN_UI_DESIGN.md#84-phase-38--toolsauroraadmingetaudittrail) (and the as-built `aurora_capability_families` in `src/api/admin.rs`) places them under `tools.aurora.admin.*`. The implementation aligns with the UI design doc; this consolidated reference reflects the as-built `tools.aurora.admin.*` placement (see [§4.3.2](#§432-toolsauroraadmin) below). The earlier `tools.aurora.moderator.*` placement was superseded.

#### §4.3.2 `tools.aurora.admin.*`

Auth: `atproto:admin.moderation` scope. Admin-tier role check at handler level for destructive operations.

**`emitEvent`** (sub-phase 3.5). Unified action surface; takes `(ModEvent, Subject, comment?)` and writes both the moderation_event row and the audit chain entry inside one transaction. Translates the API-shaped `ModEvent` to the storage-shaped `ModerationEventType` variant per [§4.4.5](#§445-storage-event-vocabulary).

**Batch operations** (sub-phase 3.5). Six endpoints: `batchTakedownAccounts`, `batchSuspendAccounts`, `batchRestoreAccounts`, `batchTakedownRecords`, `batchApplyLabel`, `batchRemoveLabel`. Multi-subject takedowns (a spam ring of N accounts taken down together with one chain entry linking all subjects) are a real moderation workflow that bsky-PDS doesn't expose. Per Arc 4 §8.4.2 (chainlink #113) atomicity is **whole-tx-atomic**: the moderation_event row, audit chain entry, and every per-subject mutation either ALL land or NONE do. Per-subject mutation failure aborts the wrapping tx; the response body's error envelope identifies the failing subject's index and identifier. The v0.2 `failures: Vec<BatchFailure>` field is retired. `batchRemoveLabel` keeps `skipped: Vec<Subject>` for subjects without the label to remove (no-op, not failure). Snapshot capture is the explicit pre-tx phase per `docs/V03_DESIGN.md` §8.3.1; orphan snapshots on capture failure are accepted as a deliberate carve-out (the chain entry never lands).

**Aggregations** (sub-phase 3.7). `getModerationMetrics` (events_total, events_by_type, appeals_total, appeals_by_resolution, takedowns_applied, takedowns_reversed, top_moderators) and `getQueueStats` (pending_appeals_count, open_reports_count, reports_awaiting_review_age_p50/p95).

**Audit chain** (sub-phase 3.8). `getAuditTrail` returns paginated `audit_chain_entry` rows with the per-row `verified` boolean (recomputed hash matches stored `current_hash`) and the chain-level `chainVerified` / `chainVerifiedThrough` fields (catches the consistent-rewrite attack where `current_hash` was rewritten in step with the content but the linkage between rows was missed). See [§4.4.2](#§442-audit_chain_entry-chain-semantics) for chain semantics.

**Forensic export** (sub-phase 3.8). `exportAccountForensic` produces a tamper-evident metadata bundle for a target DID. v0.2 ships metadata-only export with in-memory tar assembly; CAR data and blob bytes are deferred to v0.3. See [AURORA_ADMIN_UI_DESIGN.md §8.7](AURORA_ADMIN_UI_DESIGN.md) for the full endpoint specification including the §8.7 "Implementation status (v0.2)" subsection.

**Live event tail** (sub-phase 3.9). `subscribeModEvents` ships JSON-framed messages over a WebSocket: `Hello`, `Event`, `AuditEntry`, `Heartbeat`, `OutdatedCursor`, `Error`. Polls the retention-bounded `mod_event_seq` table on a 5-second tick (Phase 3.9+ optimization can swap in LISTEN/NOTIFY-driven push transparently — the wire protocol stays the same). Optional `includeAuditChain: true` parameter interleaves `audit_chain_entry` rows by timestamp order. Visibility gated to Moderator+ for chain entries (silent gate per §3.6 non-enumeration).

**Runtime settings** (sub-phase 3.10). `getRuntimeSetting` and `setRuntimeSetting` for the Phase 3.10 runtime-configuration system. Two known keys today: `moderation-mode` and `moderation-mode-redirect-url`.

#### §4.3.3 `tools.aurora.superadmin.*`

Auth: `atproto:admin.moderation` plus `Role::SuperAdmin` check at handler level via the existing `require_admin_role!` macro. Two endpoints:

**`grantRole`** (sub-phase 3.6). Relocated from `com.atproto.admin.grantRole`. Inputs: `did`, `role`, `notes`. Writes a row to `admin_roles` and emits the corresponding chain entry.

**`revokeRole`** (sub-phase 3.6). Relocated from `com.atproto.admin.revokeRole`. Marks the role record as revoked (preserving audit history) and emits the corresponding chain entry.

**Asymmetry note: `listRoles` stays put.** `grantRole` and `revokeRole` relocate because they're destructive role-authority operations requiring SuperAdmin tier. `listRoles` (read-side role discoverability) stays at `com.atproto.admin.listRoles` because it serves all moderators legitimately and doesn't require SuperAdmin tier. The asymmetry reflects "authority tier matches operation destructiveness" rather than "all role-related endpoints belong to one tier" — the same principle that keeps `getAuditLog` at the moderator tier while `emitEvent` (which writes to it) requires admin tier.

#### §4.3.4 `tools.aurora.ops.*`

Operator/infrastructure tier; the ~30 endpoints relocated from `com.atproto.admin.*` during Phase 2.3 plus two net-new endpoints (`listAccounts` with broader filters than the parity surface, `getInstanceMetrics` for operator-flavored aggregates).

Auth: `atproto:admin.server` scope (operator-level). `atproto:admin.*` (full) implicitly satisfies it. `atproto:admin.moderation` does not — moderators don't need server operations.

Full endpoint list in [AURORA_ENDPOINT_INVENTORY.md](AURORA_ENDPOINT_INVENTORY.md). Sub-clusters: system health, sequencer, blob ops, federation, rate-limit, validation/jobs/nonce, stats/accounts/metrics.

### §4.4 Schema additions

#### §4.4.1 admin_audit_log → audit_chain_entry migration history

The cycle's audit-log evolution had two phases:

**Sessions 1–6.** Hash-chain columns added to the existing `admin_audit_log` table — every new audit row cryptographically linked to the previous via `previous_hash` (= prior row's `current_hash`) and `current_hash` (= SHA-256 over canonical-serialized row content). Pre-chain rows received a `current_hash = "pre-chain"` sentinel and `verified = false` so consumers know which rows have actual cryptographic protection vs which predate the chain. Tracked under chainlink #97.

A new `audit_chain_entry` table was introduced alongside `admin_audit_log` for the richer chain entries (with `cascade_subjects`, `cascade_snapshot_ids`, `event_id` reference). The two tables coexisted during the transition.

**Session 7 (chainlink #109).** Full migration. Every administrative call site routed through `audit_chain::append_entry`. The legacy `AdminRoleManager::log_action` helper was dropped, the `admin_audit_log` table was dropped via migration `0004_drop_admin_audit_log.sql` (SQLite + Postgres variants). `audit_chain_entry` is now the single system of record for administrative decisions; `getAuditLog` reads from it with the legacy wire shape preserved for back-compat.

> **Reconciliation (C5).** Earlier drafts in the corpus disagreed on whether `audit_chain_entry` and `admin_audit_log` should coexist. The cycle's as-built reality is: dual-table during Sessions 1–6 (working migration), single-table after Session 7 (`audit_chain_entry` only). This consolidated reference documents the single-table reality.

#### §4.4.2 audit_chain_entry chain semantics

`audit_chain_entry` columns: `id`, `sequence`, `created_at`, `actor_did`, `action`, `subject_did`, `subject_uri`, `subject_cid`, `rationale`, `snapshot_id`, `event_id`, `current_hash`, `previous_hash`, `cascade_subjects` (JSON list, batch entries), `cascade_snapshot_ids` (JSON list, batch entries; chainlink #111).

**Per-row hash:** SHA-256 over the canonical JSON of `(sequence, timestamp, actor_did, action, subject_did, subject_uri, subject_cid, rationale, snapshot_id, event_id, previous_hash, cascade_subjects, cascade_snapshot_ids)`. Re-hashing the row content reproduces `current_hash` — the verification primitive.

**Linkage:** `previous_hash` of row N+1 equals `current_hash` of row N. Verification walks the chain checking both per-row hash and prior-row linkage. The chain-level check catches the consistent-rewrite attack where an attacker rewrites both content AND `current_hash` on a prior entry (so per-row verification of that single row passes) but cannot also rewrite the next row's `previous_hash` without invalidating the next row's hash — an attacker would have to rewrite every subsequent row consistently.

**Concurrency.** `append_entry` uses three layers: in-process `tokio::sync::Mutex` ahead of the transaction, `BEGIN`/`COMMIT` transaction wrap, and (on Postgres) `pg_advisory_xact_lock(AUDIT_CHAIN_LOCK_KEY)` as the transaction's first statement. Without these, two concurrent appends both observe the same chain head, both compute the same next-sequence, and the second `INSERT` fails on the `UNIQUE(sequence)` constraint while the underlying mutation has already executed — silent chain entry loss under bursty load. See chainlink #106.

**Pre-Phase-3.8 sentinel rows (defensive-only in v0.2).** `verify_chain_range` skips rows where `current_hash = "pre-chain"` — their linkage is undefined by design and verifying them as if they were real chain entries would produce false negatives. Sentinels still count toward gap detection (the chain must be contiguous). **In v0.2 deployments, no sentinel rows exist.** `migrations/0004_drop_admin_audit_log.sql` dropped the legacy `admin_audit_log` table outright rather than migrating its rows into `audit_chain_entry` as `current_hash="pre-chain"` placeholders (the migration's preamble cites the rationale: v0.2 has not shipped to upstream and internal deployments rebuild from migration scratch). The skip path remains correct and continues to ship as a future-compatibility hook for any later restoration of legacy data, but consumers writing against the chain in v0.2 will not encounter sentinel rows. See [AURORA_ADMIN_UI_DESIGN.md §8.4](AURORA_ADMIN_UI_DESIGN.md) for the wire-format-side note.

#### §4.4.3 Snapshot capture (audit_snapshot)

`audit_snapshot` columns: `id`, `captured_at`, `subject_did`, `subject_uri`, `subject_cid`, `content` (JSON), `content_hash` (SHA-256 over content).

A snapshot captures *what the subject looked like at the moment a decision was taken on it*. Captured before the action lands, so the snapshot reflects pre-decision state. The audit chain entry's `snapshot_id` (or `cascade_snapshot_ids` for batch entries) points at the captured row.

For account subjects, snapshot content includes handle, takedown_ref, deactivated_at, active_action. For record/blob subjects v0.2 captures the URI/CID; richer per-record state is a v0.3 candidate.

**Forensic export linkage.** Bundle integrity is verifiable against the chain forever: re-hash the exported tar bytes and compare to the `X-Aurora-Bundle-Hash` response header at issuance time, or query the chain entry's rationale for the canonical hash record (chainlink #99).

#### §4.4.4 mod_event_seq retention-bounded subscription channel

The live `subscribeModEvents` channel reads from `mod_event_seq`, a separate retention-bounded mirror of the subset of `moderation_event` columns the wire format emits. Rationale: `moderation_event` is the unbounded historical aggregate; `mod_event_seq` is the streaming surface where storage stays bounded by operator-configured retention. Per chainlink #115 / [AURORA_ADMIN_UI_DESIGN.md §3.5](AURORA_ADMIN_UI_DESIGN.md).

**Schema.** `seq` (autoincrementing primary key, independent from `moderation_event.id`), `moderation_event_id` (foreign-key reference for join-back), `actor_did`, `action`, `subject_did`, `subject_uri`, `subject_cid`, `detail` (JSON), `created_at`. The `meta` column is intentionally NOT mirrored — wire format doesn't carry it.

**Dual-write.** Every successful `moderation_event` INSERT writes a `mod_event_seq` row in the same transaction via `insert_moderation_event_in_tx`. Atomicity is by transaction wrapping: both rows land or neither does. Direct `INSERT INTO moderation_event` bypassing the helper does NOT populate `mod_event_seq` — the helper is the canonical write path, pinned by a negative-path test.

**Retention.** Configured via `PDS_MOD_EVENT_RETENTION_DAYS` env var (default 7). A background cleanup job runs every 24 hours and deletes `mod_event_seq` rows older than the retention window. `moderation_event` is not pruned — the historical aggregate retains forever.

**OutdatedCursor.** When a client connects with a cursor older than the oldest retained `mod_event_seq.seq`, the handler emits one `OutdatedCursor { oldestAvailableSeq, message }` frame and closes the WebSocket cleanly with code 1000. The client re-bootstraps via `tools.aurora.moderator.queryEvents` for the missed window and resubscribes with a fresh cursor.

> **Reconciliation (C6).** Earlier Phase 3 design drafts committed to the `mod_event_seq` table; pre-Session-9 implementation lacked it. Session 9 (chainlink #115) brought implementation in line with the docs; the table now exists with the schema, dual-write, retention, and OutdatedCursor semantics described above.

#### §4.4.5 Storage event vocabulary

`ModerationEventType` (12 variants, subject-aware) is the storage representation:

```text
AccountTakedown, AccountSuspend, AccountWarn, AccountRestore,
LabelCreate, LabelRemove,
BlobQuarantine, BlobRestore,
ReportSubmit, ReportReview,
AppealSubmit, AppealReview
```

Logging method: `ModerationEventLogger::log_event` ([src/admin/events.rs:117–167](../src/admin/events.rs#L117)). Phase 3.5's translation layer reads `(ModEvent, Subject)` and writes the appropriate variant.

### §4.5 Sub-phase ordering and dependencies

Per [§2.1](#§21-admin-and-moderation-parity)'s Phase 3 phasing, sub-phases shipped in roughly this order with the dependencies noted:

- **3.1** Lexicon/design pass + per-namespace module organization. Gating for everything below.
- **3.2** `tools.aurora.describeCapabilities`. Foundation; cheap.
- **3.3** Moderator-tier reads (`queryEvents`, `getEvent`, `queryStatuses`, `getSubjectContext`, `getSubjectHistory`).
- **3.4** Moderator-tier appeals reads (`listAppeals`, `getAppeal`).
- **3.5** Admin-tier action surface (`emitEvent` plus the six batch endpoints + `triggerPasswordReset`).
- **3.6** SuperAdmin-tier role management relocation.
- **3.7** Aggregations (`getModerationMetrics`, `getQueueStats`).
- **3.8** Audit chain (`getAuditTrail`) + forensic export (`exportAccountForensic`).
- **3.9** Live event tail (`subscribeModEvents`).
- **3.10** Runtime settings (`getRuntimeSetting`, `setRuntimeSetting`).

After 3.5 lands, 3.7 (aggregations), 3.8 (audit chain), 3.9 (subscription), and 3.10 (runtime settings) are independent. 3.6 (role-mgmt relocation) is independent of every other sub-phase and shipped at any point after 3.1.

### §4.6 Bootstrap path

The first SuperAdmin is granted via direct SQL insertion into the `admin_roles` table against a freshly-created account. After the first SuperAdmin exists, all subsequent role grants flow through `tools.aurora.superadmin.grantRole` and the audit chain. See [README.md "First Admin User" section](../README.md) for the operator-facing procedure (Block 6 / chainlink #95 dropped the `PDS_ADMIN_DIDS` env var auto-grant; bootstrap is now the SQL path).

---

## §5 Postgres backend Phase 4

Phase 4 makes Aurora-Locus deployable as multiple instances against one Postgres backend without sequencer races (only one instance writes the firehose at a time) or stale per-process caches (when one instance mutates state another instance has cached, the cached entry must be invalidated cross-instance).

### §5.1 Selective Postgres for shared databases

Phases 1–3 of [§2.3](#§23-postgres-backend-feasibility)'s plan landed the schema, backend selection, and per-file query-layer compatibility. Phase 4 builds on that to enable horizontal scaling.

The hybrid model holds throughout: shared global state (`account_db`, `did_cache_db`) goes to the configured backend (SQLite for hobbyist, Postgres for production); per-actor `repo.sqlite` always stays SQLite. Phase 4's multi-instance work applies only to the Postgres path; SQLite-backed deployments are inherently single-instance.

### §5.2 Configuration and environment variables

> **Reconciliation (C4).** Earlier drafts referenced `AURORA_SEQUENCER_LEADER_RETRY_MS`. As-built uses `PDS_SEQUENCER_LEADER_RETRY_MS` matching the established `PDS_*` prefix (`PDS_DB_BACKEND`, `PDS_DB_URL`, `PDS_DB_MAX_CONNECTIONS`, `PDS_BLOBSTORE_S3_*`, `PDS_MOD_EVENT_RETENTION_DAYS`, etc.). The consolidated reference uses the as-built names.

Env vars introduced or relevant to Phase 4:

| Var | Default | Purpose |
|---|---|---|
| `PDS_DB_BACKEND` | `sqlite` | `sqlite` or `postgres` |
| `PDS_DB_URL` | (required for Postgres) | Postgres connection string; file path for SQLite |
| `PDS_DB_MAX_CONNECTIONS` | `25` | Pool sizing |
| `PDS_DB_MIN_CONNECTIONS` | `5` | Pool sizing |
| `PDS_DB_ACQUIRE_TIMEOUT_SECS` | `30` | Pool acquire timeout |
| `PDS_DB_IDLE_TIMEOUT_SECS` | (none) | Pool idle timeout |
| `PDS_DB_MAX_LIFETIME_SECS` | (none) | Pool max-lifetime |
| `PDS_SEQUENCER_LEADER_RETRY_MS` | `2000` | Standby retry interval; bounds 500–30000 |

Channel names, lock keys, and payload schemas are hardcoded.

### §5.3 Connection model

Each Aurora-Locus instance uses **`pool_size + 2`** connections against Postgres:

- The application `AnyPool` (default 25 connections; `PDS_DB_MAX_CONNECTIONS`).
- One dedicated connection for the sequencer leader-election advisory lock. The lock is held for the lifetime of a leader, so it's a long-idle connection. The dedicated connection is opened directly via `AnyConnection::connect` rather than borrowed from the pool — borrowing would invisibly steal one application slot for the leader's lifetime (chainlink #103).
- One dedicated connection for the LISTEN listener (long-idle).

Operators sizing managed-Postgres connection limits should account for `(pool_size + 2) × instance_count`.

### §5.4 Multi-instance support

#### §5.4.1 Sequencer leader election

On startup, the sequencer attempts to acquire a single int64-keyed session-level advisory lock via `pg_try_advisory_lock(SEQUENCER_LEADER_LOCK_KEY)`. The lock holder is the *leader*; non-holders are *standbys*. Only the leader writes new firehose events.

Postgres advisory locks are session-scoped: when the holding connection terminates (graceful close, network drop, server-side timeout), the lock is released automatically. This gives free failure detection — no application-level heartbeat needed for v0.2.

Standbys still serve read traffic and accept writes that don't generate firehose events; they just don't advance the `repo_seq` table. A writer that gets a request requiring firehose emission while the local instance is a standby returns 503 Service Unavailable; operators run a load balancer in front of multiple instances; 503 retries land on the leader on the next attempt.

**Failure modes:**

- **Connection drop** → lock auto-releases; the dropping instance is no longer leader. Standbys pick up on their next 2s retry tick. The drop side reconnects and enters the standby retry loop.
- **Slow but alive leader** → not detected by Postgres alone. v0.2 accepts this as a known limitation: a leader that's stuck (deadlocked, GC pause, etc.) but still holding its TCP connection blocks standbys indefinitely. Application-level heartbeat is a future consideration.
- **Network partition** → Postgres terminates the leader's connection via TCP keepalive eventually. Operators tune Postgres for the partition-detection latency they want.
- **Postgres restart** → all advisory locks released. All standbys race to acquire on next retry tick. One wins.

**Lock key derivation.** The advisory lock key is the first 8 bytes of `SHA-256("aurora-locus.sequencer.leader")` interpreted as a signed int64. Hashing a human-readable identifier avoids collisions with other applications using advisory locks on the same Postgres database, survives schema-namespace changes, and is reproducible. Hardcoded constant in code; not configurable. A separate key (`SHA-256("aurora.audit_chain")`) covers the audit-chain advisory lock from [§4.4.2](#§442-audit_chain_entry-chain-semantics); the two keyspaces don't collide (verified by a runtime test).

#### §5.4.2 Cache invalidation via LISTEN/NOTIFY

Postgres LISTEN/NOTIFY: the writing instance issues `NOTIFY aurora_cache_invalidate, '<payload>'` after the modifying transaction commits; listening instances asynchronously receive the payload on a long-lived LISTEN connection and invalidate matching local cache entries.

**Single channel** (`aurora_cache_invalidate`) with a payload schema `{"type": "<cache-type>", "key": "<key>"}`. Payload-based dispatch is cheap (JSON parse + match on `type`); receivers ignore `type` values they don't recognize, so old code coexists with new senders.

**v0.2 only NOTIFYs one cache type:** `local_records:<did>` invalidates per-DID entries in `LocalRecordsCache`. The audit ([§5.4.3](#§543-cache-types-requiring-invalidation)) found six in-process state-holding pieces; only `LocalRecordsCache` was both purely in-memory and required cross-instance NOTIFY. The other five either store in Postgres directly (no per-process layer to invalidate), or have semantics that don't require cross-instance consistency, or are out of scope for v0.2 multi-instance work.

**Write-site instrumentation.** `NOTIFY` calls happen *after* the modifying SQL transaction commits. Two reasons: (a) avoid invalidating before new data is visible (B re-reading before A commits would re-cache stale data); (b) NOTIFY is buffered until commit in Postgres so it doesn't fire on rollback — emitting via a separate connection would lose this guarantee. Concrete write sites that mutate per-DID repo state emit one NOTIFY per affected DID after the commit.

**Listener — dedicated connection.** Each process opens one dedicated long-lived Postgres connection, issues `LISTEN aurora_cache_invalidate`, and processes notifications in a Tokio task that loops on `connection.notifications().recv()`. The connection is *not* drawn from the main `AnyPool` because pool connections cycle and `LISTEN` on a connection returned to the pool stops delivering notifications.

**Connection drop recovery.** During disconnect, no NOTIFYs are received; caches may serve stale data for the duration of the disconnect. Recovery is automatic via the listener reconnect loop (six-step exponential backoff: 1s, 2s, 4s, 8s, 16s, 30s, then capped at 30s). Notifications emitted during the disconnected window are lost; the TTL fallback in each invalidatable cache (LocalRecordsCache: 5 seconds) covers this case.

#### §5.4.3 Cache types requiring invalidation

Audit ([§2.3](#§23-postgres-backend-feasibility) Phase 4.1) inventoried six cache-shaped pieces of state across `src/`:

| Cache | Storage | Cross-instance NOTIFY? |
|---|---|---|
| `LocalRecordsCache` | In-memory Moka, 5s TTL | **Yes** |
| `DidCache` | Postgres `did_doc` / `did_handle` tables | No — Postgres is the SoT, no per-process layer |
| `RateLimiter` request counts | In-memory `governor` state | Out of scope — distributed rate limiting is a separate workstream |
| `OAuthStateStore` | In-memory HashMap | Out of scope — explicitly noted as single-instance limitation |
| `DPopNonceStore` | In-memory HashMap, 5min TTL | Out of scope — DPoP integration not enabled in scope when audit ran |
| `NonceStore`, `PdsDiscovery`, etc. | Per-instance in-memory | No — semantics don't require cross-instance consistency |

So the Phase 4 implementation scope reduced to: one cache type (`local_records`), NOTIFY emitted at the existing `invalidate_did` call sites in repo write handlers, single LISTEN connection per process.

### §5.5 Operator considerations

**Failover characteristics:**

- **Leader-process termination** (kill, OOM, graceful shutdown): standbys reacquire within 2s (`PDS_SEQUENCER_LEADER_RETRY_MS`). Firehose has up to 2s of write-side silence; events queued in Postgres `repo_seq` are not lost — the new leader picks up where the old left off.
- **Postgres restart**: all instances reconnect; one wins the lock race. Connection-establishment latency before firehose resumes.
- **Network partition** between an instance and Postgres: the isolated instance's TCP connection eventually times out; during the timeout window the isolated instance still thinks it's leader and may attempt writes that fail at the network layer. Operators tune `tcp_keepalives_*` for desired partition-detection latency.

**Backup tooling.** `aurora-locus backup --postgres` and `aurora-locus restore --postgres` wrap `pg_dump`/`pg_restore` for consistent snapshots. WAL archiving for point-in-time recovery is operator-side concern (postgresql.conf settings, archive command); the operator guides under [docs/operator/](operator/) document the recommended setup.

**Capacity planning.** `(pool_size + 2) × instance_count` connection slots required against the Postgres backend. Default `pool_size = 25` is appropriate for typical web-app workloads; operators with high-traffic deployments tune up via `PDS_DB_MAX_CONNECTIONS`.

### §5.6 Out of scope for v0.2

**Distributed rate limiting.** The `distributed_rate_limiter` hook in `RateLimiter` is unwired. Wiring it would require either a Redis backend or a Postgres-CAS-based token bucket. Both are larger than fits in v0.2. The current per-instance limit is a known softness: a malicious caller distributing requests across multiple instances can exceed the intended global rate. Acceptable for the v0.2 deployment profile (one or two instances behind a load balancer, not adversarial-grade rate enforcement).

**OAuth state and DPoP nonces multi-instance.** Both flagged as known per-process limitations. Solving multi-instance for them needs a different mechanism (Redis or a SQL-backed transient store) and a separate design discussion.

**Caches with TTL > 5s on the NOTIFY path.** The TTL-fallback story costs at most one TTL window of staleness on a missed NOTIFY. For LocalRecordsCache (5s TTL), that's 5s. Future caches with longer TTLs need either accepted longer-staleness windows on listener-disconnect, or a cache-version-vector exchange after listener reconnect, or a move to Postgres-backed (like DidCache).

**Postgres-level replication, consensus protocols, cross-region coordination.** Operator-side concerns; v0.2 assumes a single Postgres endpoint (or a managed cluster fronted as a single endpoint).

**Internal forwarding of firehose-write requests** to the leader. The 503-and-retry path is what ships; internal forwarding is a future consideration if operators report it as an issue.

**Heartbeat for stalled-but-alive leader.** Decision deferred to operator feedback. If the case is hit in practice, it gets a future chainlink.

---

## §6 Proto-blue migration record

Workstream A replaced the bundled 36,809-line `Rust-Atproto-SDK` directory with the modular `proto-blue 0.2.6` crate. Migration was bounded: 12 source files imported from `atproto::`; 8 distinct SDK surfaces touched; 34 total `atproto::` references across the codebase. Most of Aurora-Locus's 47K lines is PDS-specific server logic that doesn't touch the SDK at all.

### §6.1 What was migrated

| `atproto::` surface | Used in | `proto_blue::` equivalent | Migration shape |
|---|---|---|---|
| `did_doc::DidDocument` (+ helper types) | 4 files | `proto_blue::common` | Direct import swap |
| `handle::HandleResolver` | 1 file | `proto_blue::identity` | Direct import swap |
| `tid::Tid` | 1 file | `proto_blue::syntax::Tid` + `proto_blue::common::next_tid()` | API shape change (method → free function) |
| `types::Did` | 1 file | `proto_blue::syntax::Did` | Direct import swap |
| `repo::Repository`, `repo::RepoError` | 4 files | `proto_blue::repo::Repo`, `proto_blue::repo::RepoError` | API shape change (storage/signer injection) |
| `car::CarWriter`/`Reader`/`Error` | 1 file | `proto_blue::repo::{blocks_to_car, read_car}` (functions) | API shape change (struct → function) |
| `oauth::OAuthClient`, `oauth::PkceParams` | 1 file | `proto_blue::oauth` | Direct import swap |
| `blob::validate_blob_size`, `detect_mime_type_from_data` | 1 file | **Not in proto-blue.** Inlined to `src/blob_store/mime.rs`. | Local extraction |
| `server_auth::PasswordHasher` | 2 files | **Not in proto-blue.** Extracted to `src/auth/password.rs`. | Local extraction |

### §6.2 Structural changes

**`Repository` → `Repo` with storage/signer injection.** The bundled SDK's `Repository::create(did)` was in-memory-only; `proto_blue::repo::Repo::create(storage, did, signer)` requires injected storage and signer traits. Aurora-Locus implemented `RepoStorage` for `ActorStore` (the SQLite-backed per-actor storage) at `src/actor_store/repo_storage.rs`; `Repo` is constructed with `Arc::new(actor_store)` as storage. This is *better* architecture than the bundled SDK's in-memory model — separating repo logic from storage is a clean abstraction win — but it touched the construction flow as well as imports.

**CAR API: struct → function.** The bundled SDK's `CarWriter`/`CarReader` builder/parser pattern became `blocks_to_car(root, blocks)` and `read_car(data)` functions. `actor_store/car.rs` was rewritten against the function-based API.

**`PasswordHasher` and blob utilities extracted locally.** Both are server-side concerns proto-blue rightly doesn't include in a client SDK. Lifted to local modules (`src/auth/password.rs`, `src/blob_store/mime.rs`). The `argon2` crate was added to `Cargo.toml` (previously transitive through the bundled SDK).

### §6.3 What remains

Migration complete. The bundled `Rust-Atproto-SDK/` directory was removed; the `atproto = { path = "./Rust-Atproto-SDK" }` dependency was dropped from `Cargo.toml`; `cargo check --all-features` shows no remaining `atproto::` references.

Concurrent with the migration: `jsonwebtoken 9 → 10` dependency bump, MSRV bump to 1.85 (proto-blue's required edition is 2024). `axum` stays at `0.7` — the cycle's earlier draft proposed bumping to `0.8` but the upgrade was deferred when the proto-blue work was sufficient on its own.

Full per-file translation detail is in commit history (`#1` through `#14` in chainlink, plus the proto-blue migration baseline at `c2d6fd2`). The original per-file mapping doc was absorbed into this consolidated reference; the cycle's mechanical work doesn't need the line-by-line record preserved.

---

## §7 SQLite/Postgres coupling work

The cycle's audit found 22 files importing `SqlitePool` or `sqlx::sqlite::*`, classified by which database surface they touched. The architectural decision shaping the migration was the three-database split documented in [§2.3](#§23-postgres-backend-feasibility): global state on configurable backend, DID cache on configurable backend, per-actor repos always on SQLite.

### §7.1 Coupling audit findings

| Group | Surface | Files | Action |
|---|---|---|---|
| A | `account_db` consumers | 15 | Refactor `db: SqlitePool` → `db: AnyPool` |
| B | `did_cache_db` consumer | 1 | Refactor `db: SqlitePool` → `db: AnyPool` |
| C | Per-actor SQLite consumers | 2 | No change; per-actor stays SQLite |
| D | Infrastructure (db/mod.rs, context.rs, cli/health.rs, identity/resolver.rs tests) | 4 | Refactor `db/mod.rs` and `context.rs`; the rest follow |

Group A files: `account/manager.rs`, `oauth/{client,token_rotation,device}.rs`, `blob_store/{store,quarantine}.rs`, `admin/{reports,appeals,moderation,events,invites,roles,labels}.rs`, `sequencer/sequencer.rs`, `mailer/tracking.rs`. Group B file: `identity/cache.rs`. Group C files: `actor_store/{store,transaction}.rs` (`actor_store/repo_storage.rs` sits beneath these and routes through the same per-actor SQLite pool — no changes needed).

### §7.2 AnyPool migration approach

Three design options were considered for the abstraction layer; the cycle adopted **Approach 4 — Hybrid**:

- **Approach 1: Generic over `sqlx::Database`** — rejected. sqlx's compile-time query checking (`query!`/`query_as!` macros) doesn't play well with truly-generic database types; different drivers have different parameter binding syntax (`?N` vs `$N`).
- **Approach 2: `enum DbPool { Sqlite(SqlitePool), Postgres(PgPool) }`** — rejected. ~80 queries × 2 dispatches = ~160 implementations to keep in sync.
- **Approach 3: `sqlx::Any` driver baseline** — accepted. Most queries are simple enough that `Any` handles them.
- **Approach 4: Hybrid (Approach 3 baseline + per-query escapes via Approach 2)** — adopted. Use `Any` for the ~95% of code paths where it works; provide driver-specific implementations only where features diverge (advisory locks, `RETURNING`, `JSONB`, `LISTEN/NOTIFY`).

**Concretely.** Each of the 16 manager structs in Groups A and B holds `db: AnyPool`. Most queries use the unified `?N` parameter syntax (sqlx internally rewrites for Postgres). Backend-specific paths exist only where genuinely needed — chainlink #89 (sequencer leader election via `pg_advisory_xact_lock`), chainlink #90 (LISTEN/NOTIFY), chainlink #106 (audit-chain serialization with optional Postgres advisory lock).

### §7.3 Schema translations

The original SQLite schema at `migrations/0001_initial.sql` (479 lines) was translated to a Postgres counterpart at `migrations/postgres/0001_initial.sql`. Per-construct translation rules:

| SQLite | Postgres |
|---|---|
| `INTEGER PRIMARY KEY AUTOINCREMENT` | `BIGSERIAL PRIMARY KEY` |
| `BOOLEAN` (stored as INTEGER 0/1) | `BOOLEAN` |
| `DATETIME` | `TIMESTAMPTZ` |
| `BLOB` | `BYTEA` |
| `DEFAULT 0` (boolean) / `DEFAULT 1` (boolean) | `DEFAULT false` / `DEFAULT true` |
| `PRAGMA *` | (no equivalent; configured via `postgresql.conf`) |

**Boolean column literals in queries.** `SELECT ... WHERE invitesDisabled = 0` was rewritten to `= false` across the audited files; SQLite tolerates the latter on INTEGER columns since SQLite 3.23.0, and Postgres requires it on `BOOLEAN`. Centralized helper `crate::db::read_bool` covers cross-backend boolean column reads.

**Postgres-specific features deliberately not used in v0.2 schema:**

- `JSONB` columns — `detail` fields stay as TEXT-with-JSON-content. JSONB is a v0.3 candidate where querying-inside-JSON pays off.
- `tsvector` + GIN indexes — out of scope; revisit if/when search workloads demand it.
- Partitioning — out of scope; revisit at scale.
- `LISTEN/NOTIFY` — used as a runtime primitive (see [§5.4.2](#§542-cache-invalidation-via-listennotify)) but not as a schema feature.

Per-file migration record is preserved in commit history (chainlink sub-phases 5.0.1 through 5.0.6 plus Phase 5.1 standing CI). The cycle's per-file mechanical work doesn't need the line-by-line record preserved here.

---

## §8 Cycle delivery summary

### §8.1 Phases delivered

The v0.2 cycle delivered the following phases against the upstream baseline `c2d6fd2`. Each line corresponds to substantive design and implementation work; cross-references point at the consolidated section that documents the shipped form.

**Workstream A — Proto-blue migration** ([§6](#§6-proto-blue-migration-record)). 12 source files migrated from the bundled `Rust-Atproto-SDK` to `proto-blue 0.2.6`. Two surfaces (`PasswordHasher`, blob utilities) extracted locally. `Repository` refactored against proto-blue's storage/signer injection model.

**Workstream B — Postgres backend** ([§5](#§5-postgres-backend-phase-4), [§7](#§7-sqlitepostgres-coupling-work)). 16 files refactored from `SqlitePool` to `AnyPool`. Postgres schema translated. Multi-instance support via leader election + LISTEN/NOTIFY. Backup/restore wrappers + WAL archiving operator guides. CI runs against both SQLite and Postgres.

**Admin/moderation Phase 1 (parity)** ([§3](#§3-lexicon-shape-audit)). Five parity-gap endpoints shipped, plus per-endpoint cleanup driven by the cycle-opening lexicon-shape audit. Of the eleven endpoints audited, ten now ship clean and one (`getSubjectStatus`) is mostly clean with a single Aurora extension (`suspended`) that is omitted from the wire when not populated. The cycle closed every wire-breaking drift the audit identified; the only residual deviation is the `suspended` extension, scoped as a v0.3 candidate to either relocate to `tools.aurora.*` or fold into `takedown` semantics.

**Admin/moderation Phase 2 (namespace cleanup)** ([§4.3.4](#§434-toolsauroraops)). ~30 operator-flavored endpoints relocated from `com.atproto.admin.*` to `tools.aurora.ops.*`. `com.atproto.admin.*` is now the slim parity surface bsky-PDS exposes; operator extensions are visible and well-organized under their own namespace.

**Admin/moderation Phase 3 (Aurora extensions)** ([§4](#§4-admin-and-moderation-phase-3)).
- 3.1 Lexicon design + module organization.
- 3.2 `tools.aurora.describeCapabilities`.
- 3.3 Moderator-tier reads (`queryEvents`, `getEvent`, `queryStatuses`, `getSubjectContext`, `getSubjectHistory`).
- 3.4 Moderator-tier appeals reads (`listAppeals`, `getAppeal`).
- 3.5 Admin-tier action surface (`emitEvent` + 6 batch endpoints + `triggerPasswordReset`).
- 3.6 SuperAdmin-tier role management relocation.
- 3.7 Aggregations (`getModerationMetrics`, `getQueueStats`).
- 3.8 Audit chain (`getAuditTrail`) + forensic export (`exportAccountForensic`, metadata-only in v0.2).
- 3.9 Live event tail (`subscribeModEvents` with `OutdatedCursor` support).
- 3.10 Runtime settings (`getRuntimeSetting`, `setRuntimeSetting`).

**Cycle audit work.** Two rounds of round-trip auditing surfaced 15 chainlinks worth of fixes: integrity issues (audit chain transitive verification, forensic export bundle hash, audit chain concurrent-writer serialization), security issues (DPoP enforcement end-to-end, admin router XSS, `PDS_ADMIN_DIDS` shadow-grant elimination, `/admin/debug.html` production gating, admin UI `adminRefreshToken` storage removal), wire-format alignments (subscribeModEvents AuditEntry variant, scope tightening for operator-flavored endpoints, batch ops failures field, batch ops per-subject snapshot capture, emitEvent SendEmail role gate to Admin+), and the audit chain coverage migration (every administrative call site routed through `audit_chain::append_entry`; `admin_audit_log` table dropped). Plus the documentation refactor that produced this consolidated reference.

### §8.2 Deferred to v0.3

- **S3 blob storage activation.** Initially deferred per the v0.2 cycle plan; ultimately shipped in v0.2 alongside the assessment's three-phase activation work. AWS SDK dependencies and `src/blob_store/s3.rs` exports are live; backend selection runs via `BlobstoreConfig` from the `PDS_BLOBSTORE_*` env vars. See [§2.2](#§22-blob-storage-s3-feasibility) for the post-cycle status.
- **Forensic export full-content inclusion.** v0.2 ships metadata-only ([§4.3.2](#§432-toolsauroraadmin)); CAR data + blob bytes deferred. Streaming response body for large bundles also deferred.
- **Forensic export streaming.** In-memory tar assembly today; streaming response body for large bundles deferred.
- **Per-record/per-blob status tracking.** [`tools.aurora.moderator.queryStatuses`](#§431-toolsauroramoderator) accepts `subject_type=Record|Blob` for wire-format stability but short-circuits to empty pages because `subject_status` only tracks repo-level state today.
- **Batch ops end-to-end per-subject atomicity** (chainlink #113). v0.2 shipped two-tier atomicity (chain-entry-atomic, per-subject best-effort); resolved in v0.3 Arc 4 §8.4.2 — every `batch*` handler (and `emitEvent` after Arc 4's multi-subject reshape) is now whole-tx-atomic via `_in_tx` manager variants. Per-subject mutation failure aborts the wrapping tx atomically with the chain entry; the v0.2 `failures` response field is retired.
- **Distributed rate limiting.** [§5.6](#§56-out-of-scope-for-v02) — Redis or Postgres-CAS token bucket required.
- **OAuth state and DPoP nonces multi-instance.** Per-process limitations preserved.
- **Hover tooltips, signed URLs, Aurora-driven CDN purge on takedown, live SQLite→Postgres migration tooling.** All scoped out by their respective assessments.
- **Schema redesign for Postgres-native features** (JSONB, tsvector, partitioning). v0.2 keeps schema 1:1 between backends; future cycles introduce per-table where it pays off.
- **`tools.aurora.admin.invite-lineage-v1` and `reporter-context-v1`** capabilities. Endpoints not yet shipped; the §8.15 capability advertisement omits them until handlers land.

### §8.3 Out of scope by design

These were assessed and deferred indefinitely (not v0.3 candidates by themselves):

- **Per-actor stores on Postgres.** The hybrid model (global state on configurable backend, per-actor state always SQLite) is the architecture, not a transitional state.
- **Wholesale ORM replacement.** Staying with sqlx; not switching to Diesel, SeaORM, or any other ORM.
- **Sharding across multiple Postgres instances.** Aurora-Locus operates against a single Postgres backend; sharding is a separate architectural conversation if scale demands it.
- **Multi-region/multi-bucket S3 configurations.**
- **Blob storage backends beyond Disk and S3.**
- **Wholesale UI framework rewrite.** The admin UI extends the existing multi-page SPA pattern.

---

*End of AURORA_DESIGN.md*
