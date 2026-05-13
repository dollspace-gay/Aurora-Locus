# Arc 9 Phase B exercise script

Localhost smoke-test script for Chrys's Phase B sweep of Arc 9
(`chainlink #55` — Hygiene pass). Mirrors the Arc 7/8 convention
at [`arc7-phase-b-commands.md`](arc7-phase-b-commands.md) and
[`arc8-phase-b-commands.md`](arc8-phase-b-commands.md): curl
against `localhost:2583`, `cargo test` for deterministic
test-infra checks, no deployment framing.

Arc 9 is a hygiene pass with heterogeneous surfaces: a forensic-
endpoint wire-shape migration with a `schemaVersion: "2"`
manifest marker (Item 2), a test-clock primitive that makes
`identity::cache` TTL tests deterministic (Item 12), a manual
`Debug` impl on `AppContext` with sensitive-field redaction
(Item 8), ~20 sections of `AURORA_ADMIN_UI_DESIGN.md` prose
historicized (Item 15), a "Per-key value formats" section in
`file-tier-config.md` (Item 19), an audit-date comment on
`validate_config.rs` (Item 17), and the clippy-zero baseline
preserved from Step 1 (Item 7). Phase B exercises the curl-
visible surface (Section A), the test-infra outcomes (Sections
B-C, G), the decoupling discipline (Section H), and the docs
(Sections D-F).

## Prerequisites

- Working dir: `/mnt/d/- - CODING/RUST/aurora-locus`.
- Branch `skydeval/v0.4-cycle` at the Arc 9 Step 4 tip
  (`2190d1d`) or its descendants.
- Free port 2583.
- `curl`, `jq`, `tar`, `diff` on the dev machine.
- `sqlite3` for one optional inspection in Section A.

---

## Setup (one-time per session)

### Start the PDS

```bash
cargo run -- serve
```

Expected log lines (order may vary):

```
Distributed-state substrate initialized (Postgres-CAS) ...
🚀 Aurora Locus PDS listening on 0.0.0.0:2583
```

### Health probe

```bash
curl -s http://localhost:2583/health | jq
```

Expected: `{"status":"ok",...}`.

### Test accounts (skip if local state already has them)

Admin path — create the account, grant SuperAdmin, mint JWT.

> **`grant-admin` CLI signature**: positional `<DID> <ROLE>`
> followed by optional `--notes` and `--force` flags. Verified
> via `cargo run -- grant-admin --help`. Arc 7's and Arc 8's
> scripts had the flag form `--did ... --role ...` from earlier
> drafts; that form does not work against the current binary.

```bash
curl -s -X POST http://localhost:2583/xrpc/com.atproto.server.createAccount \
  -H 'Content-Type: application/json' \
  -d '{"email":"alice@localhost","handle":"alice.localhost","password":"TestPassword123!"}' \
  | jq

cargo run -- grant-admin did:plc:<from-above> superadmin \
  --notes "Arc 9 Phase B sweep"

SESSION=$(curl -s -X POST http://localhost:2583/xrpc/com.atproto.server.createSession \
  -H 'Content-Type: application/json' \
  -d '{"identifier":"alice.localhost","password":"TestPassword123!"}')
export ADMIN_TOKEN=$(echo "$SESSION" | jq -r '.accessJwt')
```

Tokens last ~1 hour; re-mint if a 401 appears mid-sweep.

### Seed an audit chain entry for the forensic exercises

Section A reads `audit-entries.json` from the bundle; it needs
at least one chain entry against the subject. Easiest path:
emit one moderation event against alice via `emitEvent`. If
local state already has audit-chain rows for the account, skip.

```bash
curl -s -X POST http://localhost:2583/xrpc/tools.aurora.admin.emitEvent \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{
    "action": "TakedownAccount",
    "subjects": [{"$type": "com.atproto.admin.defs#repoRef", "did": "did:plc:<alice-did>"}],
    "rationale": "phase-b seed",
    "snapshotCapture": true
  }' | jq '.audit_entry_id'
```

Expected: a stringified i64 (the chain entry id). Confirms a
chain row exists for the Section A export to surface.

---

## Section A — exportAccountForensic v2 wire shape (Item 2)

The load-bearing surface for Arc 9. Verifies the migrated wire
shape matches `getAuditTrail` field-for-field + the manifest
carries the new `schemaVersion`.

`exportAccountForensic` is a **POST** with a JSON body — not a
GET with query params. `rationale` is required and non-empty.
`include_audit_chain: true` requires SuperAdmin role (alice has
it per Setup).

### A1. POST the export, save the TAR

```bash
curl -s -o /tmp/forensic.tar \
  -X POST http://localhost:2583/xrpc/tools.aurora.admin.exportAccountForensic \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{
    "did": "did:plc:<alice-did>",
    "rationale": "phase-b A1 forensic-shape verification",
    "includeRepo": false,
    "includeBlobs": false,
    "includeModerationHistory": false,
    "includeAccountMetadata": false,
    "includeAuditChain": true
  }'

ls -la /tmp/forensic.tar
file /tmp/forensic.tar
```

Expected: `/tmp/forensic.tar` exists with non-zero size; `file`
identifies it as `POSIX tar archive`. If the body is JSON
instead (with an `error` field), the request failed before
bundling — common causes: missing/empty `rationale`, missing
SuperAdmin role on the bearer, or DID mismatch with no
backing account row.

### A2. Unpack the TAR; inspect manifest.json

```bash
mkdir -p /tmp/forensic && tar -xf /tmp/forensic.tar -C /tmp/forensic
ls /tmp/forensic/

jq 'keys' /tmp/forensic/manifest.json
jq '.schemaVersion' /tmp/forensic/manifest.json
```

Expected: `ls` shows at least `manifest.json`,
`account-state.json`, `audit-entries.json`, `audit-trail.json`.
`jq 'keys'` includes `"schemaVersion"`. `jq '.schemaVersion'`
emits the literal string `"2"` (with quotes — it's a string,
not a number).

### A3. Inspect audit-entries.json field membership

```bash
jq '.[0] | keys' /tmp/forensic/audit-entries.json
```

Expected (alphabetized by `jq`):

```json
[
  "action",
  "actorDid",
  "cascadeSnapshotIds",
  "cascadeSubjects",
  "currentHash",
  "eventId",
  "id",
  "previousHash",
  "rationale",
  "sequence",
  "snapshotId",
  "subjectRef",
  "timestamp",
  "verified"
]
```

14 fields total. `subjectRef` may be `null` for the seeded
event if the subject was a Repo variant without a CID column
populated, but the field IS present in the keys list.

### A4. Inspect field types (Step 0 Q8's per-divergence resolution)

```bash
jq '.[0] | {
    id_is_string: (.id | type),
    sequence_is_number: (.sequence | type),
    timestamp_present: (has("timestamp")),
    createdAt_absent: (has("createdAt") | not),
    verified_is_bool: (.verified | type),
    subjectRef_present: (has("subjectRef")),
    snapshotId_is_string_or_null: ((.snapshotId | type) | IN("string","null"))
  }' /tmp/forensic/audit-entries.json
```

Expected:

```json
{
  "id_is_string": "string",
  "sequence_is_number": "number",
  "timestamp_present": true,
  "createdAt_absent": true,
  "verified_is_bool": "boolean",
  "subjectRef_present": true,
  "snapshotId_is_string_or_null": true
}
```

Each line covers a specific Step 0 Q8 divergence: stringified
`id`, renamed `timestamp` (was `createdAt` in v1), present
`verified` + `subjectRef`, stringified `snapshotId`. A `false`
anywhere is a Phase B regression signal.

### A5. Field-for-field parity with getAuditTrail

```bash
curl -s "http://localhost:2583/xrpc/tools.aurora.admin.getAuditTrail?subjectDid=did:plc:<alice-did>" \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  | jq '.items[0]' > /tmp/trail_entry.json

jq '.[0]' /tmp/forensic/audit-entries.json > /tmp/forensic_entry.json

diff <(jq -S . /tmp/trail_entry.json) <(jq -S . /tmp/forensic_entry.json)
echo "exit: $?"
```

Expected: zero-line diff (`echo "exit: 0"`). The forensic
bundle's per-entry shape is byte-identical to `getAuditTrail`'s
`items[0]` after canonical sort. This is the canonical Phase B
gate for the migration; the parity test
`forensic_audit_entries_match_get_audit_trail_shape` asserts
the same property in unit-test scope.

### A6. (Optional) Confirm the SELECT extension

`exportAccountForensic`'s SELECT was extended to fetch the full
`audit_chain_entry` column set. Confirm via the on-disk row:

```bash
sqlite3 data/account.sqlite \
  "SELECT subject_uri, subject_cid, cascade_subjects, cascade_snapshot_ids \
   FROM audit_chain_entry \
   WHERE subject_did = 'did:plc:<alice-did>' \
   ORDER BY sequence DESC LIMIT 1;"
```

Expected: four columns returned. Values may be NULL for the
seeded event but the schema supports them, and the helper
parses them through to `cascadeSubjects` /
`cascadeSnapshotIds` on the wire.

---

## Section B — Test-clock determinism (Item 12)

`cargo test` invocations. Verifies the deterministic-clock
migration of `test_stale_handle_detection` and
`test_stale_did_doc_detection` holds across environments.
Both tests previously flaked under suite load because they
used `tokio::time::sleep` against real-wall-clock TTLs.

### B1. Flakiness loop on test_stale_handle_detection

```bash
for i in $(seq 1 10); do
  cargo test --lib test_stale_handle_detection 2>&1 \
    | grep -E "test result" \
    || echo "FAIL iteration $i"
done
```

Expected: 10 occurrences of
`test result: ok. 1 passed; 0 failed; 0 ignored; ...`. Each
iteration completes in ~50-150 ms (compile-cached) — down from
~5-10 s of wall-clock sleep in the pre-migration shape.

### B2. Flakiness loop on test_stale_did_doc_detection

Same pattern for the sibling test:

```bash
for i in $(seq 1 10); do
  cargo test --lib test_stale_did_doc_detection 2>&1 \
    | grep -E "test result" \
    || echo "FAIL iteration $i"
done
```

Expected: 10/10 pass. The sibling test had the identical race
pattern and was migrated alongside the named test; the loop
confirms both are deterministic.

### B3. Combined run for total runtime sanity

```bash
time cargo test --lib test_stale_ 2>&1 | tail -5
```

Expected: 2 tests pass; total wall-clock under ~250 ms after
the first cached build. The dramatic drop in runtime (from
~22 s combined in the pre-migration shape) is itself the
visible signal that the wall-clock sleeps are gone.

---

## Section C — AppContext Debug redaction (Item 8)

Single cargo-test invocation. Verifies the redaction test
fires and the manual `Debug` impl preserves the redaction
contract documented in §8.4.1 Item 8.

### C1. Redaction-test invocation

```bash
cargo test --lib app_context_debug_redacts_sensitive_fields 2>&1 | tail -5
```

Expected: `test result: ok. 1 passed; 0 failed`. The test
builds an `AppContext` with well-known sentinel secret values
(`test-secret-key-aurora-subscribe-32xx` for `jwt_secret`,
`"a".repeat(64)` for `repo_signing_key`, `"b".repeat(64)` for
`plc_rotation_key`) and asserts none of them appear in
`format!("{:?}", ctx)`. A failure here means a future change
dropped a redaction; investigate the diff against
`src/context.rs`'s `impl Debug for AppContext`.

---

## Section D — `AURORA_ADMIN_UI_DESIGN.md` readability

Observational, not curl-driven — readability is Chrys's
domain per the existing Phase B convention. Read selected
sections to confirm the historicized prose reads cleanly and
that §8.15 + §15 were NOT modified.

### D1. §1 Executive summary historicization

Open `docs/AURORA_ADMIN_UI_DESIGN.md`. Confirm:

- The header block (lines 3-5) reads
  `**Cycle:** v0.2 (with v0.3 + v0.4 additive amendments —
  see §15 for v0.4 specifics)`.
- §1's opening paragraphs frame the doc as v0.2-originating
  with subsequent additive amendments per §15 — not as a
  forward-looking v0.2 spec.
- §1.5 "Reading this document" includes a bullet pointing
  readers at §15.

### D2. §2.1 / §2.2 scope sections

Spot-check 3-4 subsections in §2.1 In scope and §2.2 Not in
scope. Confirm:

- "Ships in v0.2" framings now read as past tense ("shipped in
  v0.2 and persists through v0.4" or similar).
- "v0.3 evaluates" / "v0.3 may add" framings either cross-
  reference §15 (where v0.3/v0.4 absorbed) or point at v0.5+
  candidate tracking (where neither cycle absorbed).

### D3. §8.15 must stay UNCHANGED (Arc 8 Step 4 anchor)

Locate §8.15 `tools.aurora.describeCapabilities` (around line
4519). Confirm the section was NOT touched by Arc 9 Step 3's
audit — Arc 8 Step 4 rewrote §8.15 against the runtime
`RouteRegistry`; that prose stays current.

```bash
git log --oneline --follow docs/AURORA_ADMIN_UI_DESIGN.md | head -10
git log -L "/^## 8.15 /,/^## 8.16 /:docs/AURORA_ADMIN_UI_DESIGN.md" | head -20
```

Expected: most-recent §8.15 edits trace to Arc 8 Step 4
commits (chainlink #54), not Arc 9 Step 3 (chainlink #55).

### D4. §9.5 Post-cycle deferrals retitled

Locate §9.5. Confirm:

- Title reads "Post-cycle deferrals (originally for v0.3;
  carried forward through v0.4)" — not the original "Post-
  cycle (deferred to v0.3)".
- Each bullet cross-references back to §2.2 for current state
  rather than restating the v0.3 forward-list framing.

### D5. §15 v0.4 (Arc 6) migration notes UNCHANGED

Locate §15 (around line 6324). Confirm:

- The §15 prose (§15.1 through §15.8) is byte-identical to
  pre-Arc-9-Step-3 state. Arc 6 wrote this section as the v0.4
  Arc-6-amendments anchor; Arc 9's audit explicitly skipped it.

```bash
git log -L "/^# 15\\./,EOF:docs/AURORA_ADMIN_UI_DESIGN.md" | head -20
```

Expected: most-recent edits to §15 trace to Arc 6 commits,
not Arc 9 Step 3.

### D6. §15.N cross-references resolve

Pick 2-3 cross-references in §7 or §8 of the form "see §15.N"
or `(§15.N)`. Confirm each anchor exists in §15.

```bash
grep -nE "§15\.[0-9]" docs/AURORA_ADMIN_UI_DESIGN.md | head -10
```

For each `§15.N` mention, confirm a matching `## 15.N` heading
exists later in the file.

---

## Section E — `file-tier-config.md` per-key value formats (Item 19)

Observational — read the new section to confirm the table
matches operator expectations.

### E1. Locate the new section

```bash
grep -n "^## Per-key value formats" docs/operator/file-tier-config.md
```

Expected: a single match near the bottom of the file (after
"Security notes").

### E2. Verify the table columns

Open the section. Confirm the table has columns: **Key**,
**JSON type**, **Allowed values**, **Default**, **Notes**.
Confirm two rows: `moderation-mode` and
`moderation-mode-redirect-url`.

### E3. "Adding a new runtime setting" procedure

Confirm the 4-step procedure is followable end-to-end:

1. Append the key constant + add it to `KNOWN_RUNTIME_KEYS` in
   `src/api/aurora_admin.rs`.
2. Add the per-key validation arm to `validate_runtime_value`
   in the same file.
3. Add a default in `default_for_key`.
4. Append a row to the table above documenting the value
   format.

A reader picking a hypothetical new key (e.g.,
`new-setting-key`) should be able to follow the four steps and
land a working implementation without consulting other docs.

### E4. Example consistency with the new table

```bash
grep -A2 "^moderation-mode:" docs/operator/file-tier-config.md
```

Expected: the existing inline YAML example (around line 46)
uses `moderation-mode: reduced` and
`moderation-mode-redirect-url: https://example.org/maintenance`
— both consistent with the new table's allowed-values column.

---

## Section F — `validate_config.rs` audit-date comment (Item 17)

Trivial check — verify the audit-date comment exists.

### F1. Inspect the module doc block

```bash
head -10 src/cli/validate_config.rs
```

Expected: the module doc block contains a line of the form

```
//! Last audited for staleness: <YYYY-MM-DD> (Arc 9 Step 2 / chainlink #55;
```

with the re-audit trigger note ("Re-audit when major auth,
federation, or storage features change."). The date is the
Step 2 session date.

---

## Section G — Regression triggers

Canonical correctness gates. These `cargo test` invocations
are the cycle-wide-baseline confirmations; F1 is the
load-bearing gate.

### G1. Full lib suite

```bash
cargo test --lib
```

Expected: `test result: ok. 951 passed; 0 failed; 0 ignored`.
Step 3 baseline was 949; Step 4 added two forensic-parity
tests for a total of 951.

### G2. Cross-instance integration tests (Arc 7 baseline)

```bash
cargo test --test distributed_substrate_test
```

Expected: `11 passed; 0 failed; 0 ignored`. Arc 9 didn't touch
substrate code; the Arc 7 suite still passes. Prerequisite:
Docker daemon accessible.

### G3. Contract phrase tests

```bash
cargo test --test contract_phrases
```

Expected: `14 passed; 0 failed; 0 ignored`.

### G4. Grant-admin CLI tests

```bash
cargo test --test grant_admin_test
```

Expected: `8 passed; 0 failed; 0 ignored`.

### G5. Clippy zero-error preservation

```bash
cargo clippy --lib --no-deps -- -D warnings
```

Expected: `Finished` with zero errors. Step 1 cleared 24 errors
to 0; Steps 2-4 added new code without reintroducing any.

---

## Section H — Decoupling sweep

Cycle-narrow check against just the Arc 9 diff:

```bash
git diff --name-only d7e6995..HEAD | while read f; do
  [ -f "$f" ] && git grep -in "cairn\|hideaway\|pursuingpeace\|nearhorizon" -- "$f"
done
```

Expected: hits only inside the decoupling-discipline grep
documentation in `docs/AURORA_ADMIN_UI_DESIGN.md` (the doc
literally contains `grep -ri "cairn" static/` etc. as part of
its own self-check). The `horizon` token is excluded from the
cycle-narrow grep because the doc carries English
`horizontal/horizontally` references that pre-date Arc 9.

Cycle-wide `horizon` sweep:

```bash
git grep -i "horizon" -- '*.md' '*.rs' | grep -v "horizontal\|more-horizontal" | head -20
```

Expected: only documented false positives — design-doc self-
references listing the forbidden tokens as decoupling-
discipline criteria (`docs/V0*_DESIGN.md`,
`docs/AURORA_ADMIN_UI_DESIGN.md` decoupling-discipline grep
documentation block), prior Phase B scripts at
`docs/internal/arc*-phase-b-commands.md` listing the same
grep commands and their documented false positives, this file
listing the same.

---

## Section I — Forensic-bundle v1→v2 consumer-side check

Observational — if any scripts/tooling consumed v1 forensic
bundles, verify they handle the v2 shape or fall back via
`schemaVersion` dispatch.

### I1. Internal-only state

If no v1 consumers exist outside Aurora-Locus itself, skip
this section. The `schemaVersion` field exists for future-
proofing; current consumers are all yet-to-be-built.

### I2. v1 consumers (if any)

If v1 consumers exist, run one through the new bundle and
confirm either:

- (a) it handles the new shape natively (some consumers are
  tolerant of extra fields and the renamed `timestamp` /
  stringified `id` are forward-compatible if the consumer
  reads field names that exist in v2 too); or
- (b) it detects the `schemaVersion: "2"` marker and routes
  to a v2-aware code path.

If neither, the consumer needs an update before relying on
Arc-9-era bundles. CHANGELOG.md flags the breaking change so
the upgrade work is discoverable.

---

## Notes

- **Token expiry**: JWT lasts ~1 hour; re-mint via Setup's
  `createSession` curl if a 401 appears mid-sweep.

- **No mode toggles in Arc 9**: unlike Arc 7's
  `PDS_DISTRIBUTED_STATE_MODE`, Arc 9 introduces no env-var-
  driven paths. Single mode of operation; no restart sequence
  is needed beyond the initial `cargo run -- serve`.

- **`exportAccountForensic` is POST, not GET**: takes a JSON
  body with `did`, `rationale` (required, non-empty), and
  several `include*` flags. `include_audit_chain: true`
  requires SuperAdmin role on the bearer. A GET-style query-
  string invocation would fail with a 405 / 415; the script's
  examples all use the correct POST shape.

- **`grant-admin` CLI signature**: positional `<DID> <ROLE>`
  followed by `--notes <NOTES>` and `--force` optional flags.
  Verified via `cargo run -- grant-admin --help`. The Arc 7
  and Arc 8 Phase B scripts contained the wrong flag form
  (`--did ... --role ...`) carried over from earlier-draft
  versions of the CLI; the current binary's argument parser
  rejects that form with a clap error. Use the positional
  form above.

- **Forensic-bundle inspection**: the TAR is binary; use
  `tar -xf` to unpack before inspecting individual files.
  `manifest.json` and `audit-entries.json` are the load-
  bearing files for Arc 9's surface; `account-state.json` +
  `audit-trail.json` carry forward unchanged from prior
  cycles.

- **Test-clock loop expectations**: both
  `test_stale_handle_detection` and
  `test_stale_did_doc_detection` should run in ~50-150 ms
  each (down from ~5-10 s of wall-clock sleep in the
  pre-migration shape). Deterministic across environments.

- **§8.15 and §15 invariance**: Step 3's audit explicitly
  skipped these. Phase B Section D's D3/D5 checks confirm
  they weren't accidentally modified. The `git log -L` form
  is the surgical verification.

- **No DB substrate to inspect (mostly)**: Arc 9 doesn't add
  substrate tables. The one optional `sqlite3` inspection in
  A6 confirms the schema supports the extended
  `audit_chain_entry` column read; everything else is
  curl-driven or `cargo test`-driven.

- **"If something looks off"**: same convention as Arc 6/7/8 —
  document expected vs actual in a Phase B addendum (separate
  file under `docs/internal/`), don't push, drop back to Nova
  for triage. Cycle close depends on a clean Phase B sweep.

---

## Sign-off

Once all sections clear:

1. Document any findings or regressions in a Phase B addendum
   (separate file under `docs/internal/`).
2. If clean, Arc 9 closes; chainlink #55 can be closed.
3. v0.4 cycle close gate: all per-arc Phase B sweeps must
   pass before the cycle-close release work begins.
