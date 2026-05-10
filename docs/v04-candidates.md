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
3. **Aurora Admin UI migration for new wire shapes**. Arc 4's
   `emitEvent` reshape (`subject` → `subjects`) and the dropped
   `failures` field on batch handlers require coordinated UI
   changes. The Admin UI is a separate codebase per Arc 4's
   scope decision; the migration is tracked separately and is
   v0.4 work.
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
