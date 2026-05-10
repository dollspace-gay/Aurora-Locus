# Aurora-Locus v0.4 candidates

This document tracks v0.3-deferred work and other v0.4
candidate items. Maintained from the v0.3 cycle close (Arc 5
Step 4 / chainlink #123 handoff) onward.

The full v0.3 design corpus is at [`V03_DESIGN.md`](V03_DESIGN.md);
this document is the forward-looking complement that survives
into v0.4 cycle planning.

---

## Named deferrals from v0.3

### #123 — LB-3 runtime route enumeration

**Status**: Deferred from v0.3 cycle close. Open in the
`chainlink` CLI (`.chainlink/issues.db`); not yet scheduled
into a v0.4 arc.

**Background**: `tools.aurora.admin.describeCapabilities`
advertises a hand-curated capability list rather than
enumerating routes at runtime. The v0.2 reconciliation
conclusion (manual list stays) was carried forward into v0.3 —
the maintenance burden is bounded by a drift-detection test
that fails CI when a registered route doesn't appear in the
hand-curated list.

**Implementation paths considered**:
- **Proc-macro generation**: scrape route-registration macros
  at compile time and emit the capability list as a const.
  Build-system dependency; `proc-macro` crate authoring is
  arc-sized.
- **Runtime introspection**: walk the axum `Router` after
  construction and surface the registered routes through a
  read-only API consumed by `describeCapabilities`. Structural
  shift in how the route table is constructed; arc-sized.

Both paths require explicit v0.4 design work. v0.3 doesn't pick
one.

**Recommendation for v0.4**: open the design conversation
early in the v0.4 cycle, pick one approach, and design the arc
around it. The drift-detection test ensures v0.4 has a complete
inventory of routes to migrate from.

**Maintained surface in the interim** (load-bearing for v0.3
correctness — do NOT remove or weaken without the v0.4
replacement landing):

- Hand-curated capability families:
  `src/api/admin.rs:2939-3013` (`fn aurora_capability_families`).
- Hand-curated capability extensions:
  `src/api/admin.rs:3041-3067` (`fn aurora_capability_extensions`).
- Drift-detection test:
  `src/api/admin.rs:7223-7331` (`describe_capabilities_snapshot`).
  Asserts the hand-curated list matches the registered routes.

**Code-level anchors**: `// TODO(#123, v0.4): runtime route
enumeration deferred per V03_DESIGN.md §9 and
docs/v04-candidates.md` at `src/api/admin.rs:2939` and `:3041`.
Grep `TODO(#123, v0.4)` to locate.

---

## v0.4 candidates accumulated during v0.3 cycle

Items below were flagged during v0.3 implementation as worth
addressing in v0.4. Grouped by category. Items are advisory —
not chainlinks until promoted via `chainlink create` at v0.4
cycle open.

### Architectural follow-ups

1. **#123 runtime route enumeration** — see "Named deferrals"
   above. Architectural; arc-sized.
2. **`exportAccountForensic` shape rationalization**. The
   forensic-export endpoint uses a separate response shape that
   drops several fields and emits i64 ids as JSON numbers (not
   stringified) — distinct from the `getAuditTrail` audit-chain
   wire shape. Rationalizing the two surfaces is a v0.4
   follow-up flagged in Arc 3 cycle work and reaffirmed in Arc 4
   `docs/operator/audit-chain-verification.md` ("Out of scope").
3. **Aurora Admin UI v0.3 migration**. Arc 4's `emitEvent`
   reshape, the dropped batch-handler `failures` field, the
   per-subject error envelope, the `affected_count` semantic
   shift, the new `SettingSource: "File"` value, and the three
   new HTTP 400 error codes from emitEvent's embedded-id
   validation all require coordinated UI changes. Arc 3's
   `cascadeSnapshotIds` and `subject_cid` filter and Arc 5
   Step 3's `timeRange` canonical shape are net-new feature
   surfaces the UI can adopt. Arc 1 Step 4's `grant-admin` CLI
   command needs a steady-state UI counterpart per the CLI/UI
   parity principle (see "Project patterns / conventions"
   below). The Admin UI is a separate codebase per Arc 4's
   scope decision; the migration is tracked separately and is
   v0.4 work. **Full enumeration in the "Aurora Admin UI v0.3
   migration scope" subsection below.**
4. **GC sweep for orphaned blob storage**. `DeleteBlob`'s
   post-commit backend storage delete (Arc 4 §8.4.1, Phase 4
   deferred-action) is best-effort with WARN-on-failure. Orphan
   storage on backend-delete failure accumulates until a GC
   sweep reconciles. v0.4 candidate per the Arc 4 close
   commentary.
5. **File-tier reload-on-SIGHUP** (#124, Arc 5 Step 2 follow-up).
   The file tier is read once at `AppContext::new`; runtime
   changes to `runtime.yaml` aren't picked up until restart. A
   reload-on-SIGHUP path would let operators bump file-tier
   values without restarting. Out of scope per Arc 5 Step 2's
   recon Q5 decision; `setRuntimeSetting` covers the in-process
   hot path.
6. **Compositional reshape of `ModEventAction`** (#125
   v0.4-or-later candidate). The flat 16-variant enum is the
   v0.3 committed contract. A compositional reshape (separating
   action-verb from subject-type into orthogonal enum axes)
   is a v0.4-or-later candidate gated on use-case surface —
   no consumer demand has surfaced during v0.3 to motivate the
   wire-format break.

### Code-level cleanups

7. **Pre-existing clippy lint cleanup**. 21 clippy `-D warnings`
   errors exist in pre-Arc-5 code (`src/api/aurora_admin.rs`,
   `src/oauth/token.rs`, `src/api/admin.rs`, `src/admin/defs.rs`):
   `dead_code` on three subject-extractor helpers
   (`require_repo_did`, `subject_uri_cid`, `require_blob_cid`),
   `redundant_closure`, `useless_format`, `clamp-like pattern`,
   `doc list item without indentation`, `if has identical
   blocks`. None blocks v0.3 release; surfaced under
   `cargo clippy -- -D warnings` per Arc 5 Step 3. Cleanup is a
   tidying pass; no behavior changes.
8. **`AppContext` does not impl `Debug`**. Two test cleanup
   sites in v0.3 wanted `expect_err` on
   `Result<AppContext, _>` and had to fall back to `match`.
   Adding `#[derive(Debug)]` (most fields are `Arc<dyn …>` with
   Debug impls; mechanical) lets a small batch of test
   cleanups simplify.
9. **DID validator consolidation** (Arc 1 Step 4 finding). DID
   parsing logic is duplicated across handler and validation
   layers. A consolidation pass would make DID-shape changes a
   single-site edit.
10. **`AdminRoleManager::grant_role_in_tx` revoked-row UNIQUE
    bug** (Arc 1 finding). Granting a previously-revoked role
    can hit the UNIQUE constraint. Fix may require a small
    schema change (allow multiple rows distinguished by status)
    or an `INSERT ON CONFLICT` resolution path.
11. **`Display` impl for `Role`** (Arc 1 finding). Several
    `format!("{:?}", role)` sites would benefit from a proper
    `Display`. Cosmetic; affects log readability.
12. **`identity::cache::tests::test_stale_handle_detection`
    flaky under suite-wide load**. 1-second TTL race in
    `src/identity/cache.rs:516`. Passes in isolation but flakes
    intermittently in full-suite runs. Either widen the TTL,
    inject a test clock, or skip under high-parallelism load.

### Documentation / process refinements

13. **Snapshot test for `getRuntimeSetting`**. Per Arc 5 Step 0
    recon Q2(d), there is no formal snapshot test for the
    `getRuntimeSetting` response shape. The `SettingSource`
    typed enum's wire format is currently pinned only by an
    inline serialize-to-string assertion. A proper snapshot
    test would catch silent shape changes (e.g., a future
    refactor that drops the custom Serialize and emits
    serde-default `{"Runtime": null}` style).
14. **Subscribe parity test fix** (Arc 4 follow-up item). The
    `subscribeModEvents` ↔ `getAuditTrail` parity test had a
    tooling-side issue documented at Arc 4 cycle. v0.4 resumes
    the parity verification work.
15. **AURORA_ADMIN_UI_DESIGN.md prose audit**. Multiple sections
    were updated during Arc 4 (atomicity framing) and Arc 5
    (TimeRange, file-tier). A full audit pass for stale framing
    elsewhere is a v0.4 doc-cleanup candidate.
16. **CHANGELOG conventions formalization**. The v0.3 cycle
    accumulated many `### Changed` / `### Added` / `### Removed`
    blocks under `[Unreleased]` before the v0.3.0 release tag.
    Formalizing the convention (e.g., one block per category at
    most, with consolidations at release boundaries) would tidy
    future releases.

### Operator-experience refinements

17. **`validate_config.rs` audit beyond #155**. Arc 5 Step 1
    removed one misleading warning (`PDS_ADMIN_DIDS`-related).
    Other warnings in the file may be similarly stale or
    misleading post-#95. v0.4 audit pass.
18. **Operator runbook for first SuperAdmin bootstrap**.
    README's "First Admin User" section covers the SQL
    insertion path post-#155. A standalone runbook in
    `docs/operator/` would be more discoverable.
19. **`setRuntimeSetting` value-format documentation**. The
    `KNOWN_RUNTIME_KEYS` allowlist exists but the per-key
    value-shape documentation is split across the source
    `validate_runtime_value` function and the `set_runtime_setting`
    handler. A consolidated reference in
    `docs/operator/file-tier-config.md` (or a sibling doc)
    would help operators authoring `runtime.yaml`.

### Security / threat-model refinements

20. **OAuth state and DPoP nonces multi-instance**. v0.3 carried
    forward the per-process limitation. Multi-instance support
    requires either a Redis-shared store or a Postgres-CAS
    table. v0.4 candidate.
21. **Distributed rate limiting**. v0.3 carried forward the
    per-process rate limiter. Multi-instance support requires
    Redis or Postgres-CAS token bucket. v0.4 candidate.

---

## Aurora Admin UI v0.3 migration scope

Item 3 above ("Aurora Admin UI v0.3 migration") captures the
v0.3 → v0.4 UI work as a single architectural follow-up. The
migration spans wire-format absorbing (REQUIRED for v0.3
functionality), wire-format adopting (OPTIONAL feature
adoption), and CLI/UI parity gaps. Each is enumerated below
so v0.4 UI cycle planning has a complete inventory.

The Admin UI lives in a separate codebase. v0.3 deliberately
did not modify it (per Arc 4's scope decision); this section
is the handoff inventory.

### Wire-format absorbing (REQUIRED for v0.3 functionality)

The UI cannot continue to operate against v0.3 wire shapes
without absorbing these changes. These are the load-bearing
items for the migration:

- **`emitEvent` payload shape**: `subject:` (singular) →
  `subjects: [...]` array. Single-subject callers wrap in a
  one-element array (`subjects: [s]`). Known UI construction
  site: `static/admin/ActionPanel.js:534`. Audit
  `static/admin/` for additional construction sites — anywhere
  the UI builds an `emitEvent` body. The corresponding output
  field renames `snapshot_id: Option<String>` →
  `snapshots: Vec<SnapshotRef>` paired 1:1-by-index with
  `subjects` (empty when `snapshot_capture: false`).
- **`emitEvent` per-action `MAX_BATCH_SIZE` caps**:
  `DeleteAccount` = 10, `DeleteBlob` = 25, all other
  multi-subject-supported actions = 50. UI batch-action
  affordances must enforce these client-side OR surface the
  server's HTTP 400 rejection clearly. Client-side enforcement
  prevents the operator from constructing a too-large batch
  in the first place.
- **`emitEvent` embedded-id-variant length-1 enforcement**:
  `ResolveReport`, `DismissReport`, `ResolveAppeal`,
  `EscalateAppeal`, and `SendEmail` reject `subjects.len() > 1`
  with HTTP 400 `SubjectsArrayInvalidForAction`. UI must
  prevent or warn against constructing multi-subject calls
  for these actions.
- **Batch handler responses drop the `failures` field**:
  `BatchAccountsOutput`, `BatchLabelOutput`, and
  `BatchRemoveLabelOutput` no longer carry
  `failures: Vec<BatchFailure>`; the `BatchFailure` struct is
  retired. Any UI parser reading `response.failures` is now
  reading a missing field — typically silently undefined. UI
  partial-success rendering paths are dead code.
- **Per-subject error envelope on 4xx**: post-Arc-4,
  per-subject mutation failure surfaces in 4xx response
  bodies via `failingSubject` and `failingSubjectId` keys
  (NOT in the 200-response `failures` list, which is gone).
  UI error rendering needs to translate these to
  operator-friendly messages. Particularly important for
  batch handlers where the failing-subject context tells the
  operator which subject in their selection caused the abort.
- **`affected_count` semantic shift**: post-Arc-4,
  `affected_count` always equals input length on success
  (every batch handler is whole-tx-atomic; partial-success is
  no longer a state the caller can observe). UI partial-
  success rendering — "X of N applied" — is dead code; the
  remaining-count display is always 0 on success.
- **`SettingSource: "File"` value**: `getRuntimeSetting`
  responses can now return `source: "File"` for keys resolved
  from the file-tier YAML at startup. UI rendering the
  `source` field needs to handle the new value — likely
  visually identical to `"Default"` (informational, not
  actionable), distinguished from `"Runtime"` (operator-set
  via setRuntimeSetting) and `"RecoveryMode"` (env-var
  override active).
- **Three new HTTP 400 error codes from emitEvent's
  embedded-id validation**: `SubjectVariantMismatch`,
  `SubjectTargetMismatch`, `OrphanedAppeal`. UI error
  rendering needs user-friendly translations. Particularly
  important for the appeal-resolution UI — `SubjectTargetMismatch`
  surfaces when an operator passes the wrong subject for an
  embedded-id action (e.g., resolving Appeal #42 with
  `subjects: [Repo(some-other-did)]` instead of the
  appeal's recorded subject).
- **`SubjectUnion::RepoBlobRef` `record_uri` snake_case**
  (Arc 2 Step 0.5): the inbound `updateSubjectStatus` blob
  variant uses `record_uri` (snake_case) to byte-match
  `Subject::Blob`'s shape. UI calls to `updateSubjectStatus`
  with a Blob subject need to use the snake_case key. v0.2
  used `recordUri` (camelCase).
- **`grantRole` / `revokeRole` action-ID renames** (Arc 2
  Step 2): response field renames `audit_entry_id` →
  `auditEntryId`; on `grantRole` the embedded role record's
  wrapper field also renames `admin_role` → `adminRole`. UI
  consumers of the response need to read the new keys. (Per
  the Arc 2 Step 2 step-report, the only in-tree consumer
  was a unit test; admin UI invokes these endpoints but
  discards the response, so no UI-side coordination was
  needed at the time. Re-verify against current UI before
  v0.4 work.)

### Wire-format adopting (OPTIONAL; takes advantage of v0.3 features)

These are net-new feature surfaces. UI doesn't break without
adopting them, but adoption improves the operator experience:

- **`getAuditTrail.cascadeSnapshotIds`**: new per-entry wire
  field paired 1:1-by-index with `cascadeSubjects`. UI
  audit-trail detail views can surface cascade snapshot IDs
  alongside cascade subjects so operators can drill into the
  per-subject pre-decision state for batch chain entries.
- **`getAuditTrail` `subject_cid` filter**: new 7th filter
  (was 6). UI's audit-trail filter form can gain a CID input
  field for record/blob lookups by content hash.
- **`getAuditTrail.chainVerified` / `chainVerifiedThrough`**:
  per-row `verified` plus chain-level fields catch the
  consistent-rewrite attack (Arc 3 / chainlink #97). UI can
  surface a chain-integrity status indicator on the audit
  page header so operators get an at-a-glance signal.
- **`getModerationMetrics` canonical `timeRange` field**: UI
  metrics queries can offer preset names (`last_hour`,
  `last_24h`, `last_7d`, `last_30d`) as a dropdown alongside
  the existing custom-window picker. Legacy `start`/`end`
  peer fields continue to work for the custom-window case.
  Adopting the preset shape lets the UI label common windows
  consistently across operators.
- **Audit chain coverage on previously-audit-blind handlers**
  (Arc 3 / chainlink #97): `triggerPasswordReset`,
  `grantRole`, `revokeRole`, the six batch ops, plus seven
  others now write chain entries. UI can surface the
  resulting `auditEntryId` in success toasts/responses,
  letting operators jump to the chain-entry detail view from
  any successful action.

### CLI/UI parity gaps

Per the CLI/UI parity principle (see "Project patterns /
conventions" below), every v0.3 admin-tier CLI command should
have a UI counterpart unless it's bootstrap-class:

- **`grant-admin <did> <role> [--notes] [--force]`** (Arc 1
  Step 4). Runtime XRPC equivalent is
  `tools.aurora.superadmin.grantRole`. UI needs a SuperAdmin
  role-management page with grant + revoke buttons. The CLI
  command itself remains for the bootstrap case (when no
  authenticated SuperAdmin exists yet) but is not a
  steady-state operator path. The role-management UI surface
  should cover:
  - **View current role holders** across SuperAdmin, Admin,
    and Moderator tiers. Sourced from the `admin_role` table
    via a yet-to-be-designed `listRoles` endpoint OR by
    surfacing role-grant/revoke chain entries via
    `getAuditTrail` (action filter `role.grant` /
    `role.revoke`).
  - **Grant role** affordance — calls `grantRole`. Form
    fields: target DID, role tier, optional notes, optional
    force flag (for re-granting a previously-revoked role
    once the UNIQUE bug at item 10 is fixed).
  - **Revoke role** affordance — calls `revokeRole`.
    Confirmation modal with rationale text input.
  - **Audit-trail view of role grants/revokes** — uses
    existing `getAuditTrail` filtered by
    `action: "role.grant"` or `action: "role.revoke"`.
  - **CLI sentinel handling**: when surfacing audit entries
    in this view, distinguish `cli:`-prefixed `actor_did`
    values (CLI bootstrap operations) from PDS-originated
    grants. The `cli:` prefix is deliberately not a valid
    DID method (Arc 1 Step 4 / drift §Z.4 pattern 7) so
    `actor_did LIKE 'cli:%'` is the canonical filter.

### Documentation alignment (post-UI-migration cleanup)

These doc updates land AFTER the UI migration ships so the
narrative reflects steady-state rather than mixed v0.3/v0.4:

- **`docs/AURORA_ADMIN_UI_DESIGN.md` Phase 3.5** was updated
  in Arc 4 Step 4 to describe the target multi-subject
  `emitEvent` UI shape. The doc is forward-looking; once the
  UI ships, audit the doc for any remaining v0.2/v0.3
  transitional framing.
- **README "First Admin User" section** explains the CLI
  bootstrap path post-#155 (SQL insertion or `grant-admin`).
  After the role-management UI lands, README should note
  that subsequent grants flow through the UI; the
  CLI/SQL path stays documented as the bootstrap-class
  carve-out only.
- **CLI documentation**. If a separate CLI reference exists
  (or one is added in v0.4), `grant-admin` should be flagged
  as bootstrap-class, with a cross-reference to the steady-
  state UI surface.

### Migration scope sizing note

The wire-format-absorbing list alone (10+ touch points across
multiple UI surfaces) suggests the UI migration is at least
arc-sized for v0.4 — not a single-PR follow-up. Recommendation
for v0.4 cycle planning: scope a dedicated UI-migration arc
that pairs with backend-side observability (e.g., dual-shape
acceptance windows, deprecation logging on legacy field
reads) so the migration can land progressively rather than as
a single all-or-nothing flip.

---

## Project patterns / conventions established in v0.3

Conventions that emerged during v0.3 implementation and are
worth preserving as defaults for v0.4 work. These are
patterns, not hard rules — bootstrap-class carve-outs are
real and the conventions accommodate them rather than
handwave.

### CLI/UI parity for admin operations

**Principle**: every new admin-tier CLI command should pair
with a UI surface that exposes the same operation while
authenticated. Operators shouldn't drop to a terminal for
things the UI ought to be doing.

**Carve-out for bootstrap-class commands**: commands that
operate in a state where no admin token can exist (e.g.,
`grant-admin` for the first SuperAdmin, when the system has
zero authenticated administrators) stay CLI-only by
necessity. Direct SQL insertion into `admin_role` is the
ultimate bootstrap fallback; `grant-admin` is a controlled
mid-tier between SQL and the future UI. README documents
these as one-time bootstrap operations; UI takes over for
steady-state.

**Implementation guideline**: when designing a new CLI
command, the design step should explicitly answer: "what's
the UI counterpart, and is it built in the same cycle or
deferred?" Same-cycle pairing is preferred; cross-cycle
deferral is acceptable when the CLI command unblocks
operator work and UI cycle capacity is constrained, but the
UI counterpart MUST be tracked in `docs/v04-candidates.md`
(or the active cycle's candidate file) to prevent silent
permanent CLI-only state.

**Verification at cycle close**: cycle-close audit (Arc 5
§9.4.4-style audit sweep) should include a step that lists
admin-tier CLI commands without UI counterparts and confirms
each one is either bootstrap-class (documented carve-out) or
tracked as an open candidate.

### (Future patterns accumulate here)

As v0.4 and later cycles surface additional project-wide
conventions worth carrying forward, add them here. The
drift-audit section §Z.4 from each cycle's close is a
natural feeder.

---

## How to use this document

When v0.4 cycle planning starts:

1. Read this document end-to-end.
2. Promote items to chainlinks via `chainlink create` for the
   ones the v0.4 cycle will scope.
3. Items not promoted stay in this document for the next
   cycle's planning.
4. Closed items (work shipped in v0.4) get crossed off here
   and tracked in the v0.4 release CHANGELOG.

This document is informational; the canonical chainlink
tracker remains the project's `chainlink` CLI.
