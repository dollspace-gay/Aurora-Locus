# Arc 6 Phase B exercise script

Consolidated verification script for skydeval's Phase B sweep
against `pds.goddess.systems`. Aggregates the per-step
verification sections from `/tmp/arc6_step1_report.md` through
`/tmp/arc6_step7_report.md` into a single coherent run-through.

## Prerequisites

- Running bsky-PDS at `pds.goddess.systems` (TypeScript
  deployment) with Aurora-Locus admin UI served from
  `static/admin/`.
- Branch `skydeval/v0.4-cycle` at the Step 8 tip (post-audit).
- Staged data items called out per-exercise. The "requires
  staged data" annotations indicate exercises that cannot be
  driven by happy-path operator workflows alone.
- Node ≥ 18 on the local dev machine for running the UI test
  suite (per [`docs/operator/running-ui-tests.md`](../operator/running-ui-tests.md)).
- A SuperAdmin role grant to drive the role-management
  affordances.

---

## Section A — Error-translation infrastructure (Step 1)

A.1. **Translated error path**: trigger a 4xx response for each
seed error code; confirm the rendered message is the operator-
friendly template, not `HTTP 400: <raw>`.

- `SubjectVariantMismatch`: emitEvent with an account subject
  targeted at a record-action like `TakedownRecord`. **Ready
  to exercise** (any account DID in the moderation queue).
- `SubjectTargetMismatch`: resolveAppeal with an embedded ID
  that doesn't match the appeal's actual subject. **Ready to
  exercise** if there are any live appeals; otherwise
  **requires staged data: a fabricated appeal subject**.
- `OrphanedAppeal`: resolveAppeal against an appeal whose
  subject has been deleted server-side. **Requires staged
  data: an appeal whose subject was deleted after the appeal
  landed**.
- `SubjectsArrayInvalidForAction`: emitEvent with
  `subjects: [a, b]` for an embedded-id action like
  `ResolveReport`. **Ready to exercise** (any two account
  DIDs + a non-existent report ID will produce the
  validation error).

A.2. **Untranslated 4xx fallback**: trigger 401 by sending a
request with an expired or malformed Bearer token; trigger
403 by sending an Admin-required request as a Moderator.
Confirm the rendered error is `HTTP 401: …` or `HTTP 403: …`
(legacy format preserved). **Ready to exercise**.

A.3. **Per-subject envelope**: emitEvent with a batch of two
subjects where one is invalid (e.g., one valid account DID
and one malformed string); confirm the rendered error appends
` (subject: <bad>, id: <i>)`. **Ready to exercise**.

A.4. **Module-absent defensive path**: temporarily comment out
the `error-translations.js` `<script>` tag in `index.html`;
reload; trigger any 4xx; confirm rendering falls back to
`HTTP <status>: <message>` without throwing in the browser
console. Restore the script tag. **Ready to exercise**.

A.5. **Successful-response regression check**: exercise any
2xx-returning endpoint (e.g., a Dashboard render that calls
`getQueueStats`); confirm normal operation. **Ready to
exercise**.

---

## Section B — Wire-format absorbing (Step 2)

B.1. **ActionPanel emitEvent (multi-subject shape)**: pick any
single-subject moderation action (e.g., ApplyLabel from
AccountDetail's action panel); submit. Server should accept
without a 400. A regression to the v0.2 wire shape would
surface as `SubjectsArrayInvalidForAction` via Step 1's
error-translation. **Ready to exercise**.

B.2. **BulkActionPanel limits (per-action MAX_BATCH_SIZE)**: open
the Accounts page bulk action panel; bulk-select 51 accounts
and try "Takedown selected accounts". UI should disable
confirm at 51 with "Too many subjects (max 50)". For
DeleteAccount/DeleteBlob the cap is 10/25, but those actions
aren't in BATCH_ACTIONS — the cap-check only fires at 50 for
all current bulk actions. **Ready to exercise** (current
50-cap path).

B.3. **`affected_count` rendering (atomic-batch result shape)**:
submit a successful bulk action; confirm the status region
reads "Processed N subject(s)" without any "skipped" suffix.
Submit a bulk action that 4xx's; confirm the per-subject
envelope rendering still surfaces the failing subject inline.
**Ready to exercise**.

B.4. **SettingSource parity** — five sub-cases:

- Set `moderation-mode` via SettingsUiModes save; reload the
  page; the "Current:" line shows e.g. "reduced" with no
  suffix (Runtime tier). **Ready to exercise**.
- Configure a file-tier `moderation-mode: reduced` in
  `<data>/runtime.yaml` and restart the PDS; reload; the
  current-value line shows "reduced (file)". **Requires
  staged data: a file-tier runtime.yaml entry**.
- Without a runtime row and without a file-tier entry,
  reload; the line shows "full (default)". **Requires staged
  state: clear the runtime_settings table for the key**.
- With `AURORA_RECOVERY_MODE` env var set, restart; reload;
  the line shows e.g. "full (recovery override)". **Requires
  staged state: env var + restart**.
- SettingsGeneral fields: load the page; each form-group
  label shows a `(default)` suffix for keys that aren't in
  `KNOWN_RUNTIME_KEYS` and have no runtime row. **Ready to
  exercise**.

---

## Section C — Wire-format adopting features (Step 3)

C.1. **`cascadeSnapshotIds` display** (sub-3a): trigger a
multi-subject batch emitEvent (e.g., BatchTakedownAccounts
across 2 accounts) so an audit entry with non-empty
`cascadeSubjects` lands; navigate to that entry's detail
page; confirm the `Cascade subjects (N)` section renders as
a list with subject click-throughs and snapshot ids inline.
Trigger a single-subject action; confirm the section is
absent. **Ready to exercise**.

C.2. **`.settings-source-tag` CSS** (sub-3a carryover): open
SettingsGeneral or SettingsUiModes; confirm source suffixes
(e.g., `(default)`, `(file)`) render in italic muted text
inline with the label. **Ready to exercise**.

C.3. **`subject_cid` filter** (sub-3b): enter a valid blob CID
into the "Filter by subject CID" input on the Audit page;
confirm results narrow to entries with that CID. Enter a
malformed CID; confirm a 4xx error rendering appears (legacy
`HTTP <status>: <message>` shape since the validation code
isn't in Step 1's seed translation table). **Ready to
exercise**.

C.4. **chainVerified indicator — green path** (sub-3c): load
the Audit page against a healthy chain; confirm the green
"Chain verified through entry M" indicator with M matching
the head sequence. Click the indicator; the detail panel
expands inline explaining whole-chain verification. **Ready
to exercise**.

C.5. **chainVerified indicator — yellow path**: **requires
staged data: corrupt the `prev_hash` value of an entry
mid-chain to force `chainVerified === false` with
`chainVerifiedThrough > 0`**. Reload; confirm yellow ⚠
indicator with `chainVerifiedThrough = M` and "failure at
entry M+1" label. The detail panel includes the
`aurora-locus debug verify-audit-chain` code block as a
diagnostic suggestion. Restore the corrupted row after
testing.

C.6. **chainVerified indicator — red path**: **requires staged
data: corrupt the first entry's hash to force
`chainVerified === false` with `chainVerifiedThrough === 0`**.
Confirm red ✗ indicator with the "failed at the first entry"
label. Restore after testing.

C.7. **`timeRange` preset dropdown** (sub-3d): switch the
moderator dashboard to each of the four presets (`last_hour`,
`last_24h`, `last_7d`, `last_30d`); confirm the metrics
table re-renders for each. Granularity changes hour → day
at the `last_7d` boundary; visually obvious in the bucket
count. **Ready to exercise**.

C.8. **Toast click-throughs — emitEvent** (sub-3e): from
AccountDetail or RecordDetail, dispatch any single-subject
moderation action (e.g., ApplyLabel); confirm the success
toast shows "View audit entry" link. Click; confirm
navigation to `#mod/audit/<id>` and the detail view loads
correctly. **Ready to exercise**.

C.9. **Toast click-throughs — batch endpoints**: from Accounts
page, bulk-select 2+ accounts and dispatch a batch action;
confirm "Processed N subjects." toast with click-through.
**Ready to exercise**.

C.10. **Toast click-throughs — triggerPasswordReset**:
AccountDetail → "Send password reset"; confirm toast surfaces
the audit-entry click-through. **Ready to exercise**.

C.11. **Toast click-throughs — exportAccountForensic**:
AccountDetail → "Generate forensic export"; confirm the toast
shows bundle hash inline and audit-entry id as action link.
**Ready to exercise**.

C.12. **Toast click-throughs — setRuntimeSetting**:
SettingsGeneral save any card OR SettingsUiModes save
moderation mode; confirm toast shows audit-entry click-
through pointing at the last write's entry. **Ready to
exercise**.

C.13. **Toast click-throughs — revokeRole**:
SettingsRolesMembers → revoke any non-self role grant;
confirm toast surfaces click-through. **Ready to exercise**
(requires a non-self role grant to exist; staged data if no
second admin exists).

---

## Section D — Modal consolidation (Step 4)

D.1. **Substrate sanity-check**: open any newly-migrated modal
(e.g., BlobOps Run GC); confirm focus trap (Tab cycles
within modal), Escape dismisses, Enter submits when valid,
focus returns to triggering element on close. **Ready to
exercise**.

D.2. **AccountDetail overridePassword**: AccountDetail →
"Override password" → modal with password + rationale
fields. Submit disabled until both non-empty. On submit →
toast (with audit-entry click-through if the endpoint
surfaces one). **Ready to exercise**.

D.3. **AccountDetail updateSigningKey**: same shape with DID-
key text + rationale. **Ready to exercise**.

D.4. **AccountDetail deleteAccount**: typed-confirm gate (must
type the exact handle), rationale required, ack checkbox
required. Submit stays disabled until all three satisfied.
**Ready to exercise** (requires a non-self account to
delete; staged data if no test account available).

D.5. **SettingsRolesMembers revoke**: typed gate 'REVOKE',
required rationale. Toast surfaces Step 3 click-through.
**Ready to exercise** (requires an existing role grant to
revoke).

D.6. **InviteDetail / Invites disable (single + bulk)**: simple
destructive confirm. **Ready to exercise**.

D.7. **SettingsUiModes mode switch**: simple destructive
confirm; rationale collected separately above the radio
buttons. **Ready to exercise**.

D.8. **BlobOps GC**: zero-field modal. **Ready to exercise**.

D.9. **AccountDetail toggleInvites**: select for
Enabled/Disabled + rationale. Confirm the select-based UX
replaces the prior inverted-OK-Cancel. **Ready to exercise**.

D.10. **RateLimits cleanup**: zero-field modal. **Ready to
exercise**.

D.11. **SystemHealth nonce cleanup**: zero-field modal. **Ready
to exercise**.

D.12. **Sequencer doAction**: any sequencer op shows the
destructive-confirm modal with the heading reading the op's
prompt. **Ready to exercise**.

D.13. **Chain-indicator detail panel** (not migrated): confirm
Step 3 inline-expansion behavior unchanged. **Ready to
exercise**.

D.14. **Regression check**: every migrated site that ends in a
wire call still exercises Step 3's auditEntryId click-
through path. Confirm the toasts still surface and link
correctly after Step 4's modal-driven flows replace the
prior native confirms/prompts. **Ready to exercise**.

---

## Section E — Role-management UI (Step 5)

E.1. **Grant flow — happy path**: SettingsRoles → "Grant role"
on any tier → fill in a valid DID + rationale → submit.
Confirm: success toast with audit-entry click-through;
SettingsRoles refreshes showing the new grant. **Requires
staged data**: a DID that doesn't already have the tier.

E.2. **Grant flow — invalid DID (client-side)**: submit with
`foo` as DID. Confirm: form-error inline ("DID must start
with 'did:'…"); submit disabled; no wire call made. **Ready
to exercise**.

E.3. **Grant flow — invalid DID (server-side)**: submit with a
syntactically-valid-looking DID that's not registered
(e.g., `did:plc:nonexistent`). Confirm: server 4xx surfaces
— for grant_role the backend returns plain-text errors, so
the rendering falls back to `'HTTP <status>'` (no structured
translation). Documented as a known gap; not a Step 5
regression. **Ready to exercise**.

E.4. **Grant flow — already-granted**: submit a grant for a
DID that already holds the tier. Confirm: server's error
surfaces (plain-text). **Requires staged data**: a DID with
an existing tier grant.

E.5. **Grant flow — force flag**: not applicable — the form
has no `force` field per Step 5 Decision 2 (backend
contract doesn't accept it). **Skip this exercise.**

E.6. **Revoke flow (integrity check)**: SettingsRolesMembers →
"Revoke" on any row → typed gate "REVOKE" → rationale →
submit. Confirm: success toast with click-through; member
removed from list. **Ready to exercise**.

E.7. **Audit-trail dual links**: N/A — deferred per Step 5.
Confirm absence of links is intentional, not a regression.
**Ready to confirm.**

---

## Section F — CLI sentinel handling (Step 6)

F.1. **Audit list sentinel rendering**: with a `cli:`-prefixed
audit entry present, confirm the Audit page's actor column
renders as `CLI: <suffix>` badge — non-clickable `<span>`,
muted-bordered chip styling, no underline on hover.
**Requires staged data: an audit entry with `actor_did`
starting `cli:`** (e.g., a debug subcommand invocation, or a
manual SQL insert into `audit_chain_entry`).

F.2. **Audit detail sentinel rendering**: navigate to the same
entry's detail page (e.g., via Step 3's toast click-through
or via the audit list row's audit-entry link). Confirm the
"Actor DID" field renders the badge. **Requires staged
data** (same entry as above).

F.3. **Events list + live-prepend sentinel**: if any moderation
event has a CLI-source actor, confirm the events page
renders the badge in both the table and the live-prepend
prepended row. **Requires staged data**: a moderation event
emitted by a CLI tool.

F.4. **Event detail sentinel**: navigate to that event; confirm
the "Actor" field renders the badge. **Requires staged
data** (same).

F.5. **AccountDetail recent-actions sentinel**: if the account
has any audit entries from a CLI actor, the "by <actor>"
rendering in the recent-actions list should show the badge.
**Requires staged data**.

F.6. **Runtime DID regression check**: a runtime-PDS account
still renders as a clickable `<a class="entity-ref">` link
to `#ops/accounts/...`. No regression in any of the seven
surfaces. **Ready to exercise**.

F.7. **Defensive edge cases**:

- `cli:` (empty suffix) renders as `CLI: ` with empty suffix
  text. **Ready to exercise** with manual fixture.
- `cli:<x>script>` (HTML-injection attempt) renders as
  `CLI: <x>script>` with the angle brackets HTML-escaped.
  **Ready to exercise** with manual fixture.
- `null` / `undefined` / non-string DIDs fall through to the
  existing em-dash rendering. **Ready to exercise**.

---

## Section G — Backend observability (Step 7)

G.1. **emitEvent canonical shape**: POST with
`{"action": {"kind":"TakedownAccount"}, "subjects":[{...}], "rationale":"..."}`
→ 200 success; counter unchanged. **Ready to exercise**.

G.2. **emitEvent legacy shape**: POST with
`{"action":{...}, "subject":{...}, "rationale":"..."}`
→ 200 success (normalized internally); counter incremented
with labels `(tools.aurora.admin.emitEvent,
v0.2_single_subject, subject)`; structured
`legacy_wire_shape_ingested` info log emitted. **Ready to
exercise**.

G.3. **emitEvent ambiguous**: POST with both fields → 400 with
"pick exactly one shape per request". **Ready to exercise**.

G.4. **emitEvent empty**: POST with neither → 400 with
"requires either canonical or legacy". **Ready to
exercise**.

G.5. **updateSubjectStatus canonical shape**: POST with
`RepoBlobRef` using `record_uri` → 200; counter unchanged.
**Ready to exercise**.

G.6. **updateSubjectStatus legacy shape**: POST with
`RepoBlobRef` using `recordUri` → 200; counter incremented
with labels `(com.atproto.admin.updateSubjectStatus,
v0.2_camelCase_record_uri, recordUri)`. **Ready to
exercise**.

G.7. **updateSubjectStatus both shapes**: POST with both
`record_uri` and `recordUri` → 400 "not both". **Ready to
exercise**.

G.8. **Sunset header configuration**: with
`PDS_V03_WIRE_SUNSET_DATE` unset OR set, no `Sunset` header
appears on responses today (response headers are not emitted
in v0.4 per Step 7 Decision 1 + operator doc). **N/A until
headers ship in v0.5+** — confirm absence is intentional.

G.9. **Counter visibility**: scrape `/metrics`; confirm
`aurora_legacy_wire_ingest_total` is exposed with the
expected labels (empty when no legacy traffic has hit).
**Ready to exercise**.

G.10. **UI regression check**: every Step 2-shipped UI flow
(emitEvent via ActionPanel, etc.) continues to work
canonically. The UI sends canonical shapes only; no counter
increments expected from UI-driven traffic. **Ready to
exercise**.

---

## Section H — Test suite verification

H.1. **Rust lib tests**:

```
cargo test --lib
```

Expected: 864 passing, 1 pre-existing flake
(`identity::cache::tests::test_stale_handle_detection` —
fails under parallelism, passes in isolation; documented as
non-blocking in Step 7 report). Confirm by running:

```
cargo test --lib identity::cache::tests::test_stale_handle_detection
```

— expect this to pass in isolation.

H.2. **UI tests**:

```
node static/admin/scripts/api/__tests__/endpoints.test.js
node static/admin/scripts/api/__tests__/capabilities.test.js
```

Expected: 4 + 8 = 12 passing tests. Per
[`docs/operator/running-ui-tests.md`](../operator/running-ui-tests.md)
the directory-discovery form (`node --test <dir>`) does not
work; run per-file as shown.

---

## Section I — Decoupling sweep (verify clean)

I.1. Run:

```
git grep -i "cairn\|hideaway\|horizon\|pursuingpeace\|nearhorizon"
```

Expected: zero real hits. False positives (the word
"horizontal" / "horizons" / "Horizon" as part of a sentence,
self-references inside V03_DESIGN.md and V04_DESIGN.md
listing the forbidden tokens, plus
AURORA_ADMIN_UI_DESIGN.md §11.7 listing them as verification
criteria) are acceptable. Confirm any hits are false-positive
matches against allowed tokens.

I.2. Run:

```
grep -ri "cairn\|hideaway\|horizon\|pursuingpeace\|nearhorizon" docs/
```

Expected: only the same false positives plus the V0x_DESIGN.md
self-references.

---

## Section J — JWT-deprecation middleware (Step 8 wiring)

J.1. **JWT request emits Deprecation headers**: send a request
to any admin endpoint with a JWT-shaped Bearer token (three
base64url segments separated by `.`). Confirm the response
includes `Deprecation: true`, `Sunset: <date from config>`,
`Warning: 299 - "JWT authentication is deprecated…"`, and
`X-OAuth-Migration-Guide: <url from config>`. **Ready to
exercise** (any valid JWT token will trigger; opaque OAuth
tokens won't).

J.2. **JWT request increments counter**: after J.1, scrape
`/metrics`; confirm `jwt_deprecation_warnings_total` has
incremented. **Ready to exercise**.

J.3. **OAuth request does NOT emit headers**: send a request
with an OAuth opaque bearer token (no dots in the token
format). Confirm no Deprecation header. **Ready to exercise**.

J.4. **No-token request unchanged**: send an unauthenticated
request (e.g., to a public endpoint). Confirm no headers
added, no counter increment. **Ready to exercise**.

---

## Sign-off

Once all sections clear:

1. Document any new findings or regressions in a Phase B
   addendum (separate from this script).
2. If clean, the v0.4 within-doll's-repo PR is ready to open.
3. Tag and release after the PR merges (skydeval's call).
