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
- **Section D** — six worked examples with byte-equal canonical
  forms and the SHA-256 hashes they produce. A consumer
  implementation that reproduces these hashes byte-for-byte is
  correct.

The worked examples in Section D come from the side-script test
at [tests/audit_chain_canonical_verification.rs](../../tests/audit_chain_canonical_verification.rs).
The doc and the side-script must agree; the side-script is the
executable form of Section C.

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
object with these 13 fields. Aurora-Locus's writer constructs the
object via `serde_json::json!({...})`, which is backed by
`serde_json::Map<String, Value>`. Without the `preserve_order`
feature (Aurora-Locus does not enable it), `Map` is a
`BTreeMap<String, Value>`, so **serialized keys come out in
alphabetical order** regardless of source-order in the macro.

Field order in the canonical JSON (alphabetical):

1. `action`
2. `actor_did`
3. `cascade_snapshot_ids`
4. `cascade_subjects`
5. `event_id`
6. `previous_hash`
7. `rationale`
8. `sequence`
9. `snapshot_id`
10. `subject_cid`
11. `subject_did`
12. `subject_uri`
13. `timestamp`

Per-field representation in the canonical input:

| Canonical field | Type | Notes |
|---|---|---|
| `action` | string | Direct copy from wire's `action`. |
| `actor_did` | string | Direct copy from wire's `actorDid`. |
| `cascade_snapshot_ids` | string \| null | **JSON-encoded string** of the array (or `null` when empty); see Section C for the wire→canonical conversion. |
| `cascade_subjects` | string \| null | **JSON-encoded string** of the array (or `null` when empty); see Section C. |
| `event_id` | number \| null | **Numeric i64**, NOT the wire's stringified form. Convert from wire-string back to number before constructing the canonical input. |
| `previous_hash` | string \| null | Direct copy. `null` for the genesis row. **Hashed inside the canonical object**, not via the textbook prefix-concat form. |
| `rationale` | string | Direct copy. |
| `sequence` | number | i64 as JSON number. |
| `snapshot_id` | number \| null | **Numeric i64**, NOT the wire's stringified form. Convert before hashing. |
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

**Canonical JSON** (alphabetical key order, no whitespace):

```json
{"action":"TakedownAccount","actor_did":"did:plc:moderator","cascade_snapshot_ids":null,"cascade_subjects":"[{\"$type\":\"com.atproto.admin.defs#repoRef\",\"did\":\"did:plc:test1234567890abcdef\"}]","event_id":null,"previous_hash":null,"rationale":"spam","sequence":1,"snapshot_id":null,"subject_cid":null,"subject_did":"did:plc:test1234567890abcdef","subject_uri":null,"timestamp":"2026-05-09T00:00:00Z"}
```

**SHA-256**:
```
3e5c0aca41c91b941e7382218fb063be599a9f50266aee7a188991096a2450bc
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
- (others: same as Example 1 — null/absent)

**SHA-256**:
```
5815a391b016fd4ac25f5ec6070a136971f5c91c93d145fecf3615fdebae1f20
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
- (others: null)

All three flat subject_* columns are populated. The wire form's
`subjectRef` is `Blob { did, cid, record_uri: Some(<uri>) }`;
the canonical form puts the record URI in `subject_uri`.

**SHA-256**:
```
39ec7c56b387f34c36798f7165a538b588581419bd51f6e9c8b09bbd92def49a
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
- (others: null)

The wire form's `subjectRef` is `Blob { did, cid, record_uri:
None }` — when the originating record isn't known, `subject_uri`
is `null` even though the variant is `Blob`. The `record_uri`
key is also absent from the cascade entry.

**SHA-256**:
```
2b8e88caa44c1b4fefe3f7790dbf8161a50c1f0c1298ec678c0a47641254a842
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

**Canonical JSON**:

```json
{"action":"BatchTakedownAccounts","actor_did":"did:plc:moderator","cascade_snapshot_ids":"[7,null,12]","cascade_subjects":"[{\"$type\":\"com.atproto.admin.defs#repoRef\",\"did\":\"did:plc:victim1\"},{\"$type\":\"com.atproto.admin.defs#repoRef\",\"did\":\"did:plc:victim2\"},{\"$type\":\"com.atproto.admin.defs#repoRef\",\"did\":\"did:plc:victim3\"}]","event_id":null,"previous_hash":null,"rationale":"coordinated spam network","sequence":1,"snapshot_id":null,"subject_cid":null,"subject_did":null,"subject_uri":null,"timestamp":"2026-05-09T00:00:00Z"}
```

**SHA-256**:
```
f0465eef6ef0318ae497e97d5fe7adf76143090f577266bfd812e6b7f4739d27
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
  "3e5c0aca41c91b941e7382218fb063be599a9f50266aee7a188991096a2450bc"`
  (Example 1's `currentHash` — cascade-populated shape)
- `cascade_subjects`: JSON-encoded string (single-subject):
  ```
  [{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:test1234567890abcdef"}]
  ```
- `cascade_snapshot_ids: null`

This pins **chain continuity**: Example 6's hash depends on
Example 1's hash via the `previous_hash` field. A consumer that
gets Example 1's hash wrong will also get Example 6's hash wrong
even if every other field is right — exactly the linkage property
the chain provides.

**SHA-256**:
```
92ad90ef72ec8b8af22478c325ed8b3514166169fd2f244279dc47138b9c43b7
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

This contract is stable. Any future change to the canonical
hash-input shape (field additions, field ordering, representation
changes) is a chain-incompatible breaking change
and would constitute a new chain era; consumers should expect to
update their verification implementation in lockstep with such a
release.

The four contract surfaces from
[contract-stability.md](contract-stability.md) commit to additive
evolution of the wire shape. The audit-trail read contract here is
a fifth surface, committed in the doc comment on
`crate::api::aurora_admin::GetAuditTrailOutput`.
