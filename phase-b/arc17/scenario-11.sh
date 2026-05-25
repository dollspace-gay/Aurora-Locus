#!/usr/bin/env bash
# Arc 17 Scenario 11 — Backend matrix (Postgres re-run).
#
# Re-runs Scenarios 2 + 3 + 12 + 6 + 16 against Postgres. Scenarios 9 /
# 14 / 15 / 10 are backend-transparent and don't need re-running.
#
# Source-of-record: docs/internal/arc17-phase-b-commands.md Scenario 11.
#
# THIS SCRIPT IS A WRAPPER. It flips BACKEND=postgres, auto-provisions
# the two Postgres containers (via lib/instance.sh::pb_pg_provision),
# and re-invokes the relevant scenarios in sequence. Each invoked
# scenario re-sources lib/env.sh, which picks up BACKEND=postgres and
# emits the postgres-flavored DB URLs. The matrix DELIBERATELY does
# NOT call pb_fresh_data_dir between child scenarios — chained
# scenarios share state across launches (the bootstrap from one is
# the precondition for the next), matching the SQLite matrix behavior.

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

# shellcheck source=../lib/env.sh
source phase-b/lib/env.sh
# shellcheck source=../lib/instance.sh
source phase-b/lib/instance.sh

# ============================================================
# Block 1 — Auto-provision both Postgres containers up-front
# ============================================================
echo
echo "[scenario-11] Block 1: auto-provision Postgres containers"
echo "============================================================"

# The matrix re-runs multi-instance scenarios (2 needs both A and B),
# so provision both roles up-front. pb_pg_provision is idempotent —
# fast no-op if a container is already up. Doing it here lets the
# matrix fail fast at the top if docker is unavailable rather than
# part-way through scenario-2's launch.
export BACKEND=postgres
pb_env_init >/dev/null
pb_pg_provision a
pb_pg_provision b

# ============================================================
# Block 2 — Flip BACKEND; reset role env vars so re-emission picks up
# Postgres URLs.
# ============================================================
echo
echo "[scenario-11] Block 2: flip BACKEND=postgres"
echo "============================================================"

export BACKEND=postgres

# Clear A_DID/B_DID so the re-run starts fresh (Postgres is a separate
# substrate; the SQLite-run DIDs don't apply).
unset A_DID A_JWT A_ADMIN_JWT
unset B_DID B_JWT B_ADMIN_JWT

# Re-init env defaults under BACKEND=postgres.
pb_env_init
pb_env_echo_confirm

# ============================================================
# Block 3 — Re-run the matrix-row scenarios in order
# ============================================================
echo
echo "[scenario-11] Block 3: re-run matrix scenarios (2, 3, 12, 6a, 6b, 16) on Postgres"
echo "============================================================"

# Each re-invocation runs in the current shell so env state survives;
# uses `bash -c` would isolate and lose state. We deliberately source.

# Note: 6a/6b leave B in warn mode; 16 then runs against warn. Same as
# the SQLite sequence.

for scn in scenario-2 scenario-3 scenario-12 scenario-6a scenario-6b scenario-16; do
    echo
    echo "============================================================"
    echo "[scenario-11] invoking phase-b/arc17/${scn}.sh under BACKEND=postgres"
    echo "============================================================"
    # shellcheck source=/dev/null
    source "phase-b/arc17/${scn}.sh"
done

echo
echo "[scenario-11] decision-point:"
echo "  expected: each re-invoked scenario produces the SAME pass conditions on"
echo "            Postgres as on SQLite. Postgres-specific machinery (apply_writes"
echo "            FOR UPDATE) is exercised by the Scenario 16 re-run."
echo "  operator: confirm all six re-runs landed clean before sign-off."
