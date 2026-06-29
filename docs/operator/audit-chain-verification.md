# Audit chain verification

Aurora-Locus exposes the admin audit chain via
`tools.aurora.admin.getAuditTrail`. External consumers can
**independently verify chain integrity** by recomputing SHA-256
hashes from response data — the wire response contains every
field the canonical hash input depends on.

This document specifies:

- **Section A** — the wire response shape (what comes off
  `getAuditTrail`).
- **Section B** — the canonical hash-input shape (what gets
  SHA-256'd).
- **Section C** — the wire-to-canonical bridge (the transformation
  rules between the two).
- **Section D** — seven worked examples with byte-equal canonical
  forms and the SHA-256 hashes they produce. A consumer
  implementation that reproduces these hashes byte-for-byte is
  correct.

The worked examples in Section D come from the side-script test
at [tests/audit_chain_canonical_verification.rs](../../tests/audit_chain_canonical_verification.rs).
The doc and the side-script must agree; the side-script is the
executable form of Section C.

> **v0.9 chain-format bump (chainlink #345).** The canonical input
> gained two fields — `source` (a provenance discriminator) and
> `payload` (action-specific scalars) — both now SHA-256'd. Rows
> written before v0.9 hash under the prior 13-field form and will
> not re-verify under the 15-field form; this is a deliberate
> chain-era boundary (see Versioning). All hashes and canonical
> forms in this document are the **v0.9 (15-field)** form.

---

## Section A — Wire response shape

`getAuditTrail` returns `GetAuditTrailOutput`:

```json
{
  "items": [<AuditEntry>, <AuditEntry>, ...],
  "cursor": "<opaque-cursor-string>",
  "chainVerified": true,
  "chainVerifiedThrough": 42
}
```

Each item is an `AuditEntry`:

| Field | JSON type | Notes |
|---|---|---|
| `id` | string | Row primary key, stringified i64. |
| `sequence` | number | Monotonic chain sequence (i64). |
| `timestamp` | string | RFC 3339 with subsecond precision when present. |
| `actorDid` | string | DID that authored the decision. |
| `action` | string | Action verb (e.g., `TakedownAccount`). |
| `subjectRef` | object \| null | Discriminated Subject (see below). |
| `rationale` | string | Operator-supplied free text. |
| `snapshotId` | string \| null | Audit snapshot id, **stringified i64** for JS-precision parity. |
| `eventId` | string \| null | Moderation event id, **stringified i64** for JS-precision parity. |
| `currentHash` | string | SHA-256 over the canonical input — the hash this doc tells you how to reproduce. Hex-lowercase, 64 chars. |
| `previousHash` | string \| null | Prior row's `currentHash`. **`null` for the genesis row.** |
| `verified` | boolean | Aurora-Locus's per-row verify check. Independent verification is what this doc enables. |
| `cascadeSubjects` | array of object | Per-subject Subject objects. Single-subject events carry a one-element array (mirroring the `subjectRef` field); multi-subject events carry one element per subject. The array is empty only on legacy single-subject chain rows that pre-date the cascade-population convention, or on chain entries written without a subject. |
| `cascadeSnapshotIds` | array of (string \| null) | Per-subject snapshot ids paired by index with `cascadeSubjects`. **Stringified i64** values; `null` per element when the subject wasn't snapshottable at decision time. Empty array when `snapshot_capture: false` was passed or for chain entries without subjects. |
| `source` | string | **(v0.9)** Provenance discriminator: `default_action`, `auto_label_rule`, `manual`, `stale_expiration`, `operator_removal`, `escalation`, or `system_diagnostic`. Operator-initiated decisions use `manual`. Never null. Direct copy into the canonical input. |
| `payload` | object \| absent | **(v0.9)** Action-specific scalars (e.g. `{"applied":true}` on `moderation_auto_label_applied`). Omitted from the wire object when the action carries no payload. See Section C for the wire→canonical conversion. |

`subjectRef` is one of three discriminated shapes:

```json
// Repo (account-level)
{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:..."}

// Record (single record)
{"$type":"com.atproto.repo.strongRef","cid":"...","uri":"at://..."}

// Record (cascade entry from `batchTakedownRecords` — URI-level convention)
{"$type":"com.atproto.repo.strongRef","cid":"","uri":"at://..."}

// Blob (with optional originating record URI)
{
  "$type": "com.atproto.admin.defs#repoBlobRef",
  "cid": "...",
  "did": "did:plc:...",
  "record_uri": "at://..."   // optional; omitted when not present
}
```

A `Record` entry with an empty-string `cid` inside
`cascadeSubjects` signals **URI-level takedown semantics**, not
missing data. This shape is produced exclusively by
`tools.aurora.admin.batchTakedownRecords` and is pinned by
`batch_takedown_records_produces_uri_level_cascade_with_empty_cids`.
Single-subject paths (e.g., `emitEvent{TakedownRecord}`) carry
real CIDs and remain CID-level. The `subject_cid` canonical
column for these cascade entries is the empty string verbatim
(no normalization) — see Section C.

Subject vocabulary stability is a separate contract — see
[contract-stability.md §1](contract-stability.md#1-subject-vocabulary-stability).

---

## Section B — Canonical hash-input shape

The bytes that get SHA-256'd are the UTF-8 encoding of a JSON
object with these 15 fields (13 pre-v0.9, plus `payload` and
`source` from the v0.9 bump). Aurora-Locus's writer constructs the
object via `serde_json::json!({...})` with the 15 keys listed in
alphabetical source order, so **serialized keys come out in
alphabetical order** regardless of which `serde_json::Map` backing
the build graph happens to select (`BTreeMap` when the
`preserve_order` cargo feature is off, `IndexMap` when it's on —
v0.7+ has `preserve_order` enabled transitively via a dependency,
and writing keys in alphabetical source order is what guarantees
the wire invariant durably across feature-graph drift in either
direction).

Field order in the canonical JSON (alphabetical):

1. `action`
2. `actor_did`
3. `cascade_snapshot_ids`
4. `cascade_subjects`
5. `event_id`
6. `payload` *(v0.9)*
7. `previous_hash`
8. `rationale`
9. `sequence`
10. `snapshot_id`
11. `source` *(v0.9)*
12. `subject_cid`
13. `subject_did`
14. `subject_uri`
15. `timestamp`

Per-field representation in the canonical input:

| Canonical field | Type | Notes |
|---|---|---|
| `action` | string | Direct copy from wire's `action`. |
| `actor_did` | string | Direct copy from wire's `actorDid`. |
| `cascade_snapshot_ids` | string \| null | **JSON-encoded string** of the array (or `null` when empty); see Section C for the wire→canonical conversion. |
| `cascade_subjects` | string \| null | **JSON-encoded string** of the array (or `null` when empty); see Section C. |
| `event_id` | number \| null | **Numeric i64**, NOT the wire's stringified form. Convert from wire-string back to number before constructing the canonical input. |
| `payload` | string \| null | **(v0.9) JSON-encoded string** of the action-scalar object (or `null` when the action has no payload); see Section C. The same wire→canonical asymmetry as `cascade_subjects`: the wire carries the parsed object, the canonical input carries its serialized string embedded as a value. |
| `previous_hash` | string \| null | Direct copy. `null` for the genesis row. **Hashed inside the canonical object**, not via the textbook prefix-concat form. |
| `rationale` | string | Direct copy. |
| `sequence` | number | i64 as JSON number. |
| `snapshot_id` | number \| null | **Numeric i64**, NOT the wire's stringified form. Convert before hashing. |
| `source` | string | **(v0.9)** Direct copy from wire's `source`. Never null (operator decisions use `"manual"`). |
| `subject_cid` | string \| null | Per-Subject-variant; see Section C. |
| `subject_did` | string \| null | Per-Subject-variant; see Section C. |
| `subject_uri` | string \| null | Per-Subject-variant; see Section C. |
| `timestamp` | string | Direct copy from wire's `timestamp`. |

The hash function is **SHA-256** over the canonical JSON's UTF-8
byte representation. Output: **hex-lowercase, 64 characters**. The
result is what `currentHash` on the wire equals.

**`previous_hash` is inside the canonical object**, not concatenated
outside. The textbook form `SHA-256(prev_hash || canonical(fields))`
is NOT what Aurora-Locus does. The chain linkage comes from
`previous_hash` being one of the canonical-object's fields, just
like any other.

---

## Section C — Wire-to-canonical bridge

### Direct-copy fields

| Wire | Canonical | Conversion |
|---|---|---|
| `id` | (not in canonical) | `id` is the row primary key, surfaced for client convenience but not part of the hash input. |
| `actorDid` | `actor_did` | None. |
| `action` | `action` | None. |
| `rationale` | `rationale` | None. |
| `timestamp` | `timestamp` | None. |
| `sequence` | `sequence` | None (both numeric). |
| `previousHash` | `previous_hash` | None. `null` for genesis. |
| `source` | `source` | **(v0.9)** None — direct copy. Never null. |
| `currentHash` | (the hash itself) | This is what the canonical input produces. |
| `verified` | (not in canonical) | Server-side verification flag, surfaced for client convenience. |
| `chainVerified`, `chainVerifiedThrough` | (not in canonical) | Top-level fields on the response, not per-entry. |

### Stringified-i64 → numeric-i64

`snapshotId` and `eventId` on the wire are **stringified** for
JavaScript-precision parity (i64 max exceeds
`Number.MAX_SAFE_INTEGER`, so wire-numeric would lose precision
for large ids in JS). The canonical form uses **numeric** i64s.

```text
Wire:        "snapshotId": "9007199254740993"
Canonical:   "snapshot_id": 9007199254740993
```

Convert by parsing the wire string to i64 (or `BigInt`/`bigint`
in your consumer's language) before constructing the canonical
JSON.

### `subjectRef` decomposition

The wire has one composite `subjectRef` field; the canonical form
has three flat columns (`subject_did`, `subject_uri`,
`subject_cid`). The decomposition is per Subject variant:

| Wire `subjectRef` | `subject_did` | `subject_uri` | `subject_cid` |
|---|---|---|---|
| `null` (no subject) | `null` | `null` | `null` |
| `Repo { did }` | `did` | `null` | `null` |
| `Record { uri, cid }` | `null` | `uri` | `cid` |
| `Blob { did, cid, record_uri: <some> }` | `did` | `record_uri` | `cid` |
| `Blob { did, cid, record_uri: <none> }` | `did` | `null` | `cid` |

Note: `Blob`'s `record_uri` (optional in the wire payload) becomes
`subject_uri` in the canonical form when present, and `null`
otherwise. This means the same column slot (`subject_uri`) carries
different semantic meaning per variant — `Record`'s URI is the
record's `at://...` URI; `Blob`'s URI is the URI of the originating
record (when known).

### `cascadeSubjects` → canonical `cascade_subjects`

The wire has an array of Subject objects:

```json
"cascadeSubjects": [
  {"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:victim1"},
  {"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:victim2"}
]
```

The canonical form has a **JSON-encoded string** of the same
array — the literal characters that result from
`serde_json::to_string(&cascade_subjects)`, embedded as a string
value inside the canonical object:

```json
"cascade_subjects": "[{\"$type\":\"com.atproto.admin.defs#repoRef\",\"did\":\"did:plc:victim1\"},{\"$type\":\"com.atproto.admin.defs#repoRef\",\"did\":\"did:plc:victim2\"}]"
```

When the wire array is empty, the canonical value is `null`, NOT
`"[]"`.

### `cascadeSnapshotIds` → canonical `cascade_snapshot_ids`

Same JSON-encoded-string treatment as `cascade_subjects`, with one
additional conversion: each element is **stringified-i64 on the
wire**, **numeric-i64 in the canonical encoded string**.

```text
Wire:       "cascadeSnapshotIds": ["7", null, "12"]
Canonical:  "cascade_snapshot_ids": "[7,null,12]"
```

The outer `[]` in the canonical value is part of the JSON-encoded
string; the inner numbers are unquoted i64 values; `null`
represents subjects that weren't snapshottable at decision time.

When the wire array is empty, the canonical value is `null`.

### `payload` → canonical `payload` *(v0.9)*

Same JSON-encoded-string treatment as `cascade_subjects`. The wire
carries the parsed object (or omits the key entirely); the
canonical form carries the literal characters
`serde_json::to_string(&payload)` produces, embedded as a string
value:

```text
Wire:       "payload": {"applied": true}
Canonical:  "payload": "{\"applied\":true}"
```

When the action has no payload (the wire omits the `payload` key),
the canonical value is `null`, NOT `"{}"` or `""`. Aurora-Locus
hashes the **stored** serialized string verbatim; a consumer must
re-serialize the parsed object with no whitespace and the same key
order the producer used (Phase A payloads are flat single-key
objects, so this is unambiguous).

---

## Section D — Worked examples

Each example pairs a fixed input with the SHA-256 hash it produces.
Reproduce the hash with your implementation by:

1. Constructing the canonical JSON object per Section B with the
   field values shown.
2. Serializing to a UTF-8 byte string with **alphabetical key
   order** and **no whitespace** (the format
   `serde_json::to_string()` produces for a
   `BTreeMap`-backed `Map<String, Value>`).
3. Computing SHA-256 over the bytes.
4. Hex-encoding lowercase.

If your hash matches, your implementation is byte-equivalent to
Aurora-Locus's writer.

All examples use `timestamp: "2026-05-09T00:00:00Z"`, which is
fixed (not the production `Utc::now()`) so the hashes are
reproducible. In production, `timestamp` is the row's `created_at`
column value verbatim.

Examples 1–6 are operator-initiated decisions, so each carries
`source: "manual"` and `payload: null` (the v0.9 fields at their
default values). Example 7 exercises a substrate-emitted entry with
a non-`manual` source and a populated payload. Every canonical JSON
below is the **v0.9 (15-field)** form — `payload` sits between
`event_id` and `previous_hash`; `source` between `snapshot_id` and
`subject_cid`.

### Examples 1–4 (single-subject events) and the single-subject chain-row shape

Single-subject events populate BOTH the flat
`subject_did`/`subject_uri`/`subject_cid` columns AND a
single-element `cascade_subjects: [s]`. Multi-subject events
(Example 5) use synthetic-primary (NULL flat columns, populated
cascade). Examples 1–4 below reflect the current single-subject
shape; the `cascade_subjects` canonical column is a JSON-encoded
string of the one-element array. The production writer emits the
embedded Subject with `$type` first (from the internal-tag) and
then the struct fields in **source-declared order** (Repo: just
`did`; Record: `uri`, `cid`; Blob: `did`, `cid`, `record_uri`).

### Example 1 — `repoRef` Subject, genesis row

**Inputs**:
- `sequence: 1`
- `actor_did: "did:plc:moderator"`
- `action: "TakedownAccount"`
- `subject_did: "did:plc:test1234567890abcdef"`
- `subject_uri: null`
- `subject_cid: null`
- `rationale: "spam"`
- `snapshot_id: null`
- `event_id: null`
- `previous_hash: null` (genesis)
- `cascade_subjects`: JSON-encoded string of a one-element array:
  ```
  [{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:test1234567890abcdef"}]
  ```
- `cascade_snapshot_ids: null` (snapshot_capture: false)
- `payload: null`, `source: "manual"`

**Canonical JSON** (alphabetical key order, no whitespace):

```json
{"action":"TakedownAccount","actor_did":"did:plc:moderator","cascade_snapshot_ids":null,"cascade_subjects":"[{\"$type\":\"com.atproto.admin.defs#repoRef\",\"did\":\"did:plc:test1234567890abcdef\"}]","event_id":null,"payload":null,"previous_hash":null,"rationale":"spam","sequence":1,"snapshot_id":null,"source":"manual","subject_cid":null,"subject_did":"did:plc:test1234567890abcdef","subject_uri":null,"timestamp":"2026-05-09T00:00:00Z"}
```

**SHA-256**:
```
f51dd8d375762a1e22954eec59af4972efeea5847ff427eaeaee1aaee5ce24ca
```

### Example 2 — `strongRef` Subject (Record)

**Inputs**:
- `sequence: 1`
- `actor_did: "did:plc:moderator"`
- `action: "TakedownRecord"`
- `subject_did: null`
- `subject_uri: "at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc"`
- `subject_cid: "bafyreidemorecord"`
- `rationale: "off-topic"`
- `cascade_subjects`: JSON-encoded string:
  ```
  [{"$type":"com.atproto.repo.strongRef","uri":"at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc","cid":"bafyreidemorecord"}]
  ```
  Note `uri` precedes `cid` here — the production writer uses
  source-declared field order, NOT alphabetical, when serializing
  the embedded Subject struct. The internally-tagged `$type` is
  always emitted first.
- `cascade_snapshot_ids: null`
- `payload: null`, `source: "manual"`
- (others: same as Example 1 — null/absent)

**SHA-256**:
```
16555784f242d5951a46de0ab23d47f0cf061c8651b221900b8995f039e2f9ba
```

### Example 3 — `repoBlobRef` Subject with `record_uri`

**Inputs**:
- `sequence: 1`
- `actor_did: "did:plc:moderator"`
- `action: "TakedownBlob"`
- `subject_did: "did:plc:test1234567890abcdef"`
- `subject_uri: "at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc"`
- `subject_cid: "bafyreidemoblob"`
- `rationale: "csam"`
- `cascade_subjects`: JSON-encoded string (struct order: did,
  cid, record_uri):
  ```
  [{"$type":"com.atproto.admin.defs#repoBlobRef","did":"did:plc:test1234567890abcdef","cid":"bafyreidemoblob","record_uri":"at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc"}]
  ```
- `cascade_snapshot_ids: null`
- `payload: null`, `source: "manual"`
- (others: null)

All three flat subject_* columns are populated. The wire form's
`subjectRef` is `Blob { did, cid, record_uri: Some(<uri>) }`;
the canonical form puts the record URI in `subject_uri`.

**SHA-256**:
```
95a66f39bec9238cca0dd3554de615e222ec6470cf7753cd5068cc5c0591c54e
```

### Example 4 — `repoBlobRef` Subject without `record_uri`

**Inputs**:
- `sequence: 1`
- `actor_did: "did:plc:moderator"`
- `action: "TakedownBlob"`
- `subject_did: "did:plc:test1234567890abcdef"`
- `subject_uri: null`   ← differs from Example 3
- `subject_cid: "bafyreidemoblob"`
- `rationale: "csam-orphan-blob"`
- `cascade_subjects`: JSON-encoded string (no `record_uri` key
  — `skip_serializing_if = "Option::is_none"` drops it):
  ```
  [{"$type":"com.atproto.admin.defs#repoBlobRef","did":"did:plc:test1234567890abcdef","cid":"bafyreidemoblob"}]
  ```
- `cascade_snapshot_ids: null`
- `payload: null`, `source: "manual"`
- (others: null)

The wire form's `subjectRef` is `Blob { did, cid, record_uri:
None }` — when the originating record isn't known, `subject_uri`
is `null` even though the variant is `Blob`. The `record_uri`
key is also absent from the cascade entry.

**SHA-256**:
```
3b9f4b0f5b0c93ba166217f19bfb46ddd1354cf4a74e85bd0810d6e88c39159a
```

### Example 5 — Batch event with cascades

**Inputs**:
- `sequence: 1`
- `actor_did: "did:plc:moderator"`
- `action: "BatchTakedownAccounts"`
- `subject_did: null`, `subject_uri: null`, `subject_cid: null`
  (batch entries don't have a top-level subject — every subject is
  in the cascade)
- `rationale: "coordinated spam network"`
- `snapshot_id: null`, `event_id: null`
- `previous_hash: null`
- `cascade_subjects`: JSON-encoded string of three `repoRef`
  entries:
  ```
  [{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:victim1"},{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:victim2"},{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:victim3"}]
  ```
- `cascade_snapshot_ids`: JSON-encoded string `[7,null,12]` —
  three entries paired by index with `cascade_subjects`. The middle
  subject wasn't snapshottable at decision time.
- `payload: null`, `source: "manual"`

**Canonical JSON**:

```json
{"action":"BatchTakedownAccounts","actor_did":"did:plc:moderator","cascade_snapshot_ids":"[7,null,12]","cascade_subjects":"[{\"$type\":\"com.atproto.admin.defs#repoRef\",\"did\":\"did:plc:victim1\"},{\"$type\":\"com.atproto.admin.defs#repoRef\",\"did\":\"did:plc:victim2\"},{\"$type\":\"com.atproto.admin.defs#repoRef\",\"did\":\"did:plc:victim3\"}]","event_id":null,"payload":null,"previous_hash":null,"rationale":"coordinated spam network","sequence":1,"snapshot_id":null,"source":"manual","subject_cid":null,"subject_did":null,"subject_uri":null,"timestamp":"2026-05-09T00:00:00Z"}
```

**SHA-256**:
```
2f8145772ef1a1972482d1416634921edd358bb1580ca400e7da08c6ea539a3c
```

### Example 6 — Second entry, with `previous_hash` chained from Example 1

**Inputs**:
- `sequence: 2`
- `actor_did: "did:plc:moderator"`
- `action: "RestoreAccount"`
- `subject_did: "did:plc:test1234567890abcdef"`
- `subject_uri: null`, `subject_cid: null`
- `rationale: "appeal granted"`
- `snapshot_id: null`, `event_id: null`
- `previous_hash:
  "f51dd8d375762a1e22954eec59af4972efeea5847ff427eaeaee1aaee5ce24ca"`
  (Example 1's v0.9 `currentHash`)
- `cascade_subjects`: JSON-encoded string (single-subject):
  ```
  [{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:test1234567890abcdef"}]
  ```
- `cascade_snapshot_ids: null`
- `payload: null`, `source: "manual"`

This pins **chain continuity**: Example 6's hash depends on
Example 1's hash via the `previous_hash` field. A consumer that
gets Example 1's hash wrong will also get Example 6's hash wrong
even if every other field is right — exactly the linkage property
the chain provides.

**SHA-256**:
```
95d85237bd7c8e5469d648fa854628bf3ef414c2cd651e614972332754c6b1b3
```

### Example 7 — Substrate-emitted entry with `source` + `payload` *(v0.9)*

The only example varying both new fields, so consumers can confirm
they fold `payload`/`source` into the canonical object at the right
positions. A `moderation_auto_label_applied` entry authored by the
substrate (`did:system`), `source: "auto_label_rule"`, carrying the
action scalar `{"applied":true}`.

**Inputs**:
- `sequence: 1`
- `actor_did: "did:system"`
- `action: "moderation_auto_label_applied"`
- `subject_did: "did:plc:test1234567890abcdef"`
- `subject_uri: null`, `subject_cid: null`
- `rationale: "auto-label rule matched report category"`
- `snapshot_id: null`, `event_id: null`, `previous_hash: null`
- `cascade_subjects`: JSON-encoded string (single-subject):
  ```
  [{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:test1234567890abcdef"}]
  ```
- `cascade_snapshot_ids: null`
- `source: "auto_label_rule"`
- `payload`: the wire object `{"applied":true}` becomes the
  canonical JSON-encoded string `"{\"applied\":true}"`

**Canonical JSON**:

```json
{"action":"moderation_auto_label_applied","actor_did":"did:system","cascade_snapshot_ids":null,"cascade_subjects":"[{\"$type\":\"com.atproto.admin.defs#repoRef\",\"did\":\"did:plc:test1234567890abcdef\"}]","event_id":null,"payload":"{\"applied\":true}","previous_hash":null,"rationale":"auto-label rule matched report category","sequence":1,"snapshot_id":null,"source":"auto_label_rule","subject_cid":null,"subject_did":"did:plc:test1234567890abcdef","subject_uri":null,"timestamp":"2026-05-09T00:00:00Z"}
```

**SHA-256**:
```
168054b81407fe774f080bdc2dfece49183d249f90c20c237c12006e47fb6d6b
```

---

## Out of scope

- **`tools.aurora.admin.exportAccountForensic`** uses an
  independent response shape that drops several fields and emits
  i64 ids as JSON numbers (not stringified). Forensic export
  verification is not covered by this document. Rationalizing the
  two surfaces is future work.
- **`tools.aurora.admin.subscribeModEvents`** shares the
  `AuditEntry` wire shape (verified by an internal parity test)
  but has its own filter set and stream semantics. The per-entry
  hash verification is identical; subscription mechanics are not
  covered here.

## Versioning

A change to the canonical hash-input shape (field additions, field
ordering, representation changes) is a chain-incompatible breaking
change and constitutes a new chain era; consumers update their
verification implementation in lockstep with such a release.

**v0.9 era (chainlink #345).** The `payload` and `source` fields
joined the canonical input, taking the form from 13 to 15 fields.
Rows written before v0.9 hash under the 13-field form; their stored
`currentHash` will not reproduce under the 15-field form, and the
server-side `verified` flag reflects this. This is the expected
chain-era boundary — Aurora-Locus is pre-1.0 and carries no
production chain data across the bump. From v0.9 forward the
15-field form is stable.

The four contract surfaces from
[contract-stability.md](contract-stability.md) commit to additive
evolution of the wire shape. The audit-trail read contract here is
a fifth surface, committed in the doc comment on
`crate::api::aurora_admin::GetAuditTrailOutput`.
