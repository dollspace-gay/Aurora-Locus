# Contract Stability

Operator-and-integrator guide to Aurora-Locus's stability contracts
on its admin-and-capability surfaces.

Aurora-Locus commits to six stability contracts. If you're deploying
Aurora-Locus or building external tools against it, this document
tells you which surfaces are stable and how to write code that won't
break across releases.

The contracts are committed in source via doc comments at canonical
locations and pinned by automated tests. Removing a commitment
phrase from its doc location, or breaking the wire shape of a
committed surface, fails CI loudly.

---

## TL;DR

| # | Surface | Promise | Canonical commitment | Test that pins it |
|---|---|---|---|---|
| 1 | `Subject` (canonical Aurora) | Variants additive only; no shape change | `crate::admin::defs::Subject` doc comment | `subject_*_wire_format_snapshot` in `src/admin/defs.rs` |
| 1 | `ReportSubject` (createReport input) | Variants additive only; no shape change | `crate::api::moderation::ReportSubject` doc comment | `report_subject_*_wire_format_snapshot` in `src/api/moderation.rs` |
| 2 | `describeCapabilities` response | Field names + shapes additive only | `crate::api::admin::DescribeCapabilitiesResponse` doc comment | `describe_capabilities_snapshot` in `src/api/admin.rs` |
| 3 | Capability strings | `<kebab-family>-v<integer>`; breaking changes ship as new version | `crate::api::admin::aurora_capability_extensions` doc comment | `describe_capabilities_snapshot` (full set) + `contract_phrases` (versioning convention) |
| 4 | Action-ID surfacing | Aurora-namespace handlers writing chain entries surface `auditEntryId` (and `eventId` if also writing event rows) | `crate::admin::audit_chain` module doc | `tests/admin_handler_contract.rs` (structural lint) |
| 5 | Audit-trail read | Wire shape, filter set, and canonical hash-input form for `getAuditTrail`; external chain verification reproducible per [audit-chain-verification.md](audit-chain-verification.md) | `crate::api::aurora_admin::GetAuditTrailOutput` doc comment | `tests/audit_chain_canonical_verification.rs` (canonical-form + worked-example hashes) |
| 6 | Multi-subject emitEvent | Input/output shape, per-action multi-subject support set, per-action `MAX_BATCH_SIZE`, atomicity scope, chain row shape | `crate::api::aurora_admin::EmitEventOutput` doc comment | `tests/contract_phrases.rs` (phrase) + snapshot tests in `src/api/aurora_admin.rs` (wire shape) |

---

## 1. Subject vocabulary stability

Aurora-Locus's admin surface uses two **distinct** Subject shapes
on the wire — they look similar but are committed independently.

### Canonical Aurora Subject

The `Subject` enum in `crate::admin::defs` is the type
Aurora-namespace handlers serialize on `getAuditTrail`,
`subscribeModEvents`, batch-label endpoints, `getSubjectContext`,
`queryEvents`, and `listAppeals`. Wire format is **internally
tagged** with `$type`:

```json
// Repo
{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:..."}

// Record
{"$type":"com.atproto.repo.strongRef","cid":"...","uri":"at://..."}

// Blob
{"$type":"com.atproto.admin.defs#repoBlobRef",
 "cid":"...","did":"did:plc:...","record_uri":"at://..."}
```

Three variants today: `Repo`, `Record`, `Blob`. The contract is
that new variants may be added in future releases (additive), but
the wire shape of these three will not change.

The `record_uri` field on `Blob` is snake_case — a deliberate
reconciliation to byte-match the parsing dual
`SubjectUnion::RepoBlobRef` on `updateSubjectStatus`. Once shipped,
snake_case is part of the contract.

### createReport subject

The `ReportSubject` enum in `crate::api::moderation` is the
inbound subject shape on `com.atproto.moderation.createReport`.
Wire format is **untagged** at the enum level — variants
disambiguate via the inner struct's `$type` field, not via
serde's tag attribute:

```json
// Repo
{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:..."}

// StrongRef
{"$type":"com.atproto.repo.strongRef","cid":"...","uri":"at://..."}
```

Two variants today: `Repo`, `StrongRef`. Contract is the same:
new variants additive only; existing shapes do not change.

### Why two surfaces

The two Subject shapes are intentionally distinct. The canonical
Aurora Subject covers the four-variant `tools.aurora.admin.*`
surface; `ReportSubject` covers the two-variant
`com.atproto.moderation.createReport` upstream surface. Cross-
surface byte comparison is not meaningful — both shapes are
correct in their own context.

---

## 2. describeCapabilities response shape

The `tools.aurora.admin.describeCapabilities` endpoint returns a
fixed-shape response:

```json
{
  "extensions": [{"name":"<capability-string>"}, ...],
  "families": {
    "tools.aurora.admin":     ["emitEvent", ...],
    "tools.aurora.moderator": ["queryEvents", ...],
    "tools.aurora.ops":       ["getStats", ...],
    "tools.aurora.superadmin":["grantRole", "revokeRole"]
  },
  "implementation": "aurora-locus",
  "version": "0.X.0"
}
```

Field names (`extensions`, `families`, `implementation`, `version`)
and the shape of each (array of `{name}` objects, map of namespace
→ array of endpoint names, two strings) are stable. New fields may
be added; existing fields will not change name or shape.

The full snapshot — every endpoint string, every capability
extension — is pinned by the `describe_capabilities_snapshot` test.
Adding or removing an endpoint to the advertised set is a wire
change and updates the snapshot in lockstep with the CHANGELOG.

---

## 3. Capability string versioning

Capability strings inside `extensions[]` follow the pattern
`<kebab-family>-v<integer>`:

- `<kebab-family>` is a kebab-case family name (e.g., `subject-context`,
  `audit-trail`, `mod-events-emit`).
- `-v` is a literal lowercase `v` separator.
- `<integer>` is the version number, starting at `1`.

Examples currently shipped: `subject-context-v1`, `audit-trail-v1`,
`mod-events-emit-v1`, `runtime-settings-v1`.

**Breaking-change discipline**: when a capability's wire shape
changes incompatibly, the new shape ships as a NEW version suffix
(e.g., `subject-context-v2`). The OLD version is removed only after
the new version has shipped and consumers have had time to migrate.
Bumping the integer is the wire signal that breaking change has
landed; consumers that gate on the old string keep working until
the old version is removed.

Two §8.15 vocabulary entries are intentionally **omitted** from
the current advertised set: `invite-lineage-v1` and
`reporter-context-v1`. Their endpoints aren't shipped yet; the
strings will be added when their handlers land.

---

## 4. Action-ID surfacing

Every `tools.aurora.*` admin handler that writes an
`audit_chain_entry` row surfaces `auditEntryId` in its response,
on a typed `*Output` struct:

```json
{
  "auditEntryId": "1234",
  // ...other handler-specific fields
}
```

Handlers that also write `moderation_event` rows additionally
surface `eventId`:

```json
{
  "auditEntryId": "1234",
  "eventId": "5678",
  // ...
}
```

The contract applies to **Aurora-namespace handlers only**
(`tools.aurora.*`). Upstream-lexicon handlers (`com.atproto.*`)
preserve lexicon conformance — those response shapes are not
covered by this contract.

**Allowlist**: one handler is exempt because its response is
binary rather than JSON. `tools.aurora.admin.exportAccountForensic`
returns a tar archive and surfaces the audit entry ID via the
`X-Aurora-Audit-Entry-Id` HTTP response header instead. The
allowlist is documented in `tests/admin_handler_contract.rs`.

Drift is caught by the structural lint at
`tests/admin_handler_contract.rs`: any new Aurora-namespace handler
that writes a chain entry but doesn't surface `auditEntryId` (and
isn't on the allowlist) fails the lint.

---

## 5. Audit-trail read

The `tools.aurora.admin.getAuditTrail` response shape is stable.
The seven-filter set is locked (AND-combined): `actor_did`,
`action`, `subject_did`, `subject_uri`, `subject_cid`,
`after_created`, `before_created`. Pagination semantics
(forward-only, newest-first, base64-encoded `CursorPosition`
cursor, default 50 max 100), per-entry wire format (`AuditEntry`
with `cascadeSnapshotIds`), and verification semantics
(`chainVerified` over the whole chain, `chainVerifiedThrough` =
head sequence on success or `failing_sequence - 1` on chain-level
failure) are all locked. New per-entry fields and new top-level
fields may be added additively; removal of any committed surface
is breaking.

The wire-to-canonical bridge (the transformation rules an external
consumer needs to independently verify chain hashes) is documented
at [audit-chain-verification.md](audit-chain-verification.md),
which includes the canonical hash-input shape, per-variant Subject
decomposition rules, and six worked examples with reproducible
SHA-256 hashes.

Canonical commitment: `crate::api::aurora_admin::GetAuditTrailOutput`
doc comment. Drift on the contract phrase is caught by
`tests/contract_phrases.rs`; drift on the canonical form is
caught by `tests/audit_chain_canonical_verification.rs`, which
reproduces the documented transformation rules + worked-example
hashes against the production writer.

---

## 6. Multi-subject emitEvent

The `tools.aurora.admin.emitEvent` input/output shapes are stable.
The input field `subjects: Vec<Subject>` accepts one or more
subjects per call; the output field `snapshots: Vec<SnapshotRef>`
pairs 1:1-by-index with `subjects` (empty when
`snapshot_capture: false`). Single-subject callers wrap in a
one-element array.

Per-action multi-subject support follows a committed action
vocabulary. Multi-subject is supported on:

- **Account state**: `TakedownAccount`, `SuspendAccount`,
  `RestoreAccount`, `DeleteAccount`.
- **Label**: `ApplyLabel`, `RemoveLabel`.
- **Blob lifecycle**: `QuarantineBlob`, `RestoreBlob`,
  `DeleteBlob`.
- **Record takedown**: `TakedownRecord`.
- **Subject status**: `UpdateSubjectStatus`.

Multi-subject is **refused** (HTTP 400
`SubjectsArrayInvalidForAction`) on:

- **Embedded-id variants**: `ResolveReport`, `DismissReport`,
  `ResolveAppeal`, `EscalateAppeal` — the embedded report/appeal
  ID makes the call inherently single-subject.
- **`SendEmail`**: per-message addressing is single-subject.

Per-action `MAX_BATCH_SIZE` caps:

| Action | Cap | Reason |
|---|---|---|
| `DeleteAccount` | 10 | Irreversible |
| `DeleteBlob` | 25 | Storage-irreversible |
| All other multi-subject-supported actions | 50 | General hard cap |

### Atomicity contract

Pre-tx snapshot capture; per-subject mutation in tx via
`_in_tx` manager variants; chain entry write inside the same
tx; commit makes everything visible atomically.

Per-subject mutation failure aborts the whole tx and surfaces
the failing subject's index and identifier in the response body
(`failingSubject`, `failingSubjectId` keys). Snapshot capture
failures BEFORE the tx leave orphan snapshots (deliberate
carve-out — the chain entry never lands, so the orphan rows
have no chain-of-custody and can be reconciled by GC).

### Chain row shape

- **Single-subject events** populate BOTH the flat
  `subject_did`/`subject_uri`/`subject_cid` columns AND
  `cascade_subjects: [s]`. External consumers can read either
  surface and get the same subject identity.
- **Multi-subject events** use synthetic-primary: NULL flat
  columns, populated `cascade_subjects: [s1, s2, ...]`.

This means `cascade_subjects` always contains every subject
regardless of arity — consumers can rely on it as the
authoritative subject list.

### Canonical commitment

`crate::api::aurora_admin::EmitEventOutput` doc comment. Drift
on the contract phrase ("emitEvent multi-subject contract is
committed") is caught by `tests/contract_phrases.rs`; drift on
the wire shape is caught by snapshot tests in
`src/api/aurora_admin.rs`'s test module.

## Out of scope

Stability contracts apply to the six surfaces above only. Other
admin surfaces (individual handlers' request shapes, moderation
queue ordering, internal database schemas, log line formats) are
**not covered** by these contracts and may change between minor
versions.

The canonical source of truth for surfaces not committed here is
the in-source doc comment on each handler / type.

---

## Versioning context

These contracts are stable forward. Contract changes between major
versions are possible but always announced in CHANGELOG with
migration guidance. The contract phrases above are not promises that
the contracts last forever; they are promises that breaking the
contracts is **loud, not silent**.
