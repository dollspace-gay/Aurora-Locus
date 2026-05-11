# Aurora-Locus v0.5 candidates

## Headline (locked): Federation

v0.5's central work is **federation**. This is not a candidate;
it is the locked headline per [`docs/V04_DESIGN.md`](V04_DESIGN.md)
§1.3 and the v0.4-cycle-close commitment to prevent the
v0.5→v0.6→… deferral pattern.

### Federation arc skeleton

v0.5's design phase produces a fuller skeleton; this section
pre-commits the work surfaces so v0.5 starts with structure
rather than re-discovering scope.

- **Entryway** — account routing across PDSes. The entryway
  decides which PDS hosts a given account and proxies requests
  accordingly.
- **Dynamic lexicon loading** — federated PDSes may use
  different lexicons (extensions, vendor-specific records).
  Aurora-Locus loads lexicons per-source rather than assuming
  a single global lexicon set.
- **Sequencer event types** — federated event ingestion with
  per-type validation. The sequencer accepts events from
  upstream PDSes, validates against the source's declared
  lexicon set, and either landed-and-published or
  rejected-with-reason per event.
- **PLC ops** — DID document operations from the PLC directory.
  Aurora-Locus subscribes to PLC ops for accounts it cares
  about; updates local DID-document state on operation
  landings.
- **CAR verification** — signed CAR file ingestion. Federated
  events arrive as CAR files; verify signatures against the
  emitting account's signing key (which Aurora-Locus has from
  PLC ops).
- **Blob quarantine** — incoming blobs from federated PDSes
  start quarantined; moderation gates the release. Quarantine
  state is a separate axis from the moderation-event
  takedown state.
- **Event size limits** — per-event payload caps to prevent
  resource exhaustion. Limits are configurable per-deployment;
  defaults are conservative.
- **Error codes** — federated-event-specific error envelope.
  Builds on the structured-error-code work from v0.3 (the four
  codes in `AuroraErrorTranslations`).
- **Time-based cursors** — sequencer cursors that support
  time-ordered queries (vs. sequence-id-only). Required for
  federation back-fill where an upstream PDS sends events
  out-of-sequence-order and Aurora-Locus reconciles by
  timestamp.

## Federation-aligned candidates

Smaller workstreams that fit alongside federation in v0.5
because they share scope, infrastructure, or operator-facing
surface area.

### Wire `emit_legacy_wire_headers()` to handler response paths

Carryover from [Arc 6 Step 7's report](../).

The `emit_legacy_wire_headers(response, endpoint, shape, fields)`
helper exists at `src/api/middleware.rs` and is wired-ready, but
no dual-shape handler calls it today. Wiring requires
restructuring the handler return type from
`Json<EmitEventOutput>` to `Response` (or introducing a wrapper
extractor pattern), which ripples through ~43 pre-existing test
call sites across `emit_event` and `update_subject_status`.

The work is federation-aligned because federated PDS consumers
of `emitEvent` and `updateSubjectStatus` benefit from the
client-side deprecation signal in `Deprecation` / `Sunset` /
`Warning` / `X-Wire-Migration-Guide` response headers. Without
these, federated clients have to poll Aurora-Locus's Prometheus
endpoint or read tracing logs to know they're on a deprecated
shape.

`PDS_V03_WIRE_SUNSET_DATE` env var becomes operational once the
headers are wired.

### Sunset-header date format validation

Carryover. Folds into the above.

The current helper accepts any `HeaderValue`-parseable string
for the `Sunset` header. RFC 7231 IMF-fixdate is the standard
format. Add validation either in the helper or at env-var parse
time during AppContext construction.

### Backend error shape for `grant_role` / `revoke_role`

Carryover from Arc 6 Step 5's report (Open question 2).

The handlers in `src/api/admin.rs` return errors as
`(StatusCode, String)` — plain text, not structured JSON. The
Arc 6 Step 1 error-translation layer can't match plain-text
bodies (it parses `body.error` and `body.message`). Reshaping
these handlers to return structured JSON unlocks translated
error messages for role-management 4xx responses.

Borderline candidate for v0.5: lands here if federation work
introduces structured-error wire conventions where this
plumbing transfers cleanly; otherwise carries to v0.6.

### Recon-process improvements (meta)

Carryover from cycle observations across Arc 6 Steps 0–8.

Three Step-0 recon patterns to formalize before v0.5's
federation Step 0 runs:

- **Assume-vs-verify-vs-find phrasing.** Step-0 questions
  should distinguish "verify whether X is true" (read code,
  produce hypothesis) from "find X" (locate a thing) from
  "assume X and proceed unless something contradicts." The
  Arc 6 Q14 enumeration of toast-emitting endpoints was
  framed as "find" but the recon assumed call sites existed;
  several didn't (Step 3 sub-3e discovered ActionPanel /
  BulkActionPanel had no toast emission, requiring substrate
  additions).
- **Kickoff form shapes should cite the backend struct
  verbatim.** Arc 6 Step 5's kickoff suggested `force` and
  `notes` fields on the grant-role form; the backend's
  `GrantRoleRequest` accepts only `{ did, role, rationale }`.
  Future kickoffs should quote the backend struct's
  `#[derive(Deserialize)]` block to avoid this drift.
- **"Display-only" vs "legacy-pattern UI" distinction.** Arc 6
  Step 0 Q14 characterized `SettingsRoles.js` as "display-
  only" (no grant action UI). It actually had grant UI, just
  built on the legacy `AuroraModal.open` + manual DOM pattern
  rather than the Step-4-introduced helpers. The Step 5
  kickoff inherited the "build from scratch" framing and
  needed clarification mid-step.

## Out-of-scope for v0.5

Everything else from the v0.4 → v0.5 sort that doesn't share
scope with federation goes to v0.6 per the federation-protective
sort. See [`docs/v06-candidates.md`](v06-candidates.md).
