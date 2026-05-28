# Phase B operator harness

Operator-driven Phase B substrate for Aurora-Locus. The scripts in this
tree mechanize the per-arc operator runs that were previously
hand-executed against the `docs/internal/arc*-phase-b-commands.md`
markdown source-of-record.

## Hard boundary — setup only, never judgment

The harness automates everything **up to** the assertion. It does NOT
assert pass/fail and does NOT collapse a scenario to a green check.
Side-effect-check blocks PRINT expected-vs-actual values, hand the
operator the inspect commands, and end at a printed decision-point
prompt. The operator reads each block, makes the judgment call against
the running binary's actual behavior, and moves on.

Setup-failure non-zero exits (process didn't launch, JWT came back
null, mock-PLC didn't reach ready) are correct and wanted — that's
setup, not judgment. A script printing `PASS` / `FAIL` against a
*semantic* assertion would cross the boundary.

Verification judgment stays the operator's against real running
binaries.

## Layout

```
phase-b/
├── lib/                       # shared setup substrate
│   ├── env.sh                 # source-once env helpers (BACKEND-driven)
│   ├── instance.sh            # cargo-run launch / wait-for-ready / kill
│   ├── data.sh                # fresh-data-dir helpers
│   ├── mock-plc.sh            # OPERATOR-harness launcher for mock-plc.py
│   └── creds.sh               # account seed + DID/JWT echo-confirm
├── mock-plc.py                # mock-PLC HTTP service (shared script;
│                              # CI launches it directly without lib/mock-plc.sh)
├── arc17/                     # Arc 17 scenarios mechanized to runnable form
│   ├── scenario-1.sh          # cargo lib regression baseline
│   ├── scenario-2.sh          # bootstrap two instances + seed A
│   ├── scenario-3.sh          # host lexicon record on A
│   ├── scenario-6a.sh         # HardFail fetch-failure rejection
│   ├── scenario-6b.sh         # Warn fetch-failure accept-with-warn
│   ├── scenario-9.sh          # admin endpoints + auth-FIRST gate order
│   ├── scenario-10.sh         # did_authority override / DNS-never-hit
│   ├── scenario-11.sh         # backend-matrix wrapper (Postgres re-run)
│   ├── scenario-12.sh         # canonical two-instance federation
│   ├── scenario-14.sh         # tombstoned authority (Arc 17 classification)
│   ├── scenario-15.sh         # single-flight de-dup
│   └── scenario-16.sh         # validate_imports override (Optimistic absorb)
└── README.md                  # this file
```

`scenario-13.sh` (live-DNS) is Cluster 1 Member 1.2's contribution —
not in this M1.1 substrate. Other arcs (12 / 13 / 14 / 15 / 16a-f)
migrate from their respective markdown when their consuming cluster
needs them; the markdown remains source-of-record until then.

## Run order (Arc 17)

The canonical sequence is:

```
scenario-1  →  scenario-2  →  scenario-3
            →  scenario-12  →  scenario-9
            →  scenario-6a  →  scenario-6b
            →  scenario-14
            →  scenario-15
            →  scenario-16
            →  scenario-10
            →  scenario-11   (Postgres re-run; optional, controlled by BACKEND)
```

Scenarios 2 and 12 are the load-bearing setup; later scenarios assume
their env (`A_DID`, `B_DID`, `B_ADMIN_JWT`, etc.) is live in the
operator's shell. Re-source the per-role env files (`A_ENV`, `B_ENV`)
at every block entry — they're idempotent and the env-drift guard for
terminal restarts.

## Conventions (locked; do not change inside scenarios)

- **Redirect, NEVER tee.** `cargo run --bin aurora-locus > /tmp/pds-<role>-<backend>.log 2>&1 &` —
  tee'd ANSI from `fmt::Layer.pretty()` breaks quoted greps. The
  `pb_launch_instance` helper bakes this in. The `--bin aurora-locus`
  selector is also load-bearing once a second `[[bin]]` lives in
  `Cargo.toml` (Cluster 1 added `phase-b-dns-responder`); bare
  `cargo run` errors out as ambiguous.
- **NEVER `--release`.** Phase B exercises debug-built behavior
  including `debug!` emission.
- **Ready probe = `describeServer`, NOT `/xrpc/_health`.** The PDS
  `/xrpc/_health` returns 404. The harness `pb_wait_for_ready` polls
  `describeServer`. NB: `mock-plc.py`'s own
  `/_health` is a *different service* on a *different port* (mock-PLC
  on `:$MOCK_PLC_PORT`, PDS on `:$PDS_SERVICE_PORT`) and DOES work —
  this is what the mock-PLC liveness probe in `pb_mock_plc_wait` uses.
- **Bare-event greps.** Pretty subscriber inserts ANSI; a quoted
  `event: "lexicon_fetch_complete"` pattern would miss
  `event\033[…m: \033[…m"lexicon_fetch_complete"`. Side-effect-check
  blocks grep on bare event names (e.g. `lexicon_fetch_complete`).
- **Fresh data dirs per scenario** that asserts a side-effect.
  Content-addressed IDs cross-contaminate across scenarios otherwise.
  `pb_fresh_data_dir <role>` is `rm -rf` + `mkdir -p`, NOT a soft reset.
- **Block-by-block re-runnability.** Each block starts with a re-source
  of the role env files and an echo-confirm of the load-bearing vars.
  Operator can re-run any single block at any point without restarting
  the whole scenario from the top.
- **Env-drift guard at block entry.** Every block re-sources `$A_ENV` /
  `$B_ENV` and calls `pb_env_echo_confirm` to print the resolved
  `BACKEND` / `A_PORT` / `A_DATA` / `B_PORT` / `B_DATA` / `PLC` values
  before running anything that depends on them.
- **JWT-non-null guard at the source.** `pb_create_account` requires
  the returned `accessJwt` length ≥ 250 (a literal `"null"` string is
  length 4; catching it at creation prevents the cascade into
  unrelated-looking auth failures several steps later).

## BACKEND selection

```
BACKEND=sqlite ./phase-b/arc17/scenario-2.sh
BACKEND=postgres ./phase-b/arc17/scenario-2.sh
```

Default is `sqlite` when unset. `lib/env.sh` resolves per-backend DB
URLs and container handles. Both backends auto-provision their state
substrate so a Postgres run is as frictionless as a SQLite run — no
manual `docker run` step before `BACKEND=postgres`.

### Postgres backend (auto-provisioned)

Under `BACKEND=postgres`, `lib/instance.sh::pb_pg_provision` ensures
the role's container (`aurora-phase-b-pg-a` on port 5432, or
`aurora-phase-b-pg-b` on port 5433) is up and reachable before
launching the PDS. The function is idempotent:

- If the container is already up + reachable: fast probe-only no-op.
- If it exists but is stopped: `docker start`, then wait for ready.
- If it doesn't exist: `docker run -d --name ... -p ...:5432 -e ...
  postgres:16`, then wait for ready (bounded 30s).
- If docker is genuinely unavailable: fails fast with the exact
  docker incantation the operator can run by hand, rather than letting
  the PDS spin to `PoolTimedOut` after 60s.

Clean-state isolation is orthogonal and follows the SQLite pattern.
`lib/data.sh::pb_fresh_data_dir <role>` is the scenario-driven "clean
slate for role X" primitive; under Postgres it wipes the role's
schema via `DROP SCHEMA public CASCADE; CREATE SCHEMA public
AUTHORIZATION aurora;` (audited safe — pg migrations are pure
tables/indexes; the next PDS launch re-runs all migrations via
`src/db/mod.rs::run_any_migrations`). Scenarios that want a clean
slate call `pb_fresh_data_dir <role>` regardless of backend and get
backend-symmetric isolation. Matrix scenarios that intentionally
share state across launches (e.g. `scenario-11.sh` chaining 2 → 3 →
12 → 6a → 6b → 16) skip the call, same as under SQLite.

Containers persist between runs by default (cheap to wipe in place —
matters when iterating). The operator can `docker rm -f
aurora-phase-b-pg-{a,b}` to force a full recreate; the next harness
invocation re-runs the docker incantation transparently.

The harness-managed lifecycle is a deliberate revision of the v0.5
"operator-side provisioning" convention. The old assumption was
inherited from the markdown scripts without recorded rationale and
left SQLite ergonomically favorable, quietly biasing operators toward
sqlite-only runs and undermining the "always run twice, no backend
carve-out" discipline.

## CI vs operator harness

The CI postgres-tests job launches `phase-b/mock-plc.py` directly via
the workflow yaml — it does NOT consume `lib/mock-plc.sh`. Same script,
different launcher, different assertion philosophy. The operator
harness hands judgment to the operator; CI asserts pass/fail
automatically. This separation is deliberate — routing CI through the
harness would erode the setup-vs-judgment boundary.

## Mock-PLC requirements

`mock-plc.py` requires Python 3.8+ and the `cryptography` package (>=3.0
with SECP256K1 support). On Debian/Ubuntu:

```
sudo apt-get install -y python3 python3-cryptography
```

Or via pip:

```
pip install cryptography
```

`lib/mock-plc.sh` is the operator-harness launcher; the CI yaml installs
`python3-cryptography` directly. (Per the v0.6 M1.4 drift finding: the
v0.5-era claim that mock-plc.py was stdlib-only was incorrect — it
imports `cryptography` for ECDSA signature verification.)

## Source-of-record

`docs/internal/arc*-phase-b-commands.md` remains the authoritative
description of what each scenario verifies. The scripts in this tree
mechanize the *runs*. When the operator wants to understand a
scenario's intent, the markdown is the canonical reference; the script
is the executable form of its commands.
