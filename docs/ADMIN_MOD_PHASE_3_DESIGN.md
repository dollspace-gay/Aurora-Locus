# Admin/Moderation Phase 3: Aurora Extensions

**Status:** design-locked, no production code yet
**Tracks:** chainlink #98 (this doc), #99–#106 (sub-phases 3.2–3.9)
**Reference:** [ADMIN_MODERATION_ASSESSMENT.md §7 Phase 3](../ADMIN_MODERATION_ASSESSMENT.md), [docs/AUDIT_lexicon_shape_phase1_1.md](AUDIT_lexicon_shape_phase1_1.md)

---

## 1. Goals

Phase 3 ships Aurora's extension surface — 14 endpoints across four
namespaces beyond the bsky-PDS-parity baseline established by
Phases 1 and 2. The deliverables:

- **Top-level capability probe** at `tools.aurora.describeCapabilities`
  so clients can discover which Aurora extensions are available.
- **12 moderator-tier endpoints** at `tools.aurora.moderator.*`
  exposing read-side queries (subject status, appeals, audit trail),
  aggregations (moderator metrics, queue stats), and a real-time
  WebSocket subscription for moderation events.
- **1 admin-tier endpoint** at `tools.aurora.admin.emitEvent` that
  accepts compositional `ModEvent` payloads, translates them to the
  storage-enum representation, and writes the audit log entry.
- **2 superadmin-tier endpoints** at `tools.aurora.superadmin.*`
  relocating role grant/revoke from `com.atproto.admin.*` per
  assessment doc §5.4 (keeps the SuperAdmin auth boundary
  structurally visible in the namespace).

Plus:
- **Hash-chain integrity** added to the existing `admin_audit_log`
  table — every new audit row is cryptographically linked to the
  previous, allowing operators to detect tampering or gaps.
- **`mod_event_seq` table** as a moderation-channel sibling to
  `repo_seq`, enabling the WebSocket subscription with cursor-based
  resume.

## 2. Non-goals

- **No workflow primitives.** Aurora doesn't ship case management,
  task assignment, escalation chains, or queue-of-the-day surfaces.
  Aurora gives moderators the data; teams build their own workflow
  on top (or use external moderator tools — see §3).
- **No external-tool detection or auto-adaptation.** Aurora's API
  shapes don't change based on whether ozone or another moderator
  surface is connected. Operators choose where to do which kind of
  work; Aurora's contracts are the same either way. (See §3.)
- **No protocol-level mode switching.** No "moderation mode" flag
  that swaps endpoint behavior. The same endpoints serve the
  built-in admin UI, external clients, and any future moderator
  surfaces identically.
- **No new event types.** Phase 3.5's `emitEvent` translates
  `ModEvent` (compositional, API-shaped) to existing
  `ModerationEventType` variants (subject-aware, storage-shaped) —
  see decision A. Adding new storage variants is its own work and
  out of Phase 3 scope.

## 3. Operator deployment framing

Aurora-Locus's admin UI in [static/admin/](../static/admin/) is a
**first-class moderator interface**, complete enough for
deployments running Aurora without external moderator tools. When
operators run Aurora alongside external moderator surfaces (e.g.,
ozone integrations), both UIs remain fully functional; operators
choose where to do which kind of work based on their team's
conventions. **Aurora does not detect or adapt to external
moderator tools at the protocol layer** — the admin UI's behavior
is the same whether external tools are connected or not.

This framing has implementation implications threaded through
Phase 3:

- **Each implementation sub-phase (3.2–3.9) includes substantive UI
  surface in `static/admin/`** for its endpoints. The UI isn't an
  afterthought; it's a peer client of the same APIs external tools
  would use.
- **Rich query response context** (resolved handles, subject
  metadata, cross-references) serves both Aurora's UI and any
  external client identically. The API doesn't return "thin"
  responses for one consumer and "rich" for another.
- **Rich event payloads** in `subscribeModEvents` are
  self-contained for rendering — they don't require the consumer
  to make follow-up queries to display the event meaningfully.
  Same payload, same self-containedness, regardless of consumer.
- **No mode-switching, no auto-detection, no protocol-level
  awareness of external tools.** Aurora's namespace ships as one
  coherent surface.

## 4. Foundation types

Decisions A, B, E, F captured here. These types live in
`src/admin/defs.rs` (a new module mirroring how `tools.aurora.admin.defs`
is the lexicon convention upstream) and are imported by per-namespace
endpoint modules.

### 4.1 Subject (decision B)

Three-variant `Subject` enum matching `com.atproto.admin.defs#repoRef`
/ `#strongRef` / `#repoBlobRef`. `$type`-discriminated serialization
for wire compatibility with ATProto convention.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "$type")]
pub enum Subject {
    #[serde(rename = "com.atproto.admin.defs#repoRef")]
    Repo { did: Did },
    #[serde(rename = "com.atproto.repo.strongRef")]
    Record { uri: AtUri, cid: Cid },
    #[serde(rename = "com.atproto.admin.defs#repoBlobRef")]
    Blob { did: Did, cid: Cid, record_uri: Option<AtUri> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubjectType {
    Account,  // matches Subject::Repo
    Record,
    Blob,
}
```

`Did`, `AtUri`, `Cid` are existing typed wrappers (or `String`
newtypes if not yet present — implementation Phase 3.2 picks).

`SubjectType` is the filter-parameter form used by query endpoints
that take a subject_type. Distinct from `Subject` (the value form)
because filters need to be parsed from query strings without
requiring a full subject identity.

### 4.2 ModEvent (decision A)

Compositional, subject-agnostic event vocabulary for the API
surface. Translation to/from the storage enum
`ModerationEventType` (12 subject-aware variants — see audit §A.1)
happens at write time in `emitEvent` and at read time in
`getAuditTrail` / event subscriptions.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "$type")]
pub enum ModEvent {
    #[serde(rename = "tools.aurora.admin.defs#takedown")]
    Takedown { comment: Option<String>, ref_: Option<String> },

    #[serde(rename = "tools.aurora.admin.defs#restore")]
    Restore { comment: Option<String> },

    #[serde(rename = "tools.aurora.admin.defs#label")]
    Label {
        create: Vec<String>,
        negate: Vec<String>,
        comment: Option<String>,
    },

    #[serde(rename = "tools.aurora.admin.defs#appealResolve")]
    AppealResolve {
        resolution: AppealResolution,
        comment: Option<String>,
    },

    #[serde(rename = "tools.aurora.admin.defs#comment")]
    Comment { text: String },

    #[serde(rename = "tools.aurora.admin.defs#reportOnBehalfOf")]
    ReportOnBehalfOf {
        reason: ReportReason,
        comment: Option<String>,
        on_behalf_of: Did,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppealResolution {
    Uphold,   // appeal granted; original action reversed
    Reject,   // appeal denied; original action stands
    Modify,   // appeal partially granted; new action substituted
}

// ReportReason is an existing enum in src/admin/reports.rs;
// re-exported here for ModEvent payload typing.
```

**Why two enums** — the API enum is subject-agnostic and
compositional (one `Takedown` variant applied to any `Subject`);
the storage enum is subject-aware (`AccountTakedown`, `RecordTakedown`,
`BlobQuarantine`) for queryability against the existing event log.
The translation layer in `emitEvent` reads `ModEvent` + `Subject`
and writes the appropriate `ModerationEventType` variant.

**Why role grant/revoke aren't here** — they live at
`tools.aurora.superadmin.*` as dedicated endpoints (decision per
assessment doc §5.4). Keeping them out of `ModEvent` makes the
SuperAdmin auth boundary structurally visible in the namespace
rather than buried in an `emitEvent` payload variant.

### 4.3 Pagination (decision E)

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

Cursor format: opaque to clients; internally a base64-encoded
JSON `{after_created: DateTime<Utc>, after_id: i64}`. Composite
cursor avoids collisions when multiple events share a timestamp
(common during bulk operations).

### 4.4 Errors (decision F)

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

Wire format: ATProto error envelope `{"error": "<CodeName>",
"message": "<optional human-readable>"}`. Per-endpoint error sets
can extend (e.g., a procedure-specific validation error) but
should reuse the shared vocabulary where it fits.

## 5. Per-namespace API surface

### 5.1 `tools.aurora.describeCapabilities` — top-level probe

**Type:** Query
**Auth:** Moderator+ via `AdminAuthContext` (matches Phase 2.3 ops
convention — scope alone isn't enough; role lookup confirms admin
status)
**Sub-phase:** 3.2 (#99)

Returns the set of Aurora extensions this server supports, so
clients can discover capabilities without trial-and-error against
endpoints.

```rust
struct DescribeCapabilitiesResponse {
    namespaces: Vec<NamespaceDescriptor>,
}

struct NamespaceDescriptor {
    nsid: String,                  // e.g. "tools.aurora.moderator"
    endpoints: Vec<String>,        // e.g. ["getSubjectStatus", "listAppeals"]
    version: Option<String>,       // e.g. "0.2.0"
}
```

The implementation enumerates registered routes under each Aurora
namespace prefix and returns the discovered endpoint names. No
schema-introspection — just naming.

### 5.2 `tools.aurora.moderator.*` (12 endpoints)

All require `atproto:admin.moderation` scope per Phase 2.2
namespace check. Listed by sub-phase grouping.

#### Reads (sub-phase 3.3, #100)

**`getSubjectStatus`** (Query)
- Input: `{ subject: Subject }`
- Output: `{ subject, takedown: Option<StatusAttr>, deactivated:
  Option<StatusAttr>, suspension: Option<StatusAttr>, recent_events:
  Vec<EventSummary> }`
- Rich context: resolved handles, recent event summaries (last 10
  events on this subject), pending appeals count.

**`getRecord`** (Query)
- Input: `{ uri: AtUri, cid: Option<Cid> }`
- Output: `{ uri, cid, value: serde_json::Value, repo: Did,
  takedown_status, blobs: Vec<BlobMeta> }`
- Rich context: blob metadata for any embedded blobs, repo handle.

**`getRepo`** (Query)
- Input: `{ did: Did }`
- Output: `{ did, handle, created_at, takedown_status,
  deactivated_at, label_count, recent_events_count, appeal_count }`
- Rich context: aggregate counts so UI can show a header summary
  without follow-up queries.

#### Queries (sub-phase 3.3 too)

**`queryEvents`** (Query)
- Input: pagination + filters `{ subject: Option<Subject>,
  subject_type: Option<SubjectType>, types: Vec<String>,
  created_by: Option<Did>, created_after: Option<DateTime>,
  created_before: Option<DateTime>, sort_direction: SortDirection }`
- Output: `PaginatedResponse<ModEventView>` where
  `ModEventView { id: i64, event: ModEvent, subject: Subject,
  created_by: Did, created_at: DateTime<Utc>, subject_handle:
  Option<String> }`
- Rich context: handle resolution for created_by + subject DIDs.

**`querySubjects`** (Query)
- Input: pagination + filters `{ subject_type: Option<SubjectType>,
  has_takedown: Option<bool>, has_open_appeal: Option<bool>,
  modified_after: Option<DateTime> }`
- Output: `PaginatedResponse<SubjectStatusSummary>` where each
  summary embeds the event counts that the UI's queue view needs
  without per-row queries.

#### Appeals reads (sub-phase 3.4, #101)

**`listAppeals`** (Query)
- Input: pagination + filters `{ status: Option<AppealStatus>,
  subject_did: Option<Did>, submitted_after: Option<DateTime> }`
- Output: `PaginatedResponse<AppealView>` where
  `AppealView { id, status, submitter_did, submitter_handle,
  subject: Subject, reason, submitted_at, original_action_summary,
  resolution: Option<AppealResolution> }`

**`getAppeal`** (Query)
- Input: `{ appeal_id: i64 }`
- Output: `AppealDetail` with the same fields as `AppealView` plus
  full event history (timeline of the appeal's lifecycle).

#### Aggregations (sub-phase 3.7, #104)

**`getModerationMetrics`** (Query)
- Input: `{ window: TimeWindow }` (e.g., `Last24h`, `Last7d`,
  `Last30d`)
- Output: `{ events_total, events_by_type: HashMap<String, u64>,
  appeals_total, appeals_by_resolution, takedowns_applied,
  takedowns_reversed, top_moderators: Vec<{did, handle, event_count}> }`
- Rich context: per-moderator activity helps team leads see
  workload distribution.

**`getQueueStats`** (Query)
- Input: none
- Output: `{ pending_appeals_count, open_reports_count,
  reports_awaiting_review_age_p50_secs, reports_awaiting_review_age_p95_secs }`
- Rich context: latency percentiles on report review serve as
  team SLA indicators.

#### Audit chain (sub-phase 3.8, #105)

**`getAuditTrail`** (Query)
- Input: pagination + filters `{ subject: Option<Subject>,
  actor_did: Option<Did>, action_filter: Option<Vec<String>> }`
- Output: `PaginatedResponse<AuditEntry>` where
  `AuditEntry { id, admin_did, action, subject_did, details,
  timestamp, ip_address, prev_hash: Option<String>,
  current_hash: String, verified: bool }`
- See §6.1 for hash-chain semantics.

#### Real-time (sub-phase 3.9, #106)

**`subscribeModEvents`** (WebSocket Subscription)
- Input: `?cursor=<seq>` (optional resume point)
- Output: stream of `{ seq: i64, event: ModEvent, subject: Subject,
  actor_did: Did, actor_handle: Option<String>, subject_handle:
  Option<String>, created_at: DateTime<Utc> }` JSON frames
- Self-contained payloads: subject and actor handles resolved
  server-side so consumers don't need follow-up queries to render
  the event.
- Resume semantics: `cursor` is the last `seq` the consumer
  acknowledged; server replays from `seq+1`. Frames within
  retention window (§6.2) replay fully; older frames return an
  `OutdatedCursor` error and the consumer must reconcile.

### 5.3 `tools.aurora.admin.*` (1 endpoint)

Auth: `atproto:admin.moderation` (admin-tier action endpoints
share scope with moderator-tier reads — admin is a permissions
escalation within the same namespace, not a different namespace).

**`emitEvent`** (Procedure, sub-phase 3.5, #102)
- Input: `{ event: ModEvent, subject: Subject, comment:
  Option<String> }`
- Output: `{ event_id: i64, audit_id: i64 }`
- Validates the `ModEvent` variant against the `Subject` type
  (e.g., `Takedown` valid for any Subject; `Label` only valid for
  Repo or Record subjects), translates to `ModerationEventType`
  storage variant, writes both the event log row and the audit
  log row in a transaction.
- The audit log row is hash-chained per §6.1.

### 5.4 `tools.aurora.superadmin.*` (2 endpoints)

Auth: `atproto:admin.moderation` plus `Role::SuperAdmin` check
at handler level (existing `require_admin_role!` macro from
[src/auth.rs](../src/auth.rs)).

**`grantRole`** (Procedure, sub-phase 3.6, #103)
- Input: `{ did: Did, role: Role, notes: Option<String> }`
- Output: `{ role_id: i64 }`
- Relocated from `com.atproto.admin.grantRole`. Aurora extension
  per assessment doc §5.4.

**`revokeRole`** (Procedure, sub-phase 3.6 too)
- Input: `{ did: Did, reason: Option<String> }`
- Output: `{ revoked: bool }`
- Relocated from `com.atproto.admin.revokeRole`.

After Phase 3.6 lands and Phase 2.4-style legacy removal happens,
`com.atproto.admin.grantRole`/`revokeRole` are deleted from the
parity surface.

**Asymmetry note: `listRoles` stays put.** `grantRole` and
`revokeRole` relocate to `tools.aurora.superadmin.*` because they're
destructive role-authority operations requiring SuperAdmin tier.
`listRoles` (read-side role discoverability — "who has what
role?") stays at `com.atproto.admin.listRoles` because it serves
all moderators legitimately and doesn't require SuperAdmin tier.
The asymmetry reflects "authority tier matches operation
destructiveness" rather than "all role-related endpoints belong to
one tier" — the same principle that keeps `getAuditLog` at the
moderator tier while `emitEvent` (which writes to it) requires
admin tier.

## 6. Schema additions

### 6.1 Hash-chain columns on `admin_audit_log`

Two new columns:

```sql
ALTER TABLE admin_audit_log
  ADD COLUMN prev_hash TEXT,
  ADD COLUMN current_hash TEXT;
```

Both nullable for the migration's sake. Nullability semantics:

- **Pre-migration rows** (existing audit entries): `prev_hash` =
  NULL, `current_hash` = `"pre-chain"` (sentinel). These rows
  were written before the chain existed and can't be retroactively
  hashed without inventing data — the sentinel says "this row
  predates the chain; hash verification is not applicable."
- **Post-migration rows**: `prev_hash` = the previous row's
  `current_hash` (or `NULL` for the very first chained row);
  `current_hash` = `SHA-256(prev_hash || canonical_serialize(
  admin_did, action, subject_did, details, timestamp))`. Forms a
  Merkle-style chain.

`getAuditTrail` returns `verified: bool` per row:
- `true` for post-migration rows where the recomputed hash matches
  the stored `current_hash` and the chain link to the previous row
  is intact.
- `false` for pre-migration sentinel rows (and for any
  post-migration row where verification fails — that's a
  tampering or schema-bug signal worth surfacing).

**Forward-looking only design** (decision C): backfilling synthetic
hashes for pre-protection rows would be misleading. Better to be
honest about which rows have actual cryptographic protection.

Migration shape (Phase 3.8):
```sql
-- migrations/0023_add_audit_hash_chain.sql (sequence number TBD)
ALTER TABLE admin_audit_log ADD COLUMN prev_hash TEXT;
ALTER TABLE admin_audit_log ADD COLUMN current_hash TEXT;
UPDATE admin_audit_log SET current_hash = 'pre-chain' WHERE current_hash IS NULL;
```

The post-migration path: every `log_action` call computes the
chain link from the most recent row's `current_hash` (a single
`SELECT current_hash FROM admin_audit_log ORDER BY id DESC LIMIT 1`)
and writes the new row in the same transaction.

**Concurrency**: simultaneous `log_action` calls would race on
"most recent row." Phase 3.8 implementation uses a serial
advisory lock keyed on the audit log table, or a SERIALIZABLE
transaction isolation level for the audit insert path. The lock
adds latency to audit writes (~5ms typical) which is acceptable
for moderation-frequency events.

### 6.2 `mod_event_seq` table for moderation channel

Independent of the existing `repo_seq` table (firehose channel).
Schema (Postgres; SQLite mirrors with `INTEGER PRIMARY KEY
AUTOINCREMENT` and `TEXT` for JSONB):

```sql
CREATE TABLE mod_event_seq (
    seq         BIGSERIAL PRIMARY KEY,
    event_type  TEXT NOT NULL,
    payload     JSONB NOT NULL,
    actor_did   TEXT NOT NULL,
    subject     JSONB NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_mod_event_seq_created_at ON mod_event_seq(created_at);
CREATE INDEX idx_mod_event_seq_actor ON mod_event_seq(actor_did);
```

(Per Phase 1 amendment: SQLite version uses TEXT for timestamps
and JSON-as-TEXT for the payload/subject columns. AnyPool
dispatches transparently.)

`subscribeModEvents` reads from this table with cursor-based
resume. Retention: 7-day floor, configurable via
`PDS_MOD_EVENT_RETENTION_DAYS` env var. Background cleanup job
deletes rows older than the retention window — pattern matches
the existing `account-purge` job in [src/jobs/tasks.rs](../src/jobs/tasks.rs).

`emitEvent` writes both `mod_event_seq` and `admin_audit_log` in
the same transaction. The two serve different consumers:
`mod_event_seq` for live subscribers, `admin_audit_log` for
forensic queries with hash-chain integrity.

## 7. UI scope expectations per sub-phase

Each sub-phase includes substantive [static/admin/](../static/admin/)
UI work for the endpoints it ships. Not exhaustive — the existing UI
patterns (multi-page SPA with hash routing, OAuth token auth, modal
detail views — see audit §5) apply.

| Sub-phase | New / extended UI surface |
|---|---|
| 3.2 | Settings → Capabilities probe panel |
| 3.3 | Subject detail modal extended; new "Search" page |
| 3.4 | Appeals queue page (list + detail modal) |
| 3.5 | Subject detail modal: action surface (takedown, restore, label, comment) |
| 3.6 | Settings → Role management page (SuperAdmin only) |
| 3.7 | Dashboard: metrics cards; new "Insights" page for time-window analysis |
| 3.8 | Subject detail modal: audit trail tab with verification badges |
| 3.9 | Real-time event feed sidebar (always-on toast / panel) |

Per §3, the UI surface isn't an afterthought; it's a peer client
of the same APIs external moderator tools would use.

## 8. Implementation sub-phase ordering

Per assessment doc §7, sub-phases ship in the order 3.1 → 3.9.
Each sub-phase has an independent verification gate:
- Lib tests stable
- New endpoint(s) covered by per-endpoint tests
- UI surface functional (manual smoke against a running instance is
  acceptable; full e2e UI tests deferred to a future cycle)

After 3.5 lands (`emitEvent`), 3.7 (aggregations) and 3.8 (audit
chain) can ship in either order — they don't depend on each other.
3.9 (subscription) depends on 3.5's writes producing rows in
`mod_event_seq`, so it ships after 3.5.

3.6 (SuperAdmin role mgmt relocation) is independent of every
other sub-phase and can ship at any point after 3.1 closes.

## 9. Open questions

### 9.1 No "review-pending" subject state today

Audit finding (§A.2): Aurora-Locus has no explicit "review-pending"
state for actors. Subject status surfaces are:
- `actor.takedown_ref` (account is taken down)
- `actor.deactivated_at` (user-initiated deactivation)
- `account_moderation.reversed` (action reversed)
- AppealStatus pipeline: Pending → UnderReview → {Approved,
  Denied, Escalated}
- ReportStatus pipeline: Open → Acknowledged → Resolved

Phase 3 does not introduce a new "review-pending" state. The
existing report and appeal status pipelines cover the
pending-review semantics adequately. If future operator feedback
shows the need for a queue-of-the-day primitive that flags
subjects needing review (vs the current report-driven model),
that's a follow-up cycle's design call.

### 9.2 Hash-chain advisory lock granularity

§6.1 mentions Phase 3.8 will use either an advisory lock or
SERIALIZABLE isolation for the audit insert path. The choice
between the two is deferred to implementation:
- Advisory lock is simpler, keeps existing isolation level for
  other audit-table reads.
- SERIALIZABLE catches more concurrency anomalies but affects
  every transaction touching the table, not just inserts.

Implementation will pick based on what reads against the audit
table look like at Phase 3.8 time.

### 9.3 ReportOnBehalfOf semantics

`ModEvent::ReportOnBehalfOf` (§4.2) lets an admin file a report
on behalf of another DID. The `on_behalf_of` field captures who
the report is FROM the perspective of (the user the admin is
representing), not who created the audit row (which is the
admin). Phase 3.5's implementation needs to clarify in the
storage representation:
- Does `report.reported_by` = admin or = on_behalf_of?
  Recommendation: `reported_by = admin` (preserves the audit
  trail), with a separate `on_behalf_of` column added if needed.

Deferred to Phase 3.5 design.

### 9.4 Capability versioning

`describeCapabilities` returns optional `version` per namespace
(§5.1). Phase 3 ships v0.2; how do future minor versions advertise
themselves? Two options:
- Version bumps reflected in the response (clients check version
  to know which features are available).
- Version stays static; new endpoints just appear in the
  `endpoints` list and clients infer support from presence.

Phase 3.2 implementation picks; either is workable.

---

## Appendix A — Audit raw findings

Conducted Phase 3.1 on commit `fc57aa2` (Postgres workstream head).

### A.1 ModerationEventType (12 variants, subject-aware)

[src/admin/events.rs:18–45](../src/admin/events.rs#L18). Subject-aware
storage representation:

```text
AccountTakedown, AccountSuspend, AccountWarn, AccountRestore,
LabelCreate, LabelRemove, BlobQuarantine, BlobRestore,
ReportSubmit, ReportReview, AppealSubmit, AppealReview
```

Logging method `ModerationEventLogger::log_event` at
[src/admin/events.rs:117–167](../src/admin/events.rs#L117). Phase 3.5's
translation layer reads `(ModEvent, Subject)` and writes the
appropriate variant.

### A.2 Subject status states

- `actor.takedown_ref TEXT NULL`: non-null → taken down
- `actor.deactivated_at TEXT NULL`: user-initiated deactivation
  (RFC3339)
- `account_moderation.reversed BOOLEAN`: reversal flag
- AppealStatus enum: Pending, UnderReview, Approved, Denied,
  Escalated
- ReportStatus enum: Open, Acknowledged, Resolved

No "review-pending" state exists. See open question §9.1.

### A.3 admin_audit_log schema (both backends)

```sql
admin_audit_log:
  id            (PK; INTEGER autoinc / BIGSERIAL)
  admin_did     TEXT NOT NULL
  action        TEXT NOT NULL
  subject_did   TEXT
  details       TEXT
  timestamp     TEXT NOT NULL  (RFC3339)
  ip_address    TEXT
```

Indices: `idx_admin_audit_admin (admin_did)` on both;
`idx_admin_audit_timestamp` on Postgres only.

### A.4 repo_seq schema (both backends)

```sql
repo_seq:
  seq          (PK; INTEGER autoinc / BIGSERIAL)
  did          TEXT NOT NULL
  event_type   TEXT NOT NULL
  event        BLOB / BYTEA NOT NULL
  invalidated  INTEGER 0/1 / BOOLEAN
  sequenced_at TEXT NOT NULL  (RFC3339)
```

Indices: `idx_repo_seq_did`, partial
`idx_repo_seq_seq WHERE NOT invalidated`.

`mod_event_seq` (§6.2) mirrors this conventions with payload + subject
JSONB columns added.

### A.5 static/admin/ structure

Multi-page SPA with hash-based routing, OAuth token auth in
`localStorage`, modal detail views. 6 routes today: Dashboard,
Users, Moderation Queue, Reports, Invites, Settings.
[script.js](../static/admin/script.js) is the SPA controller (~692
lines); per-page logic lives there. Phase 3 UI work extends this
pattern (no rewrite to a different framework).

### A.6 Reusable helpers from prior cycle work

- `parse_timestamp` / `opt_parse_timestamp`: duplicated across
  modules (Phase 3 cycle pattern). New Phase 3 modules should
  duplicate the same shape until a centralization pass happens.
- `read_bool` (centralized at [src/db/mod.rs](../src/db/mod.rs)):
  cross-backend boolean column read. Phase 3 modules use this for
  any `BOOLEAN` column reads (e.g., audit chain `verified` if
  stored as a column rather than computed at read).
- `crate::sequencer::PostgresLockProvider`: the advisory-lock
  pattern from Phase 4.2 is the natural reference for
  Phase 3.8's hash-chain advisory lock.
