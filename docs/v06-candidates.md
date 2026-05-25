# Aurora-Locus v0.6 candidates

v0.6 inherits everything from the v0.4 → v0.5 sort that doesn't
share scope with federation (the locked v0.5 headline; see
[`docs/v05-candidates.md`](v05-candidates.md)).

This file is the running accumulator for v0.6+. Items land here
as they surface during v0.5; v0.6's design phase decides which
become arcs.

## Carryovers from Arc 6 (v0.4 cycle)

### `api/__tests__/` — add unit tests for new Arc 6 modules

`AuroraErrorTranslations`, `AuroraModal.form`,
`AuroraModal.destructiveConfirm`, the chain-indicator helper,
the cascade-section renderer in `AuditEntryDetail.js`, and the
`AuroraToast` `opts.action` path don't have unit tests today.
The pattern from the existing `endpoints.test.js` and
`capabilities.test.js` (Node `node:test` core module, no
external framework) extends naturally.

### `SettingsGeneral` form-label `for` attribute a11y audit

Carryover from Arc 6 Step 2 Open question 2. The form-group
labels in `SettingsGeneral.js` don't have `for` attributes
pointing at their inputs. Pre-existing a11y gap; Arc 6
preserved.

### `KNOWN_RUNTIME_KEYS` coverage audit against SettingsGeneral keys

Carryover from Arc 6 Step 2 Out-of-scope flag #3. Keys like
`general.instance-name`, `general.service-url`, etc., are used
by `SettingsGeneral.js`'s `loadValues()` but may not be in the
backend's `KNOWN_RUNTIME_KEYS` allowlist
(`src/api/aurora_admin.rs:3607-3610`). Need to either add them
to the allowlist or remove the dead UI rows.

### Snapshot detail-page route + `AuroraEntityRef.snapshot` helper

Carryover from Arc 6 Step 3 Open question 1. The
`cascadeSnapshotIds` rendering in `AuditEntryDetail.js`
currently renders snapshot ids as plain `<code>` because
there's no detail-page route for snapshots. A future cycle
could add `#mod/snapshots/<id>` and a corresponding
`AuroraEntityRef.snapshot(id)` helper; the cascade rendering
becomes a one-line change at that point.

### Dashboard moderation-metrics custom-range time-window picker

Carryover from Arc 6 Step 3 Decision 5 + Open question 2. The
preset dropdown ships without a "Custom range" option because
the pre-Arc-6 Dashboard had no custom-window picker. Adding
one is substantive new UI.

### Extend `AuroraModal.form` to accept Node | string body

Carryover from Arc 6 Step 4. Required for migrating the
chain-indicator detail panel (currently inline expansion) to
a modal surface — the detail panel has HTML content (paragraphs
+ code blocks) that `form`'s string-body path can't render.

### AccountDetail legacy helpers refactor

Carryover from Arc 6 Step 4 Out-of-scope flag #2.
`promptRationale` and `promptRationaleAndConfirmation` are
AuroraModal-backed but were not refactored to use the new
`form` / `destructiveConfirm` helpers because they aren't
native-prompt sites. Three remaining consumers
(`doPasswordReset`, `updateEmail`, `updateHandle`) could
migrate.

### SettingsUiModes mode-switch flow modal-vs-page rationale

Carryover from Arc 6 Step 4 Open question 3. The
`destructiveConfirm` modal for mode-switch sits alongside a
page-level rationale textarea (above the radio buttons). A
unified flow that collects rationale inside the modal would
be cleaner but is a larger UX restructure.

### Router enhancement + dual audit-trail links on SettingsRoles

Carryover from Arc 6 Step 5 (deferred per Open question 1).
The dual-link UX from V04_DESIGN §5.3.2 option (c) requires
either extending `Audit.js` to parse hash query params on
mount, or extending the router to surface query params in
route-match results. Both are small substrate additions;
the design decides which shape.

### CLI sentinel tooltip / disambiguation

Carryover from Arc 6 Step 6 Open question 1. Operator request-
gated: if operators ask to distinguish between specific CLI
tools (e.g., `cli:bootstrap` vs `cli:gc-sweep` carry different
operational context), add a `title` attribute or hover tooltip.

### CLI sentinel prefix-to-label map refactor

Carryover from Arc 6 Step 6 Open question 2. If the backend
ever emits other sentinel prefixes (`sys:`, `auto:`, etc.), the
helper would need a prefix → label map. Currently the helper
hardcodes `cli:` → `CLI: `.

### `record()` / `blob()` / `event()` sentinel handling

Carryover from Arc 6 Step 6 Out-of-scope flag #3. The sentinel
pattern was applied only to `account()`. Records, blobs, and
events aren't typically CLI-emitted, but if a use case
surfaces, the pattern transfers in one line per helper.

### Clippy / `must_use` warning sweep on test code

Carryover from Arc 6 Step 7 Out-of-scope flag #6. 16 pre-
existing `must_use` warnings on test code that ignores
`emit_event`'s return value. Mechanical cleanup; not urgent.

### `identity::cache::tests::test_stale_handle_detection` flake

Carryover from Arc 6 Step 7 Out-of-scope flag #4. Time-
sensitive test that passes in isolation but fails under
full-suite parallelism. Pre-existing; flagged for the v04-
candidates accumulator originally (item 12) and carried
forward.

## Carryovers from v0.5 cycle

### Automated Phase B harness substrate

Per Arc 16a Step 0 r0 finding 1 (May 2026). v0.5 ships with
per-arc operator-driven markdown per V05_DESIGN.md §4.10. A
unified harness — `make phase-b` target, scripted
multi-backend execution, optional CI integration — is
deferred to v0.6+. Shape proposals to be aggregated from
arc-specific operational feedback during v0.5 implementation.

### proto-blue 0.3.2 review

Per Arc 16a Step 0.1 r0 finding (May 2026). proto-blue 0.3.2
published 2026-05-14. Arc 16a pins 0.3.1 via
`--precise 0.3.1`; future `cargo update` would accept 0.3.2
per `^0.3.1` caret semantics. v0.6+ cycle reviews 0.3.2
changelog for opt-in surface changes.

## Out of scope for v0.6 (cycle planning will decide)

Items below would be candidates for v0.7+ if v0.6 doesn't pick
them up:

### Wholesale doc reorganization of AURORA_ADMIN_UI_DESIGN.md

The v0.2 prose remains intact; Arc 6 Step 8 added §15 additively.
A future cycle could merge the §15 additions into the
appropriate v0.2 sections and restructure the doc as a
"unified v0.4 reference" rather than "v0.2 + v0.4 amendments."

### CI integration for UI tests

Carryover from Arc 6 Step 5 Open question 3. Low-cost Node
step in CI; flagged for v0.5 cycle-close audit decision.
