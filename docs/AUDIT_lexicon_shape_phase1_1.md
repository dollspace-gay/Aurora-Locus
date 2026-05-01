# Phase 1.1 — Lexicon-Shape Audit (chainlink #56)

**Scope:** Eleven `com.atproto.admin.*` endpoints that Aurora-Locus already
implements at name-parity with bsky-PDS, audited for **shape** parity per
ADMIN_MODERATION_ASSESSMENT.md §3.6.

**Reference lexicons:** `/mnt/d/- - CODING/RUST/atproto/lexicons/com/atproto/admin/`
(atproto monorepo, current main, ~2026-Q2). The assessment doc compares
against a "2025-Q1" snapshot; for the eleven stable endpoints in this list,
no upstream drift was observed between those eras during this audit (no
new required fields, no removed fields, no type changes in the reference).

**Aurora-Locus surface:** lexicons are not stored as JSON; request/response
shapes are inline in Rust handlers in [src/api/admin.rs](src/api/admin.rs).
Routes are registered in [src/api/admin.rs:14-227](src/api/admin.rs#L14-L227).
Each endpoint below cites the relevant request/response struct line range.

**Verdict legend:**
- **Clean** — every input/output field matches the lexicon in name, type, and required-ness; no extra fields beyond the spec.
- **Minor drift** — extra response payload on a procedure that has no declared output, or one isolated additive field. Wire-level breakage is unlikely; clients ignoring extras still work.
- **Major drift** — at least one of: input field renamed, required/optional flipped on a declared field, declared input parameter missing, declared output field replaced, fundamentally different request body shape.
- **Not found** — Aurora-Locus has no implementation despite being in the parity list. (None observed in this audit.)

---

## Per-endpoint findings

### deleteAccount

**Aurora-Locus location:** [src/api/admin.rs:715-751](src/api/admin.rs#L715-L751) (`DeleteAccountRequest`, `admin_delete_account`)

| Dimension | Aurora-Locus | bsky-PDS reference | Match? |
|---|---|---|---|
| Type | procedure | procedure | ✓ |
| Required input | `did` (string) | `did` (string, format=did) | ✓ (format unenforced) |
| Optional input | (none) | (none) | ✓ |
| Output | `{success: bool, did: string, message: string}` | (no output schema) | ✗ extra payload |
| Errors declared | (none in lexicon-form) | (none) | ✓ |

**Verdict:** Minor drift
**Notes:** Lexicon declares no output; Aurora returns a non-empty JSON body. Clients that ignore the body still work. Cosmetic but worth normalising during Phase 2 cleanup. DID format is validated via `starts_with("did:")` rather than against the formal DID regex — acceptable, but lexicon-grade format validation would be stricter.

---

### disableAccountInvites

**Aurora-Locus location:** [src/api/admin.rs:1830-1834](src/api/admin.rs#L1830-L1834) (`AccountInvitesRequest`), [src/api/admin.rs:1869-1898](src/api/admin.rs#L1869-L1898) (`disable_account_invites`)

| Dimension | Aurora-Locus | bsky-PDS reference | Match? |
|---|---|---|---|
| Type | procedure | procedure | ✓ |
| Required input | `did` (string) | `account` (string, format=did) | ✗ field renamed |
| Optional input | (none) | `note` (string) | ✗ missing |
| Output | `{success: bool, did: string, invitesEnabled: false}` | (no output schema) | ✗ extra payload |
| Errors declared | (none) | (none) | ✓ |

**Verdict:** Major drift
**Notes:** Input field is named `did` instead of `account`; this is a wire-level breaking divergence — a bsky-PDS-shaped client will send `{account: ...}` and Aurora's serde will reject the request. The optional `note` field is not accepted at all. This endpoint and `enableAccountInvites` share the `AccountInvitesRequest` struct and have identical drift.

---

### enableAccountInvites

**Aurora-Locus location:** [src/api/admin.rs:1830-1834](src/api/admin.rs#L1830-L1834) (`AccountInvitesRequest`), [src/api/admin.rs:1837-1866](src/api/admin.rs#L1837-L1866) (`enable_account_invites`)

| Dimension | Aurora-Locus | bsky-PDS reference | Match? |
|---|---|---|---|
| Type | procedure | procedure | ✓ |
| Required input | `did` (string) | `account` (string, format=did) | ✗ field renamed |
| Optional input | (none) | `note` (string) | ✗ missing |
| Output | `{success: bool, did: string, invitesEnabled: true}` | (no output schema) | ✗ extra payload |
| Errors declared | (none) | (none) | ✓ |

**Verdict:** Major drift
**Notes:** Same divergence as `disableAccountInvites`. Fix is shared: rename the request field to `account`, accept optional `note`, drop the response payload (or keep it behind a feature flag).

---

### getAccountInfos

**Aurora-Locus location:** [src/api/admin.rs:1376-1433](src/api/admin.rs#L1376-L1433) (query/response structs), [src/api/admin.rs:1439-1534](src/api/admin.rs#L1439-L1534) (handler)

| Dimension | Aurora-Locus | bsky-PDS reference | Match? |
|---|---|---|---|
| Type | query | query | ✓ |
| Required input | `dids` (comma-separated string in single param) | `dids` (array of did, repeated query param) | ✗ encoding |
| Optional input | (none) | (none) | ✓ |
| Output `infos` field | array, present, required | array, present, required | ✓ |
| `accountView.did` | required, string | required, string format=did | ✓ |
| `accountView.handle` | optional (`Option<String>`) | required, string format=handle | ✗ required→optional |
| `accountView.indexedAt` | required, datetime string | required, datetime | ✓ |
| `accountView.email` | optional | optional | ✓ |
| `accountView.relatedRecords` | (omitted entirely) | optional, array of unknown | ✗ missing field (optional, but absent) |
| `accountView.invitedBy` | optional, struct | optional, ref to inviteCode | ✓ |
| `accountView.invites` | array (always serialised) | optional array | ⚠ always-present array vs optional |
| `accountView.invitesDisabled` | bool, always serialised | optional bool | ⚠ always-present vs optional |
| `accountView.emailConfirmedAt` | optional | optional datetime | ✓ |
| `accountView.inviteNote` | optional (always None) | optional | ✓ |
| `accountView.deactivatedAt` | optional datetime | optional datetime | ✓ |
| `accountView.threatSignatures` | array (always empty, always serialised) | optional array | ⚠ always-present vs optional |
| Errors declared | (none) | (none) | ✓ |

**Verdict:** Major drift
**Notes:** Two material problems. (1) **Input encoding:** lexicon `params` arrays are conventionally encoded as repeated query params (`?dids=did:plc:a&dids=did:plc:b`), but Aurora parses a single comma-separated string. A bsky-PDS-shaped client will send repeated params and Aurora's `Query<GetAccountInfosQuery>` will deserialise only the last one. (2) **`accountView.handle` required→optional:** this can change the contract for callers who type the response. The remaining "always-present optional" fields (`invites`, `invitesDisabled`, `threatSignatures`) are wire-compatible because `Vec` and `bool` serialise unconditionally, but they diverge from the spec's optionality and could surprise strict deserialisers. Missing `relatedRecords` is acceptable for now (optional in spec).

---

### getInviteCodes

**Aurora-Locus location:** [src/api/admin.rs:273-282](src/api/admin.rs#L273-L282) (query/response structs), [src/api/admin.rs:285-298](src/api/admin.rs#L285-L298) (handler)

| Dimension | Aurora-Locus | bsky-PDS reference | Match? |
|---|---|---|---|
| Type | query | query | ✓ |
| Required input | (none) | (none) | ✓ |
| Optional input | `includeDisabled` (bool) | `sort` (string, knownValues recent/usage, default recent), `limit` (int, 1-500, default 100), `cursor` (string) | ✗ entirely different params |
| Output `codes` | array, required | array, required, ref to inviteCode | ✓ shape compatible |
| Output `cursor` | (omitted) | optional string | ✗ missing |
| Errors declared | (none) | (none) | ✓ |

**Verdict:** Major drift
**Notes:** Aurora exposes a different control surface entirely — `includeDisabled` is a Aurora-Locus-only knob; spec offers sort/limit/cursor pagination. Lack of cursor support means the response cannot scale to large invite-code corpora and breaks any client that follows cursor pagination. The `codes` array shape is compatible (Aurora's `InviteCode` matches `com.atproto.server.defs#inviteCode` field-by-field — confirmed via the `list_invite_codes` handler at [src/api/admin.rs:318-334](src/api/admin.rs#L318-L334) which uses the same `InviteCode` type). Note: `listInviteCodes` (separate Aurora endpoint) also calls `list_codes(false)` and accepts `limit`/`cursor` query params but ignores them — see the `#[allow(dead_code)] // TODO: Implement pagination` markers at [src/api/admin.rs:301-308](src/api/admin.rs#L301-L308).

---

### getSubjectStatus

**Aurora-Locus location:** [src/api/admin.rs:1610-1634](src/api/admin.rs#L1610-L1634) (query + response structs), [src/api/admin.rs:1663-1779](src/api/admin.rs#L1663-L1779) (handler)

| Dimension | Aurora-Locus | bsky-PDS reference | Match? |
|---|---|---|---|
| Type | query | query | ✓ |
| Required input | (none) | (none) | ✓ |
| Optional input | `did`, `uri`, `blob` | `did` (format=did), `uri` (format=at-uri), `blob` (format=cid) | ✓ (format unenforced) |
| Output `subject` (required) | `{$type, did?, uri?, cid?}` polymorphic struct | union of repoRef / strongRef / repoBlobRef | ✓ (semantically) |
| Output `takedown` | optional, `{applied: bool, ref?: string}` always present with applied=false when unset | optional, `statusAttr` | ⚠ always-present vs optional |
| Output `deactivated` | optional, `{applied: bool, ref?: string}` always present (ref carries `deactivated_at` timestamp) | optional, `statusAttr` | ⚠ always-present + ref semantics |
| Output `suspended` | optional, `{applied: bool, ref?: string}` always present | (not in lexicon) | ✗ extra field |
| Errors declared | (none) | (none) | ✓ |
| Blob query support | returns 501 NOT_IMPLEMENTED | spec accepts blob param | ⚠ unimplemented |

**Verdict:** Minor drift
**Notes:** The subject union is implemented correctly via a polymorphic struct with `$type` discriminator and skip-if-none fields. `takedown` and `deactivated` are always serialised (with `applied: false` when unset) rather than being omitted — wire-compatible but spec-divergent. The Aurora-only `suspended` field is the most significant addition; either it should move to `tools.aurora.*` namespace (per the Phase 2/3 plan in the assessment doc) or be folded into `takedown` semantics. The `deactivated.ref` field stuffs the `deactivated_at` timestamp into a slot the spec describes as a free-form `string`, which is allowed but unusual. Blob queries returning 501 is a soft gap rather than a shape divergence.

---

### sendEmail

**Aurora-Locus location:** [src/api/admin.rs:1149-1170](src/api/admin.rs#L1149-L1170) (request/response structs), [src/api/admin.rs:1176-1229](src/api/admin.rs#L1176-L1229) (handler)

| Dimension | Aurora-Locus | bsky-PDS reference | Match? |
|---|---|---|---|
| Type | procedure | procedure | ✓ |
| Required input `recipientDid` | required, string | required, string format=did | ✓ |
| Required input `content` | required, string | required, string | ✓ |
| Required input `subject` | **required**, string | optional, string | ✗ optional→required |
| Required input `senderDid` | **optional**, string | required, string format=did | ✗ required→optional |
| Optional input `comment` | optional, string | optional, string | ✓ |
| Output `sent` | required, bool | required, bool | ✓ |
| Errors declared | (none) | (none) | ✓ |

**Verdict:** Major drift
**Notes:** Both flips matter. A spec-compliant client sending only `{recipientDid, content, senderDid}` (no subject) gets a 400 from Aurora's serde. A spec-compliant client omitting `senderDid` and relying on a server-side default cannot do that with bsky-PDS, but Aurora *does* allow it — falling back to the authenticated admin's DID at [src/api/admin.rs:1208](src/api/admin.rs#L1208) — which is a safer default but diverges from spec. The `senderDid` required-ness flip is the more user-visible difference: it's the kind of thing that would only surface on integration testing.

---

### updateAccountEmail

**Aurora-Locus location:** [src/api/admin.rs:572-578](src/api/admin.rs#L572-L578) (`UpdateAccountEmailRequest`), [src/api/admin.rs:581-617](src/api/admin.rs#L581-L617) (handler)

| Dimension | Aurora-Locus | bsky-PDS reference | Match? |
|---|---|---|---|
| Type | procedure | procedure | ✓ |
| Required input | `did` (string), `email` (string) | `account` (string format=at-identifier), `email` (string) | ✗ field renamed + type narrowed |
| Optional input | (none) | (none) | ✓ |
| Output | `{success, did, email}` | (no output schema) | ✗ extra payload |
| Errors declared | (none) | (none) | ✓ |

**Verdict:** Major drift
**Notes:** Two issues stack here. (1) Input field renamed `account` → `did`, breaking wire compatibility. (2) Type narrowed from `at-identifier` (handle OR DID) to DID-only — Aurora rejects handle-form input via the `starts_with("did:")` check at [src/api/admin.rs:587](src/api/admin.rs#L587). Operators using bsky-PDS-shaped tooling and entering `alice.example.com` will be rejected. The extra response body is cosmetic but adds to the "not quite the same shape" pile.

---

### updateAccountHandle

**Aurora-Locus location:** [src/api/admin.rs:619-625](src/api/admin.rs#L619-L625) (`UpdateAccountHandleRequest`), [src/api/admin.rs:628-665](src/api/admin.rs#L628-L665) (handler)

| Dimension | Aurora-Locus | bsky-PDS reference | Match? |
|---|---|---|---|
| Type | procedure | procedure | ✓ |
| Required input | `did` (string), `handle` (string) | `did` (string format=did), `handle` (string format=handle) | ✓ (format unenforced) |
| Optional input | (none) | (none) | ✓ |
| Output | `{success, did, handle}` | (no output schema) | ✗ extra payload |
| Errors declared | (none) | (none) | ✓ |

**Verdict:** Minor drift
**Notes:** Field names match. The only deviation is the extra response body. Format validation is informal (`starts_with("did:")`, length check) but does not change wire shape. Closest-to-clean of the eleven.

---

### updateAccountPassword

**Aurora-Locus location:** [src/api/admin.rs:667-673](src/api/admin.rs#L667-L673) (`UpdateAccountPasswordRequest`), [src/api/admin.rs:676-713](src/api/admin.rs#L676-L713) (handler)

| Dimension | Aurora-Locus | bsky-PDS reference | Match? |
|---|---|---|---|
| Type | procedure | procedure | ✓ |
| Required input | `did` (string), `password` (string) | `did` (string format=did), `password` (string) | ✓ (format unenforced) |
| Optional input | (none) | (none) | ✓ |
| Output | `{success, did, message}` | (no output schema) | ✗ extra payload |
| Errors declared | (none) | (none) | ✓ |

**Verdict:** Minor drift
**Notes:** Field names and required-ness match. Same extra-response-body issue as the others. Aurora additionally enforces `password.len() >= 8`; spec is silent on length, so this is operator policy rather than shape divergence.

---

### updateSubjectStatus

**Aurora-Locus location:** [src/api/admin.rs:1536-1544](src/api/admin.rs#L1536-L1544) (`UpdateSubjectStatusRequest`), [src/api/admin.rs:1546-1608](src/api/admin.rs#L1546-L1608) (handler)

| Dimension | Aurora-Locus | bsky-PDS reference | Match? |
|---|---|---|---|
| Type | procedure | procedure | ✓ |
| Required input `subject` | string (DID or AT-URI heuristic-parsed) | union of repoRef / strongRef / repoBlobRef | ✗ scalar vs union |
| Optional input `action` | string (defaults to "", knownValues suspend/takedown/restore) | (not in spec) | ✗ Aurora-only |
| Optional input `duration` | int (seconds) | (not in spec) | ✗ Aurora-only |
| Optional input `takedown` | (not accepted) | optional, ref to statusAttr | ✗ missing |
| Optional input `deactivated` | (not accepted) | optional, ref to statusAttr | ✗ missing |
| Output `subject` | (not returned) | required, union | ✗ missing |
| Output `takedown` | (not returned) | optional, ref to statusAttr | ✗ missing |
| Output (Aurora-form) | `{success, did, action}` | (per spec above) | ✗ |
| Errors declared | (none) | (none) | ✓ |

**Verdict:** Major drift
**Notes:** This endpoint is the most divergent of the eleven. Aurora's design models moderation as imperative `action` verbs (`suspend`/`takedown`/`restore`) plus a `duration`; the spec models it as a declarative status patch (set `takedown.applied = true`, set `deactivated.applied = false`, etc.) where the server reads off the desired post-state from the request body. The two designs are not field-renames apart — they are structurally different. This drift is **already known and tracked** as Phase 1.6 (chainlink #61, "updateSubjectStatus polymorphism"); this audit confirms the scope. Note also that `restore` short-circuits with a stub message ([src/api/admin.rs:1574-1583](src/api/admin.rs#L1574-L1583)) — restore is not implemented, only signposted.

---

## Summary

| Verdict bucket | Endpoints | Count |
|---|---|---|
| Clean (full shape parity) | (none) | 0 |
| Minor drift (extra response payload, isolated field gap) | `deleteAccount`, `getSubjectStatus`, `updateAccountHandle`, `updateAccountPassword` | 4 |
| Major drift (input/output shape divergence) | `disableAccountInvites`, `enableAccountInvites`, `getAccountInfos`, `getInviteCodes`, `sendEmail`, `updateAccountEmail`, `updateSubjectStatus` | 7 |
| Not found | (none) | 0 |
| **Total** | — | **11** |

**Recurring drift patterns:**

1. **Procedures emit non-spec response bodies** (5 of 7 procedures audited).
   `{success: true, ...}`-style envelopes are returned where the lexicon
   declares no output schema. Wire-compatible for any client that ignores the
   body, but cosmetically inconsistent. Easy bulk fix during Phase 2 cleanup.

2. **`account` (at-identifier) parameters renamed to `did` (DID-only string)**
   — affects `disableAccountInvites`, `enableAccountInvites`, `updateAccountEmail`.
   This is a wire-breaking rename. Likely a translation artefact from an early
   Rust port that preferred Rust-friendly names; needs deliberate rename back.
   Note that `updateAccountEmail` also narrows the type from at-identifier to
   DID-only at the validation layer.

3. **`Option<T>` fields are serialised as always-present with sentinel values**
   instead of omitted. Most visible in `getSubjectStatus` (`takedown`/
   `deactivated` always present with `applied: false`) and `getAccountInfos`
   (`invites`/`threatSignatures` always present as empty arrays). Wire-compatible
   for permissive clients; spec-divergent for strict ones.

4. **Pagination is largely unimplemented** on query endpoints that the spec
   says should support it (`getInviteCodes` lacks sort/limit/cursor entirely;
   `listInviteCodes` accepts limit/cursor but ignores them).

5. **`updateSubjectStatus` is structurally divergent**, not just shape-divergent.
   The `action`-verb model versus the spec's status-patch model is a design
   difference that requires Phase 1.6 (#61) work, not a rename.

**Format-validation note:** none of Aurora's request structs enforce the
lexicon's `format=did|handle|at-uri|cid|datetime` constraints at the serde
layer; validation is done ad hoc in handlers (e.g. `starts_with("did:")`).
This is not a shape divergence but it does mean Aurora will accept inputs
that bsky-PDS would reject (e.g., `did:` with malformed body), and reject
some it shouldn't (e.g., `at-identifier` parameters that are valid handles).

**Recommended follow-up:**

- File one issue per major-drift endpoint (6 new — `updateSubjectStatus` is
  already #61). Group `disableAccountInvites` + `enableAccountInvites` under
  a single issue since they share the request struct.
- File a single sweep issue for the minor-drift response-body cleanup
  (covering all 4 minor + the 5 procedures with extra payloads).
- Defer the "always-present `Option`" pattern to Phase 2, since it is a
  serialisation-strategy decision that affects the whole `tools.aurora.*`
  surface, not just these eleven endpoints.

Phase 1.6 (#61) already covers `updateSubjectStatus` and is the right home
for the structural redesign — no separate issue needed for that endpoint.

— End of audit —
