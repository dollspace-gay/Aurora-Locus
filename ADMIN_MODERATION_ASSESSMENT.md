# Aurora-Locus Admin & Moderation — Assessment

**Surface:** `com.atproto.admin.*` (parity floor) + `tools.aurora.{moderator,admin,superadmin,ops}.*` (Rust extensions, role-tiered)
**Status:** Assessment — parity gaps and extension surface identified
**Reference target:** bsky-PDS 2025-Q1 (matches doll's other parity assessments)
**Depends on:** Existing `src/admin/` infrastructure, OAuth scopes, `Role` enum, `ModerationEventType` enum
**Date:** 2026-04-30

---

## 1. Where Aurora-Locus stands today

Aurora-Locus already ships a substantial admin and moderation surface. The infrastructure is more developed than this doc's title might suggest — the gap is not "build moderation from scratch" but "bring the existing surface to bsky-PDS-2025-Q1 parity and extend it with capabilities the architecture affords."

### 1.1 Existing role and authentication infrastructure

The auth model is already in place and consistent with the rest of Aurora-Locus:

- **Three-tier role hierarchy** in [src/admin/roles.rs](src/admin/roles.rs): `Moderator` (view-only), `Admin` (most actions), `SuperAdmin` (full access including role grants). The `Role` enum implements `PartialOrd` for hierarchical comparisons and exposes `can_act_as(required: Role)` for permission checks.
- **OAuth 2.1 scope hierarchy** in [src/oauth/scope.rs](src/oauth/scope.rs): admin operations gate on `atproto:admin.*` (full), `atproto:admin.moderation`, or `atproto:admin.server`. The scope check happens at route-level middleware before any handler logic runs.
- **Bootstrap path** documented in the README: a SuperAdmin is granted via direct SQL insertion into `admin_role` against a freshly created account, then subsequent role grants flow through the admin endpoints.
- **Audit log structure** in [src/admin/mod.rs](src/admin/mod.rs): `AuditLogEntry` records `admin_did`, `action`, `subject_did`, `details`, `timestamp`, `ip_address` — the substrate for any moderation event tracking.

This infrastructure shapes the rest of the doc: parity gaps and extensions both build on what's already here, rather than introducing parallel auth machinery.

### 1.2 Existing admin endpoints

Per the README and [src/admin/](src/admin/) module structure, the current XRPC surface under `com.atproto.admin.*` covers:

- **Role management:** `grantRole`, `revokeRole`, `listRoles`
- **Account moderation:** `takedownAccount`, `suspendAccount`, `restoreAccount`, `getAccount`, `listAccounts`, `getAccountInfos`, `deleteAccount`
- **Account modification:** `updateAccountEmail`, `updateAccountHandle`, `updateAccountPassword`
- **Subject status:** `getSubjectStatus`, `updateSubjectStatus`
- **Labels:** `applyLabel`, `removeLabel`
- **Reports:** `submitReport`, `updateReportStatus`, `listReports`
- **Invites:** `createInviteCode`, `getInviteCodes`, `disableInviteCode`, `enableAccountInvites`, `disableAccountInvites`
- **Audit:** `getAuditLog`
- **Server info:** `getStats`, `sendEmail`

Plus an additional ~35 operator-facing endpoints in the same namespace covering blob ops, sequencer ops, federation status, rate limit ops, and health metrics. Those are addressed in §6.

### 1.3 Existing moderation primitives

The semantic vocabulary for moderation actions and events is already defined in code:

- [src/admin/moderation.rs](src/admin/moderation.rs) — `ModerationAction` enum: `Takedown`, `Suspend`, `Flag`, `Warn`, `Restore`. Each carries reversal tracking, expiry, report linkage, and reviewer notes via `ModerationRecord`.
- [src/admin/events.rs](src/admin/events.rs) — `ModerationEventType` enum with 12 variants covering account/label/blob/report/appeal lifecycle: `AccountTakedown`, `AccountSuspend`, `AccountWarn`, `AccountRestore`, `LabelCreate`, `LabelRemove`, `BlobQuarantine`, `BlobRestore`, `ReportSubmit`, `ReportReview`, `AppealSubmit`, `AppealReview`.
- [src/admin/appeals.rs](src/admin/appeals.rs) — appeal lifecycle and review workflow, with `AppealSubmit` / `AppealReview` event integration.
- [src/admin/labels.rs](src/admin/labels.rs) — label application and removal with the existing event log integration.

The vocabulary is sufficient for current admin operations. Where it falls short for parity or extension purposes, the gaps are explicit in §3 and §5.

---

## 2. The bsky-PDS 2025-Q1 floor

This doc treats bsky-PDS as the reference implementation and the target Aurora-Locus must match. Aurora-Locus is positioned in the README as a "Bluesky-network-compatible PDS"; for that positioning to hold, Aurora-Locus cannot be missing features that bsky-PDS exposes. Anyone evaluating Aurora-Locus as an alternative to bsky-PDS must find no functional cons against the migration.

The 2025-Q1 reference target is consistent with how doll's `ACCOUNT_MANAGEMENT_PARITY.md` dates the comparison. It captures bsky-PDS at a point before the moderation-namespace migration applied to bsky-PDS itself in late 2025 — the version of bsky-PDS that exposes the full pre-migration `com.atproto.admin.*` surface, including the moderation queue, label application, and report management endpoints that bsky-PDS later relocated to `tools.ozone.*`.

This dating choice has a consequence: parity work targets the pre-migration `com.atproto.admin.*` surface. Aurora-Locus does not need to track the post-migration shift unless and until Aurora-Locus chooses to align with the newer separation-of-concerns architecture. That choice is out of scope for this assessment; today the parity question is whether Aurora-Locus matches what 2025-Q1 bsky-PDS exposed.

The 2025-Q1 reference covers approximately these endpoint families under `com.atproto.admin.*`:

- Account lifecycle and modification (creation handled by `com.atproto.server.*`, but admin-side viewing/management lives here)
- Subject status with polymorphic subject types (account / record / blob)
- Invite code management
- Moderation actions (takedown/suspend/restore/label/report)
- Communication (sendEmail)
- Account modification (email, handle, password, signing key)

What follows in §3 enumerates where Aurora-Locus's existing surface meets that floor, where it falls short, and what shipping each gap requires.

---

## 3. Parity gaps to close (must ship)

Each gap below is a feature bsky-PDS 2025-Q1 exposes that Aurora-Locus does not. Closing these is non-negotiable for Aurora-Locus to be a credible bsky-PDS alternative. They are listed in rough order of user-visible impact, not effort.

### 3.1 `disableInviteCodes` (plural)

Aurora-Locus has `disableInviteCode` (singular). The bsky-PDS lexicon defines `disableInviteCodes` (plural) accepting optional `codes: string[]` and `accounts: string[]` arrays — it disables specified codes and/or all codes for specified accounts in a single call.

The plural form is what bsky-PDS clients call. A moderator dealing with a spam ring needs to disable many codes at once; the singular form forces N round-trips and is the wrong primitive for that workflow.

**What shipping requires:**
- New lexicon at `com.atproto.admin.disableInviteCodes` matching the bsky-PDS shape
- New XRPC handler that wraps the existing single-code disable logic in a batch loop within a database transaction
- Retain `disableInviteCode` (singular) as a deprecated alias for back-compat — remove in a later minor version

### 3.2 `getAccountInfo` (singular)

Aurora-Locus has `getAccount` (pre-modern naming) and `getAccountInfos` (plural). The bsky-PDS canonical endpoint is `getAccountInfo` (singular) — the same shape as `getAccount` but with the modern name that current bsky-PDS clients call.

**What shipping requires:**
- New lexicon at `com.atproto.admin.getAccountInfo`
- New XRPC handler that's a thin wrapper over the same logic as `getAccountInfos` with a single-DID input
- Retain `getAccount` as a deprecated alias

### 3.3 `searchAccounts`

The bsky-PDS lexicon defines `searchAccounts` with optional `email`, `cursor`, `limit` parameters returning paginated `accountView[]`. Aurora-Locus has `listAccounts` which is broader (lists all accounts with filter options) but does not match the search-by-email shape that bsky-PDS clients call.

A note on lexicon conformance: the `searchAccounts` lexicon is published in `bluesky-social/atproto` but bsky-PDS does not yet wire up the handler. Aurora-Locus implementing it would be lexicon-conformant ahead of bsky-PDS itself — fine for parity purposes, since the lexicon is the contract.

**What shipping requires:**
- New lexicon at `com.atproto.admin.searchAccounts`
- New XRPC handler implementing email search (the broader filtering capability `listAccounts` provides should be retained or relocated, addressed in §6)

### 3.4 `updateAccountSigningKey`

The bsky-PDS lexicon defines `updateAccountSigningKey` with `did` and `signingKey` parameters, triggering a PLC operation to rotate the account's signing key in the DID document. Aurora-Locus has the underlying PLC rotation logic in [src/cli/rotate_keys.rs](src/cli/rotate_keys.rs) and [src/crypto/plc_client.rs](src/crypto/plc_client.rs), but no XRPC endpoint exposing the operation.

Like `searchAccounts`, this lexicon is published but not wired in bsky-PDS yet. Implementing it puts Aurora-Locus ahead on conformance.

**What shipping requires:**
- New lexicon at `com.atproto.admin.updateAccountSigningKey`
- New XRPC handler that calls into the existing PLC rotation machinery
- No new logic — this is wiring existing functionality to a new endpoint

### 3.5 Polymorphic subject in `updateSubjectStatus`

Modern bsky-PDS `updateSubjectStatus` accepts a polymorphic `subject` (one of `repoRef` / `strongRef` / `repoBlobRef`) and structured `takedown` / `deactivated` input objects. This consolidates what older bsky-PDS exposed as separate verbs (`takedownAccount`, `suspendAccount`, `restoreAccount`).

Aurora-Locus has `updateSubjectStatus` *and* the legacy split-verb endpoints. The split-verb endpoints work for accounts but don't support the polymorphic subject — `updateSubjectStatus` may or may not, depending on the current implementation's actual handling of `repoRef` vs `strongRef` vs `repoBlobRef` inputs. This needs verification.

**What shipping requires:**
- Audit Aurora-Locus's existing `updateSubjectStatus` against the modern lexicon — confirm it accepts all three subject types and the structured `takedown`/`deactivated` input objects
- If it does, mark the legacy `takedownAccount`/`suspendAccount`/`restoreAccount` as deprecated (kept for back-compat, removed in a later minor version)
- If it doesn't, extend it to accept the polymorphic shape, then deprecate the split-verb endpoints
- Either way, internal logic in `src/account/manager.rs` is preserved — only the XRPC handler shape changes

### 3.6 Verification of parity-clean endpoints

Eleven endpoints appear at parity by name, but the lexicon shape (request fields, response fields, error codes) needs verification: `deleteAccount`, `disableAccountInvites`, `enableAccountInvites`, `getAccountInfos`, `getInviteCodes`, `getSubjectStatus`, `sendEmail`, `updateAccountEmail`, `updateAccountHandle`, `updateAccountPassword`, `updateSubjectStatus`.

A surface-level "✅ matches" is provisional until the actual lexicons are diffed against bsky-PDS-2025-Q1's published lexicons. Drift in property names, optional fields, or error codes would each be small corrections, but the audit needs to happen before parity can be confidently asserted.

**What shipping requires:**
- Per-endpoint lexicon diff against `bluesky-social/atproto` lexicons at the 2025-Q1 reference point
- File issues for any drift; fix per-endpoint
- This is verification work, not new feature work, but it must happen before parity claims are credible

---

## 4. The Rust opportunity

Aurora-Locus is written in Rust against axum, sqlx, and a custom-fit per-actor storage architecture (per-DID SQLite repos with the proto-blue SDK as the ATProto substrate). This architecture affords admin and moderation capabilities that bsky-PDS's TypeScript implementation does not expose — not because TypeScript can't do them, but because doing them well requires infrastructure choices Aurora-Locus has and bsky-PDS does not.

Five Aurora-Locus characteristics enable richer admin capabilities:

**The sequencer is a Rust + axum + WebSocket primitive that already streams the firehose.** Extending it to a moderation-event channel is incremental work, not new infrastructure. Real-time push of moderation events to admin tools and dashboards becomes affordable.

**Postgres backend (Workstream B in progress) supports native batch transactions across instance-level state.** Multi-subject moderation operations — takedowns of an entire spam ring in one call, batch label applications, batch invite disablement — commit atomically with one audit-trail entry. The single-writer architecture (one Aurora-Locus binary against one Postgres backend) makes atomicity straightforward.

**The existing `AuditLogEntry` and `ModerationEventType` substrate is already richer than what bsky-PDS exposes.** Aurora-Locus tracks IP addresses, structured details, reversal lineage, and per-event categorization at a granularity that supports auditable moderation workflows out of the box. Surfacing this richness to admin clients via additional XRPC endpoints is mostly lexicon and handler work, not new domain logic.

**Type safety and exhaustive enum matching.** Aurora-Locus's `ModerationEventType` and `ModerationAction` enums force exhaustive match patterns at compile time — adding a new event variant breaks every handler that doesn't account for it, surfacing the gap as a build failure rather than a runtime omission. bsky-PDS's TypeScript equivalent relies on string discriminants and runtime checks; the cost of safely extending the event vocabulary is higher. For an admin/moderation surface where the event vocabulary will grow (escalations, scheduled actions, custom Aurora-specific events), Aurora-Locus's type system makes growth safer.

**Per-actor isolation through the existing actor_store architecture.** Each user's repository lives in its own SQLite file managed by a per-DID `ActorStore` (now backed by proto-blue's `RepoStorage` trait via the `SqliteRepoStorage` bridge in [src/actor_store/repo_storage.rs](src/actor_store/repo_storage.rs)). Operations on one user's repo never lock another user's repo. For admin operations that touch multiple subjects (batch takedowns, label sweeps), this isolation means the operations don't serialize through a shared lock — each subject's state change happens in its own transaction. bsky-PDS shares a single SQLite database across all users, so cross-user batch operations contend on the same lock.

These five characteristics shape the extension surface in §5. The extensions are not parity work; they are PDS-side capabilities Aurora-Locus offers because the architecture supports them. Operators choosing Aurora-Locus over bsky-PDS get them as differentiators; what tooling they build against them (admin web UIs, external moderation tools, compliance dashboards) is operator-determined.

### 4.1 Namespace structure for Aurora extensions

The Aurora-specific extensions live under `tools.aurora.*` rather than extending `com.atproto.admin.*` non-standardly. Within `tools.aurora.*`, the extensions are organized into four families, each aligned to a real auth boundary in Aurora-Locus's existing code:

| Namespace | Role required | OAuth scope |
|---|---|---|
| `tools.aurora.moderator.*` | Moderator+ | `atproto:admin.moderation` or higher |
| `tools.aurora.admin.*` | Admin+ | `atproto:admin.*` |
| `tools.aurora.superadmin.*` | SuperAdmin | `atproto:admin.*` + `Role::SuperAdmin` check |
| `tools.aurora.ops.*` | Operator (Admin+) | `atproto:admin.server` |

The structure mirrors the three-tier role hierarchy (`Moderator` / `Admin` / `SuperAdmin`) plus the operator concerns from §6. Every endpoint's auth requirement is visible from its namespace prefix — no need to read lexicon notes or hit endpoints to discover what scope is required. Middleware enforces the family-level scope check once for the whole namespace; per-endpoint logic stays simple.

The four-family layout follows the precedent set by `tools.ozone.*` (which has `tools.ozone.moderation.*`, `tools.ozone.team.*`, `tools.ozone.communication.*`, etc. as siblings rather than nested under a shared umbrella). It also composes cleanly with future sub-scoping — if granular moderator tiers (junior/senior moderator) emerge as a real need, they can land as `tools.aurora.moderator.junior.*` or extensions to the existing `Role` enum without restructuring the namespace.

A top-level `tools.aurora.describeCapabilities` peer to the four families serves as the namespace probe — anyone with any admin scope can call it, and the response describes which families are exposed and what's in each.

---

## 5. Aurora extensions surface

The endpoints below extend the admin/moderation surface beyond bsky-PDS-2025-Q1 parity. They distribute across the four namespaces from §4.1 according to the auth tier each requires. Each endpoint has a stated purpose, request/response sketch, and rationale grounded in the Rust opportunity.

The total surface is 13 endpoints plus the top-level `describeCapabilities` probe. None are required for bsky-PDS parity; all are additive.

### 5.1 Top-level probe

#### `tools.aurora.describeCapabilities`

**Type:** Query. **Auth:** Any admin scope (Moderator+ via OAuth `atproto:admin.moderation` or higher).

A probe endpoint that returns which `tools.aurora.*` families and endpoints this Aurora-Locus instance supports. The intent is to allow admin tools to gracefully degrade against Aurora-Locus instances at different versions, and to allow Aurora-Locus to advertise behavioral commitments (e.g., whether the audit log is hash-chained, whether real-time subscriptions are available).

**Response shape:**

```json
{
  "families": {
    "tools.aurora.moderator": ["queryEvents", "getEvent", "queryStatuses", "..."],
    "tools.aurora.admin": ["emitEvent", "..."],
    "tools.aurora.superadmin": ["grantRole", "revokeRole"],
    "tools.aurora.ops": ["listAccounts", "getInstanceMetrics", "..."]
  },
  "extensions": [
    { "name": "hash-chained-audit" },
    { "name": "realtime-events" },
    { "name": "event-variants", "value": ["accountTakedown", "..."] }
  ],
  "implementation": "aurora-locus",
  "version": "0.2.0"
}
```

**Why a top-level peer rather than per-family probes:** Probing isn't admin-tier-specific; a caller hitting Aurora-Locus for the first time wants to know what's available across all families before deciding which to call. A single peer endpoint avoids requiring callers to probe four families separately. The response is grouped by family for clarity.

The `extensions` array uses structured objects (name + optional value) so payload-bearing capabilities like `event-variants` advertise the full vocabulary, while bare-name extensions like `hash-chained-audit` declare behavioral commitments without payload.

### 5.2 `tools.aurora.moderator.*` — Moderator-tier endpoints

These are read-side operations and triage queries. Moderator-tier callers (the lowest admin tier) can use all of them. Higher-tier callers (Admin, SuperAdmin) implicitly satisfy the requirement.

#### `tools.aurora.moderator.subscribeModEvents`

**Type:** Subscription (WebSocket). **Auth:** Moderator+.

Real-time push of moderation events (takedowns, restores, label applications, report submissions, appeal reviews) as they occur. Subscribers receive each event with full payload immediately on commit.

**Connection:** `wss://aurora-locus.example/xrpc/tools.aurora.moderator.subscribeModEvents?cursor=<seq>` where `<seq>` is the last-seen monotonic event-sequence integer (optional; omitted on first connect).

**Frame format (server → client):**

```json
{
  "seq": 12345,
  "eventType": "accountTakedown",
  "payload": {
    "subject": { "$type": "com.atproto.admin.defs#repoRef", "did": "did:plc:..." },
    "actor": "did:plc:moderator",
    "comment": "...",
    "timestamp": "2026-04-30T12:34:56Z"
  }
}
```

**Cursor semantics:**
- `seq` is a monotonic int64 from a dedicated mod-event sequencer that persists to Postgres
- The sequencer reads the current max `seq` from the audit table on startup, so restarts don't collide
- Retention floor: 7 days (operators may configure longer; shorter is non-conformant)
- If a client's cursor is older than the retained window, the server returns `OutdatedCursor` and closes the connection — the client re-bootstraps via `queryEvents` (§5.2) and resubscribes with a fresh cursor

**Why:** Aurora-Locus's existing sequencer already streams the public firehose; extending it to a moderation channel is an incremental addition rather than new infrastructure. Real-time event delivery to admin tools eliminates polling and surfaces moderation activity as it happens — useful for admin web UIs (the `static/admin/` interface), external moderation tools, and compliance dashboards.

bsky-PDS does not expose this. The Rust + axum + sequencer combination makes it natural for Aurora-Locus.

#### `tools.aurora.moderator.queryEvents`

**Type:** Query. **Auth:** Moderator+.

Paginated query over the moderation event timeline, filterable by event type, actor, subject, and time range. Returns the same event shape as `subscribeModEvents` for read-side consistency.

**Request:** `{ "eventType": "...", "actor": "did:...", "subject": {...}, "timeRange": [start, end], "cursor": "...", "limit": 50 }`

**Response:** `{ "items": [...], "cursor": "..." }`

**Why:** The existing `getAuditLog` endpoint (parity-floor) returns audit log entries. `queryEvents` is the moderation-event-specific equivalent with richer filters tuned to moderation workflows. Use cases: "what did moderator X do this week?", "show me all label applications on this subject," "show me everything that happened during this time window."

Aurora-Locus's `AuditLogEntry` plus the structured `ModerationEventType` enum make this a query against an existing table with appropriate indexes — no new domain logic.

#### `tools.aurora.moderator.getEvent`

**Type:** Query. **Auth:** Moderator+.

Singular fetch by event ID. Returns the full event record.

**Request:** `{ "eventId": "..." }`

**Response:** The event record as defined by `tools.aurora.admin.defs#modEventView`.

**Why:** Without `getEvent`, event IDs returned by `emitEvent` (§5.3) and surfaced in `subscribeModEvents` are unaddressable. Deep-linking to specific events from admin UI screens, audit reports, or compliance documentation requires a singular-fetch primitive.

#### `tools.aurora.moderator.queryStatuses`

**Type:** Query. **Auth:** Moderator+.

Paginated query over current subject statuses (takedown / deactivation / label state per subject), filterable by status, label, subject type, account, and time range. The "moderation queue" view that admin UIs show — what subjects need attention, what's already actioned.

**Request:** Filterable by `status`, `labels`, `subjectType`, `account`, time range, plus pagination.

**Response:** `{ "items": [...], "cursor": "..." }`

**Why:** Distinct from `queryEvents` because `queryStatuses` answers "what's the current state of subjects?" not "what events have happened?" — complementary and orthogonal. An admin UI's queue view loads from `queryStatuses`; an admin UI's activity log loads from `queryEvents`.

#### `tools.aurora.moderator.getSubjectContext`

**Type:** Query. **Auth:** Moderator+.

Returns case-view context for a subject in a single transactional call. Optional `include` parameter selects which sub-resources to join, so callers pay only for what they need.

**Request:**

```json
{
  "subject": { "$type": "com.atproto.admin.defs#repoRef", "did": "..." },
  "include": ["status", "actionHistory", "labelHistory", "reports", "appeals", "inviteLineage", "recentActivity"]
}
```

**Default `include`:** `["status", "actionHistory"]` — minimum useful payload.

**Response:** A joined view with the requested sub-resources, plus a base `subject` and `status` field always present.

**Why:** Admin UI case-view screens load multiple related records about a subject simultaneously: current takedown state, history of actions, label history, report counts, invite lineage, recent activity. Without this endpoint, building that screen requires 5+ sequential XRPC calls. The performance impact for admin UI responsiveness is real.

The opt-in `include` parameter prevents the endpoint from becoming a kitchen sink that grows unboundedly. New context dimensions can be added as new `include` values without forcing existing callers to handle larger responses.

#### `tools.aurora.moderator.getSubjectHistory`

**Type:** Query. **Auth:** Moderator+.

Paginated full state-change history for a single subject. Where `getSubjectContext` is point-in-time-now snapshot, `getSubjectHistory` is the full audit timeline filtered to one subject.

**Request:** `{ "subject": <polymorphic>, "cursor": "...", "limit": 50 }`

**Response:** `{ "items": [...], "cursor": "..." }`

**Why:** The "history" tab on an admin UI subject view; appeal review (compare current state to state at action time); compliance/audit exports for a specific subject.

#### `tools.aurora.moderator.getReporterContext`

**Type:** Query. **Auth:** Moderator+.

Reporter-specific context: how many reports filed, how many actioned vs rejected, prior actions taken against the reporter as a subject, top reported subjects.

**Request:** `{ "account": "did:plc:..." }`

**Response:**

```json
{
  "account": "did:plc:...",
  "totalReportsFiled": 47,
  "reportsActioned": 12,
  "reportsRejected": 30,
  "reportsPending": 5,
  "actionsAgainstReporter": [...],
  "topReportedSubjects": [...]
}
```

**Why:** Distinguishing good-faith reporters from harassment vectors is a real moderation triage task. Aggregating these stats per-reporter on the fly is expensive at scale; a Postgres rollup table maintained as reports are filed/resolved makes this cheap.

This endpoint is one of three (alongside `getModeratorActivity` and `getModerationMetrics`) where the Rust-enabled differentiator is "Postgres rollup tables make queries that would be slow on the fly into fast lookups." If Workstream B (Postgres backend) is not yet shipped, the endpoint can use on-the-fly aggregation as a fallback — slower for high-volume reporters but functionally correct.

#### `tools.aurora.moderator.getModeratorActivity`

**Type:** Query. **Auth:** Moderator+ (typically self or higher-tier reviewer).

Per-moderator stats and activity timeline. Surfaces accountability data: actions taken, cases resolved, appeals overturned, average time-to-resolution, recent activity.

**Request:** `{ "account": "did:plc:...", "timeRange": [...], "cursor": "...", "limit": 50 }`

**Response:** Stats object plus paginated activity timeline.

**Why:** Moderator accountability is a feature, not a leak. Admin UIs surface this to higher-tier reviewers (Admins or SuperAdmins reviewing Moderator work) for performance reviews and to moderators themselves for self-tracking. Operators may also choose to surface aggregate moderator activity in transparency reports.

Same Postgres-rollup affordance as `getReporterContext`.

#### `tools.aurora.moderator.getModerationMetrics`

**Type:** Query. **Auth:** Moderator+.

Aggregate moderation metrics: open cases, open appeals, queue size by status, average resolution time, escalation rate, action counts over recent windows.

**Request:** Optional `metrics: string[]` to subset (default: all).

**Response:**

```json
{
  "openCases": 12,
  "openAppeals": 5,
  "queueByStatus": { "pendingReview": 7, "actionedNotResolved": 3 },
  "averageResolutionTime": "PT4H15M",
  "escalationRate": 0.08,
  "actionsLast24h": 45,
  "actionsLast7d": 312
}
```

**Why:** Admin dashboard widgets (in `static/admin/` or external tools) display these aggregates at a glance. Without rollup tables, every dashboard refresh runs full-table scans — viable for low-volume instances, materially degraded for high-volume ones. The endpoint exposes the data; whether the underlying query is from rollups or live aggregation is operator infrastructure, not API contract.

#### `tools.aurora.moderator.getAuditTrail`

**Type:** Query. **Auth:** Moderator+.

Paginated query over the moderation audit log with optional cryptographic verification. Each row represents one state-changing event with actor, subject, timestamp, and the event payload.

**Request:** `{ "cursor": "...", "limit": 50, "filter": { "account": "...", "subject": <polymorphic>, "eventType": "...", "timeRange": [...] } }`

**Response:**

```json
{
  "items": [
    { "id": "...", "previousHash": "...", "currentHash": "...", "event": {...}, "createdAt": "..." }
  ],
  "cursor": "...",
  "verified": true
}
```

**Why hash-chaining:**

If the audit log is a hash chain — each row's `currentHash` derived from `previousHash` plus row content — retroactive modification breaks the chain at the modified row. Compliance auditors and external reviewers can verify integrity by re-hashing the chain. The single-writer architecture makes this affordable: one writer, total order, no chain-fork problem.

The hash chain is an addition to the existing `AuditLogEntry` schema, not a replacement. Existing fields (`admin_did`, `action`, `subject_did`, `details`, `timestamp`, `ip_address`) all remain; `previous_hash` and `current_hash` are added per row. Migration is a schema change that backfills hashes computed across existing rows in order.

A note on operational integrity: hash chaining provides tamper-evidence within a continuous chain. Failover to a replicated standby or restore from backup creates a chain discontinuity that hash chaining alone cannot disguise. Aurora-Locus deployments that need cross-discontinuity verification need additional operator-attested epoch transitions; that's deferred work outside the scope of v0.2 shipping. Within a continuous chain, the hash invariant holds.

**Capability advertisement:** the `hash-chained-audit` extension in `describeCapabilities` indicates the chain is in use. Older Aurora-Locus versions or operators who haven't run the migration return `verified: false` and omit the extension; clients can downgrade gracefully.

#### `tools.aurora.moderator.getAppeal`

**Type:** Query. **Auth:** Moderator+.

Singular fetch for an appeal record by ID, including the original event being appealed and the resolution state if resolved.

**Request:** `{ "appealId": "..." }`

**Response:** The appeal record with linked original event, filing actor, current state.

**Why dedicated read endpoints for appeals:**

The action side of appeal lifecycle (resolve an open appeal) flows through `emitEvent` (§5.3) as `appealResolveEvent` for consistency with the unified action primitive. The read side benefits from dedicated endpoints because admin UI appeal screens want appeals-specific shapes — current status, original action under appeal, filing actor, time-to-resolution — not the generic event vocabulary.

#### `tools.aurora.moderator.listAppeals`

**Type:** Query. **Auth:** Moderator+.

Paginated list of appeals filterable by status, subject, and time range.

**Request:** Filterable by `status`, `subject`, `account` (filing), time range, plus pagination.

**Response:** `{ "items": [...], "cursor": "..." }`

**Why:** Admin UI "appeals queue" screen — moderators reviewing pending appeals load this with `status: "open"`. Distinct from `queryStatuses` because appeals have their own lifecycle (filed → resolved with disposition) that benefits from purpose-shaped responses rather than the generic subject-status shape.

#### `tools.aurora.moderator.getInviteLineage`

**Type:** Query. **Auth:** Moderator+.

Given an account, return the invite tree: who invited the account, who they invited, recursively up to a depth limit. Surfaces invite-ring patterns useful for spam-ring investigations.

**Request:** `{ "account": "did:plc:...", "depth": 3 }`

**Response:**

```json
{
  "account": "did:plc:...",
  "invitedBy": { "account": "...", "code": "...", "createdAt": "..." },
  "invited": [
    { "account": "...", "code": "...", "createdAt": "...", "status": "..." }
  ],
  "ancestors": [...],
  "descendants": [...]
}
```

**Why:** Aurora-Locus already tracks invite usage through the existing `invite_code` and related tables. Surfacing the lineage as a graph query gives moderators investigating spam rings a fast path to identify connected accounts. The data is already there; the endpoint exposes it.

### 5.3 `tools.aurora.admin.*` — Admin-tier endpoints

Destructive moderation actions live here. Moderator-tier callers cannot reach these endpoints — they're view-only per the existing `Role::Moderator` semantics. Admin-tier callers and SuperAdmin callers can.

#### `tools.aurora.admin.emitEvent`

**Type:** Procedure. **Auth:** Admin+.

Unified state-changing primitive for all moderation actions. Polymorphic event types (takedown, restore, label apply/remove, appeal resolve, comment, report on behalf of) and polymorphic subjects via a `subjects: union<...>[]` array.

**Request:**

```json
{
  "event": {
    "$type": "tools.aurora.admin.defs#takedownEvent",
    "comment": "...",
    "ref": "..."
  },
  "subjects": [
    { "$type": "com.atproto.admin.defs#repoRef", "did": "..." }
  ],
  "createdBy": "did:plc:..."
}
```

**Response:** The created event record(s) with `id`, `createdAt`, `chainPosition`.

**Event variants in `tools.aurora.admin.defs`:**
- `takedownEvent`, `restoreEvent` — apply or remove takedown
- `labelEvent` — apply or remove a label (action embedded in payload)
- `appealResolveEvent` — uphold/reject/modify a pending appeal
- `commentEvent` — add a moderator comment to a subject's record
- `reportOnBehalfOfEvent` — moderator-side report (file on behalf of user, or internal-only report)

Note: `roleGrantEvent` and `roleRevokeEvent` are *not* in this vocabulary. Role management is SuperAdmin-only and lives in `tools.aurora.superadmin.*` (§5.4) as dedicated endpoints rather than event variants under `emitEvent`. This keeps the auth check at the namespace boundary rather than dispatched per-event-variant inside the handler.

**Why a single primitive instead of N separate endpoints:**
- One audit-trail format. Every state change produces one entry, regardless of action type or subject count.
- One permission-check site. Admin scope is enforced at the namespace boundary; per-variant checks (e.g., comment-only vs takedown) can layer on if needed but the base is uniform.
- One transactional commit. Multi-subject batches commit atomically — either all subjects' state changes apply and the audit chain advances by one parent row, or none do. Single-writer architecture (one Aurora-Locus binary against one Postgres backend) makes atomicity straightforward; partial failure within a batch is impossible because no concurrent writer can leave the system in a half-applied state.

**Why subjects-as-array:**

Multi-subject takedowns (a spam ring of 50 accounts, taken down together with one audit-trail row linking N child events) are a real moderation workflow. Without batch support, taking down 50 accounts is 50 separate XRPC calls, 50 separate audit rows, and no atomicity guarantee that the 50 actions land together. With `subjects` as an array, the batch case is the same primitive as the singular case (`subjects: [oneSubject]`), the audit-trail row links N children when `len(subjects) > 1`, and the atomicity is structural.

This is the single biggest moderation-workflow advantage Aurora-Locus offers over bsky-PDS. bsky-PDS exposes per-subject endpoints with no batch primitive; spam-ring takedowns require external orchestration with no atomicity. Aurora-Locus's Postgres-backed transaction model makes batch atomicity natural.

### 5.4 `tools.aurora.superadmin.*` — SuperAdmin-tier endpoints

Role management — granting and revoking moderator/admin/superadmin roles — lives here. SuperAdmin is the only role that can call these endpoints; the namespace prefix makes that requirement structurally visible.

#### `tools.aurora.superadmin.grantRole`

**Type:** Procedure. **Auth:** SuperAdmin only.

Grants a moderator/admin/superadmin role to an account. Records the grant in the `admin_role` table with grantor, grantee, role tier, and timestamp; emits a corresponding audit-log entry.

**Request:**

```json
{
  "account": "did:plc:...",
  "role": "moderator | admin | superadmin",
  "comment": "..."
}
```

**Response:** The created role record.

**Why dedicated endpoints rather than event variants on `emitEvent`:**

Role grants are a different category of operation from moderation actions. They modify the auth substrate itself rather than apply moderation outcomes to users' content or accounts. Keeping role management in `tools.aurora.superadmin.*` makes the auth boundary structurally visible — no caller without SuperAdmin scope ever reaches these endpoints, regardless of what event variants their `emitEvent` request might claim.

This also avoids a tricky middleware case: if `emitEvent` accepted a `roleGrantEvent` variant, the handler would need to do an additional SuperAdmin check after the namespace-level Admin check. Splitting the endpoints lets each namespace's middleware enforce a single auth tier cleanly.

#### `tools.aurora.superadmin.revokeRole`

**Type:** Procedure. **Auth:** SuperAdmin only.

Revokes a previously-granted role. Marks the role record as revoked (preserving audit history) and emits a corresponding audit-log entry.

**Request:**

```json
{
  "account": "did:plc:...",
  "role": "moderator | admin | superadmin",
  "comment": "..."
}
```

**Response:** The updated role record (with `revoked_at`, `revoked_by` populated).

A note on Aurora-Locus's existing `grantRole`/`revokeRole`/`listRoles` endpoints: these currently live at `com.atproto.admin.*`. They're not part of the bsky-PDS-2025-Q1 surface (bsky-PDS doesn't have a role concept; admin is gated by ADMIN_PASSWORD only), so they belong in `tools.aurora.superadmin.*` rather than in the parity-floor namespace. This relocation should happen as part of Phase 2 (namespace cleanup) — see §7.

### 5.5 `tools.aurora.ops.*` — Operator-tier endpoints

Aurora-Locus's existing operator extensions (blob ops, sequencer ops, federation status, rate-limit ops, health metrics) plus the two new endpoints introduced in §6 below.

The operator namespace is detailed in §6 rather than enumerated again here; this subsection exists to make the four-family layout complete and to call out that two endpoints are *added* under this namespace as part of the assessment work:

- `tools.aurora.ops.listAccounts` — broader account filtering preserving Aurora-Locus's existing `listAccounts` capability beyond bsky-PDS's `searchAccounts`
- `tools.aurora.ops.getInstanceMetrics` — operator-flavored aggregate metrics (system health, resource usage, account growth, federation health) to complement `tools.aurora.moderator.getModerationMetrics`'s moderation-flavored metrics

---

## 6. Operator extensions namespace (`tools.aurora.ops.*`)

The ~35 endpoints in Aurora-Locus's current `com.atproto.admin.*` surface that serve operator concerns rather than core admin or moderation — blob ops, sequencer ops, federation status, rate-limit ops, health metrics — do not belong in `com.atproto.admin.*`. None have bsky-PDS equivalents (bsky-PDS operators run shell scripts and check the database directly). All are legitimate operator capabilities for an Aurora-Locus deployment.

These should relocate to `tools.aurora.ops.*`, which serves Aurora-Locus operators, not protocol clients. The relocation lets `com.atproto.admin.*` be exactly the slim parity surface bsky-PDS exposes, while operator capabilities are visible and well-organized under their own namespace.

The relocation is mechanical: each `com.atproto.admin.<name>` operator extension moves to `tools.aurora.ops.<name>` with no semantic change — only routes and lexicon paths. The existing `static/admin/` web UI updates accordingly.

The endpoints involved (organized as they are in §1.2):
- Blob ops: `deleteBlob`, `quarantineBlob`, `restoreBlob`, `listBlobs`, `getBlobQuotas`, `getBlobStatistics`, `runBlobGC`
- Sequencer ops: `pauseSequencer`, `resumeSequencer`, `rebuildSequencer`, `resetSequencerCursor`, `getSequencerStatus`
- Federation ops: `getFederationStatus`, `getRelayConfig`, `triggerPdsDiscovery`, `listKnownInstances`
- Rate limit ops: `getRateLimitConfig`, `getRateLimitStatus`, `cleanupRateLimitState`
- Health and metrics: `getStats`, `getSystemHealth`, `getSystemMetrics`, `getDatabaseStatus`, `getResourceUsage`, `getVersionInfo`, `runHealthChecks`, `listBackgroundJobs`, `getValidationFailures`, `getNonceStoreStatus`, `cleanupNonceStores`

Plus two endpoints that should be added to the ops namespace as part of this relocation:

- `tools.aurora.ops.listAccounts` — broader account filtering (signup date, invite source, growth queries) preserving the capability that Aurora-Locus's existing `listAccounts` provided beyond bsky-PDS's `searchAccounts`
- `tools.aurora.ops.getInstanceMetrics` — operator-flavored aggregate metrics (system health, resource usage, account growth, federation health) to complement `tools.aurora.moderator.getModerationMetrics`'s moderation-flavored metrics

Auth-scope: `tools.aurora.ops.*` requires `atproto:admin.server` scope (operator-level). `atproto:admin.*` (full) implicitly satisfies it. `atproto:admin.moderation` does not — moderators don't need server operations.

---

## 7. Implementation phases

The work splits into three phases. Phase 1 is non-negotiable shipping. Phases 2 and 3 are extensions that ship as architecture and capacity allow.

### Phase 1 — Parity floor (must ship)

Close the gaps identified in §3, in dependency order:

- **1.1** Per-endpoint lexicon-shape audit (§3.6) for the 11 apparently-clean endpoints. File issues for any drift.
- **1.2** Implement the four parity-gap endpoints (§3.1–§3.4): `disableInviteCodes`, `getAccountInfo`, `searchAccounts`, `updateAccountSigningKey`.
- **1.3** Audit and modernize `updateSubjectStatus` polymorphism (§3.5). Mark legacy split-verb endpoints as deprecated.
- **1.4** Deprecation cycle for legacy renamed endpoints — `getAccount`, `listAccounts`, `getUsers`, `takedownAccount`, `suspendAccount`, `restoreAccount` — warnings in the next minor version, removal two minors later.

After Phase 1, Aurora-Locus matches bsky-PDS-2025-Q1 on `com.atproto.admin.*`. Anyone evaluating Aurora-Locus as a bsky-PDS alternative finds no missing parity.

### Phase 2 — Operator namespace relocation

- **2.1** Establish `tools.aurora.ops.*` with lexicons for the relocation map plus the two additions (§6).
- **2.2** Implement auth-scope separation: `atproto:admin.server` for ops, `atproto:admin.moderation` for admin/moderation, `atproto:admin.*` for both. (The OAuth scope hierarchy in §1.1 already supports this; only middleware enforcement work remains.)
- **2.3** Mechanical relocation per-endpoint. Update `static/admin/` web UI accordingly.
- **2.4** Deprecate the legacy operator endpoints in `com.atproto.admin.*`. Same cadence as Phase 1.4 — warnings, then removal.

After Phase 2, `com.atproto.admin.*` is exactly bsky-PDS-2025-Q1's surface. Operator extensions are in their own namespace with appropriate auth scope.

### Phase 3 — Aurora extensions (`tools.aurora.{moderator,admin,superadmin}.*`)

The 13 extension endpoints from §5 plus the top-level `describeCapabilities`, in rough order of consumer value and dependency:

- **3.1** Lexicon design pass for all 14 endpoints plus the shared `tools.aurora.admin.defs` (event variants, common types, error codes) and per-namespace defs files where shape divergence justifies them. Lexicons reviewed before handler implementation begins. Phase 3.1 is gating work for everything that follows; 3.2–3.9 ship independently once lexicons are stable.
- **3.2** Top-level probe: `tools.aurora.describeCapabilities` (§5.1). Foundation; cheap to ship.
- **3.3** Moderator-tier read endpoints (§5.2): `queryEvents`, `getEvent`, `queryStatuses`, `getSubjectContext`, `getSubjectHistory`. The base set that admin web UIs and external moderation tools depend on most.
- **3.4** Moderator-tier appeals reads (§5.2): `getAppeal`, `listAppeals`. Closely tied to 3.3 in admin UI value.
- **3.5** Admin-tier action surface: `tools.aurora.admin.emitEvent` (§5.3) with all event variants, atomic batch handling, audit-chain advancement.
- **3.6** SuperAdmin-tier role management (§5.4): relocate existing `grantRole`/`revokeRole` from `com.atproto.admin.*` to `tools.aurora.superadmin.*`. Add deprecation aliases at old paths per Phase 1.4 cadence.
- **3.7** Moderator-tier aggregations (§5.2): `getReporterContext`, `getModeratorActivity`, `getModerationMetrics`, `getInviteLineage`. Depends on Postgres rollup tables; on-the-fly fallback acceptable for low-volume instances.
- **3.8** Moderator-tier audit chain: `tools.aurora.moderator.getAuditTrail` (§5.2) with hash-chain schema migration. The migration is the gating piece — once the schema is in place and existing rows are backfilled, the endpoint is straightforward.
- **3.9** Moderator-tier real-time: `tools.aurora.moderator.subscribeModEvents` (§5.2). Largest single piece — sequencer-channel extension plus WebSocket handler plus cursor support per the locked lexicon.

Phases 3.2–3.9 can ship independently as their dependencies clear. None are required for parity; each adds value as it lands.

---

## 8. Open questions

These are genuinely open and want resolution before or during the lexicon design pass (Phase 3.1). Recommendations are stated where the doc is leaning a particular way; final calls belong to the reviewer.

### 8.1 The `com.aurora.federation.*` namespace

Aurora-Locus already exposes `com.aurora.federation.aggregateTimeline` and `com.aurora.dpop.getNonce` under the `com.aurora.*` prefix. This doc proposes new work under `tools.aurora.*`. Should the existing `com.aurora.*` endpoints consolidate to `tools.aurora.federation.*` for namespace consistency, or stay as-is?

**Recommendation:** Stay as-is unless there's a specific consolidation reason. `com.aurora.*` is shipped; consolidating is a breaking change that needs justification beyond aesthetics. If consolidation happens later, it follows the standard deprecation cycle.

### 8.2 Deprecation cadence for renames vs replacements

Phase 1.4 deprecates renamed endpoints (`getAccount` → `getAccountInfo`). Phase 2.4 deprecates relocated endpoints (operator extensions moving to `tools.aurora.ops.*`). Phase 3.6 deprecates the role-management endpoints relocating from `com.atproto.admin.*` to `tools.aurora.superadmin.*`. What's the timeline?

**Recommendation:** Two-version deprecation cycle for renames and relocations as a default — endpoint deprecated in v0.X emits warnings in v0.X+1 and is removed in v0.X+2. Same cadence regardless of whether the change is a rename or a relocation; consumers get a consistent migration window from release notes plus the deprecation warnings emitted by the legacy endpoints during the warning window.

Aurora-Locus has no known external consumers at this stage, so same-cycle removal is also acceptable where it simplifies the implementation — particularly for endpoints that ship their replacement in the same release. If unexpected external consumers surface, a v0.X.1 patch can restore endpoints temporarily while migration is coordinated.

### 8.3 Extension vocabulary in `describeCapabilities`

Should the set of valid extension names in `describeCapabilities` be lexicon-defined (a finite registered set in `tools.aurora.admin.defs#extensions`) or free-form (arbitrary strings)?

**Recommendation:** Lexicon-defined. Free-form invites version drift and one-off extensions that fragment the ecosystem. A finite registered set keeps the contract enforceable. The initial set is the four extensions named in §5.1 plus whatever falls out of Phase 3.1 lexicon design.

### 8.4 Error envelope conventions

ATProto lexicons declare error types per-endpoint. Should `tools.aurora.admin.*` share a common error vocabulary?

**Recommendation:** Yes — a shared `tools.aurora.admin.defs#errorCodes` enum with codes like `SubjectNotFound`, `InvalidEvent`, `PermissionDenied`, `OutdatedCursor`, `UnknownEventVariant`, `AppealNotFound`, `BatchValidationError`. Endpoints draw from the shared vocabulary plus declare endpoint-specific errors as needed. The shared vocabulary is finalized during Phase 3.1.

### 8.5 Granular moderator scopes within `tools.aurora.admin.*`

Currently the auth model has three roles (Moderator/Admin/SuperAdmin) and three OAuth scopes (`atproto:admin.*`/`atproto:admin.moderation`/`atproto:admin.server`). Within `tools.aurora.admin.*`, every moderator-scope endpoint allows any of the three roles. Should there be sub-scoping — e.g., a "junior moderator" who can apply labels but not take down accounts, or a "senior moderator" who can resolve appeals?

**Recommendation:** Defer sub-scoping to a future minor version. v0.2 ships single moderator scope; if real deployment demand emerges for finer-grained control, the existing Role enum extends to support it without breaking the lexicons (`Role::JuniorModerator`, `Role::SeniorModerator`, with `can_act_as` updated accordingly). The XRPC surface doesn't need to change for sub-scoping to land.

---

## 9. Closing

Aurora-Locus's existing admin and moderation infrastructure is more developed than the bsky-PDS comparison-based parity assessment first suggests. The surface area is roughly correct; what's needed is targeted parity work for ~5 specific gaps and a namespace cleanup that separates protocol-level admin from operator-level admin from Aurora-specific extensions.

The Aurora extension surface (`tools.aurora.{moderator,admin,superadmin}.*` plus `tools.aurora.describeCapabilities`) is the main differentiator opportunity. Aurora-Locus's architecture — Rust + axum + sequencer + Postgres + per-actor SQLite — affords admin and moderation capabilities that bsky-PDS does not expose. The 14-endpoint extension surface in §5 captures the capabilities that are both natural to ship given the architecture and useful enough to justify the maintenance cost, distributed across role-tiered namespaces so the auth boundary is structurally visible. Operators choosing Aurora-Locus get them as PDS-side capabilities; what tooling they build against them is operator-determined.

The work is non-trivial but bounded. Phase 1 (parity floor) is small and well-scoped; Phase 2 (namespace cleanup) is mechanical; Phase 3 (extensions) is the largest piece and can ship incrementally as dependencies clear. None of the phases require redesigning Aurora-Locus's existing infrastructure — they extend what's there.

Status as of this assessment: **parity gaps and extension surface identified, ready for chainlink issue creation against Phase 1 and Phase 3.1 (lexicon design)**. Phase 2 issue creation can proceed in parallel; Phases 3.2–3.9 issue creation depends on Phase 3.1 lexicon-design output.
