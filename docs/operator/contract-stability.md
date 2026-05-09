# Contract Stability

Operator-and-integrator guide to Aurora-Locus's stability contracts
on its admin-and-capability surfaces.

Aurora-Locus v0.3 commits to four stability contracts. If you're
deploying Aurora-Locus or building external tools against it, this
document tells you which surfaces are stable and how to write code
that won't break across releases.

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

The `record_uri` field on `Blob` is snake_case — this is a
deliberate v0.3 reconciliation (Step 0.5) to byte-match the parsing
dual `SubjectUnion::RepoBlobRef` on `updateSubjectStatus`. Once
shipped, snake_case is part of the contract.

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
Pagination semantics, the committed filter set
(`actor_did`, `action`, `subject_did`, `subject_uri`, `subject_cid`,
`after_created`, `before_created`), and per-entry wire format are
locked. New per-entry fields may be added; existing fields will
not change name, type, or representation.

The wire-to-canonical bridge (the transformation rules an external
consumer needs to independently verify chain hashes) is documented
at [audit-chain-verification.md](audit-chain-verification.md),
which includes the canonical hash-input shape, per-variant Subject
decomposition rules, and six worked examples with reproducible
SHA-256 hashes.

Canonical commitment: `crate::api::aurora_admin::GetAuditTrailOutput`
doc comment (committed in Arc 3 Step 3). Drift on the canonical
form is caught by
`tests/audit_chain_canonical_verification.rs`, which reproduces
the documented transformation rules + worked-example hashes
against the production writer.

## Out of scope

Stability contracts apply to the five surfaces above only. Other
admin surfaces (individual handlers' request shapes, moderation
queue ordering, internal database schemas, log line formats) are
**not covered** by these contracts and may change between minor
versions.

The Aurora design doc at `docs/AURORA_DESIGN.md` and the admin
UI design at `docs/AURORA_ADMIN_UI_DESIGN.md` are the canonical
sources of truth for surfaces not committed here.

---

## Versioning context

These contracts apply to v0.3 forward. v0.2 surfaces are considered
de-facto stable in retrospect but were not formally committed;
consumers building against v0.3+ have the explicit guarantees
above.

Contract changes between major versions (v0.3 → v0.4 → ...) are
possible but always announced in CHANGELOG with migration
guidance. The contract phrases above are not promises that the
contracts last forever; they are promises that breaking the
contracts is **loud, not silent**.
