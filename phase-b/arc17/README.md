# Arc 17 Phase B — scenario suite

Live-binary verification for the Arc 17 lexicon-resolution / federation work.
Each scenario launches real `aurora-locus` instance(s), drives XRPC against
them, and prints a decision-point block describing what to confirm. **The
harness sets up and runs; the operator judges.** Nothing here auto-asserts a
pass — a green checkmark that doesn't mean what it claims is worse than an
honest "not yet verified," so every scenario surfaces its expected-vs-actual
for a human to read.

## Running everything

```
BACKEND=sqlite   ./run-all.sh
BACKEND=postgres ./run-all.sh
```

`run-all.sh` is the orchestrator. One command per backend runs the whole suite
in the right order with state threaded between dependent scenarios. It does NOT
decide whether scenarios passed — it runs them and prints every decision-point
for you to confirm. Each run takes ~13-15 min (lots of instance launches /
restarts).

What it does, in order:

1. **GATE** — `cargo test --lib`. If the lib tests fail, the whole run aborts
   before launching anything. (Note: a flaky lib test will occasionally
   false-abort here — re-run. The gate fails *closed*, which is the safe
   direction.)
2. **MATRIX SIX** — scenarios 2 → 3 → 12 → 6a → 6b → 16, chained on `$BACKEND`
   (delegated to `scenario-11.sh`). State (`A_DID`, `B_DID`, `TARGET_NSID`, …)
   threads through the chain.
3. **TRANSPARENT FOUR** — scenarios 9, 10, 14, 15. Run **once per session**
   (marker-gated; see below), after the matrix because they read matrix-seeded
   state.
4. **SCENARIO-13** — live-DNS, standalone (own setup). Runs on `$BACKEND`.

### The transparent-four marker

The transparent four (9/10/14/15) are backend-independent — running them twice
(once per backend) would just repeat identical work. So after they run,
`run-all.sh` writes a marker:

```
/tmp/aurora-phase-b/transparent-four.done
```

On the next invocation (e.g. the other backend), STEP 3 sees the marker and
SKIPS, printing why. The banner at the top of every run states whether the
marker exists. To force them to run again (e.g. a fresh full pass):

```
rm /tmp/aurora-phase-b/transparent-four.done
```

So a typical full dual-backend session is:

```
rm -f /tmp/aurora-phase-b/transparent-four.done   # start clean
BACKEND=sqlite   ./run-all.sh                      # gate + matrix + transparent four + 13
BACKEND=postgres ./run-all.sh                      # gate + matrix + 13 (transparent four skipped)
```

## Running scenarios individually

`scenario-11.sh` runs just the matrix six, chained, on either backend:

```
BACKEND=sqlite   ./scenario-11.sh
BACKEND=postgres ./scenario-11.sh
```

Standalone scenarios (1, 13) can be run directly:

```
BACKEND=sqlite ./scenario-1.sh
BACKEND=sqlite ./scenario-13.sh
```

The **chained** scenarios (2/3/12/6a/6b/16) and the **transparent four**
(9/10/14/15) depend on state seeded earlier in the sequence, so running them
as standalone `./scenario-N.sh` will fail on an unbound `A_DID`/`B_DID`. Run
them via `scenario-11.sh` (matrix) or `run-all.sh` (full suite), which `source`
the scenarios in one shell so state persists. (Sourcing matters: a subshell
`./scenario-N.sh` loses the exported DIDs.)

## Backend coverage rule

Settled Decision 3: no single-backend carve-out for backend-divergent paths.
The matrix six and scenario-13 run on **both** `sqlite` and `postgres` (they
touch backend-divergent DB-write paths — placeholder syntax, bool encode/decode,
`apply_writes FOR UPDATE`, per-actor `store.sqlite` vs pg `account_db`). The
transparent four are backend-independent and run **once**.

## Scenario reference

| # | Name | What it proves | Backend | Setup |
|---|---|---|---|---|
| 1 | Regression baseline | `cargo test --lib` is green before any live testing | independent | standalone (gate) |
| 2 | Bootstrap | A + B launch; B has the lexicon resolver wired (`enabled=true`), A doesn't (`enabled=false`) | both | chained (seeds `A_DID`) |
| 3 | Host lexicon record | A serves a lexicon schema record; readback CAR is non-trivial | both | chained (seeds `TARGET_NSID`) |
| 12 | Two-instance federation (canonical) | B resolves an unknown NSID by fetching from A; cache populated; second write hits cache (no re-fetch) | both | chained |
| 6a | HardFail behavior | A unreachable + `fetch_failure_behavior=hard_fail` → 502 `LexiconFetchFailed`, `failure_class=pds_unreachable` | both | chained |
| 6b | Warn behavior | A unreachable + `fetch_failure_behavior=warn` → 200 (Optimistic accept) + both `lexicon_fetch_failed` and `…_warn_fallback` events | both | chained |
| 16 | validate_imports override | Optimistic mode absorbs a schema violation (200 + "accepting in Optimistic mode"); `validate_imports=false` bypasses the validator | both | chained |
| 9 | Admin endpoints + auth gate order | getCacheState / evictCache / fetchNow behave; auth gate fires first: no-auth 401, plain JWT 403, admin-on-disabled 503 `LexiconDisabled` | once | transparent (needs matrix state) |
| 10 | did_authority override (DNS never hit) | With an override configured, DNS path is never touched; every fetch shows the override `authority_did` | once | transparent |
| 14 | Tombstoned authority | mock-PLC returns 410 for a tombstoned DID → B write 502 `LexiconAuthorityTombstoned`, `failure_class=authority_tombstoned` | once | transparent |
| 15 | Single-flight de-dup | N=10 concurrent writes for one uncached NSID → exactly **1** fetch (`fetch_attempts` delta=1), not N | once | transparent |
| 13 | Live-DNS strict-parse | Real DNS TXT resolution: 13a/13b two records → 502 `LexiconAuthorityAmbiguous`; 13c malformed (uppercase/whitespace) → 502 `LexiconFetchFailed`/`dns_fail`. Confirms `lexicon_dns_lookup` fired ×3, override absent, no cache rows for rejected NSIDs | both | standalone (own setup; runs last) |

## Notes for verifying

- **Rate limiting is OFF in Phase B** — the harness emits
  `PDS_RATE_LIMITS_ENABLED=false`, so no scenario should ever see a 429. A 429
  appearing is a finding, not expected.
- **The SQLite cache lives in `account.sqlite`** (not `did_cache.sqlite` — that
  copy of `lexicon_cache` is tautologically empty). Per-actor records live in
  `actors/<shard>/<safe_did>/store.sqlite` (shard is hash-derived; find via
  `find … -name store.sqlite`). The per-actor store is SQLite even under
  `BACKEND=postgres`.
- **Postgres banner check:** the data-dir half of scenario-2's banner check
  reports FAILED under Postgres — the data lives in the container, not the data
  dir, so the SQLite-shaped check legitimately doesn't match. The instance is
  healthy; this is a cosmetic check mismatch, not a scenario failure.
- **Forensic-log greps use bare event names** (`lexicon_dns_lookup`, not
  `event: "lexicon_dns_lookup"`) — the pretty subscriber inserts ANSI codes
  that break quoted patterns.
- The harness manages the Postgres containers (`aurora-phase-b-pg-{a,b}`):
  start-if-absent, wipe-and-reuse per run. They can be left running between
  sessions.
