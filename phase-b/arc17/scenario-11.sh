#!/usr/bin/env bash
# Arc 17 Scenario 11 — Backend matrix (Postgres re-run).
#
# Re-runs Scenarios 2 + 3 + 12 + 6 + 16 against Postgres. Scenarios 9 /
# 14 / 15 / 10 are backend-transparent and don't need re-running.
#
# Source-of-record: docs/internal/arc17-phase-b-commands.md Scenario 11.
#
# THIS SCRIPT IS A WRAPPER. It assumes the operator has already stood up
# the two Postgres containers (per the markdown) on 5432 + 5433. It
# flips BACKEND=postgres and re-invokes the relevant scenarios in
# sequence. Each invoked scenario re-sources lib/env.sh, which picks up
# BACKEND=postgres and emits the postgres-flavored DB URLs.

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

# shellcheck source=../lib/env.sh
source phase-b/lib/env.sh

# ============================================================
# Block 1 — Preflight: confirm two postgres containers responding
# ============================================================
echo
echo "[scenario-11] Block 1: preflight Postgres container reachability"
echo "============================================================"

if ! command -v psql >/dev/null 2>&1; then
    echo "[scenario-11] psql not on PATH; install postgresql-client to validate" >&2
    echo "  apt: sudo apt-get install -y postgresql-client" >&2
fi

# Probe both containers (5432 = A; 5433 = B).
for port in 5432 5433; do
    if pg_isready -h localhost -p "$port" >/dev/null 2>&1; then
        echo "Postgres reachable on :$port"
    else
        echo "[scenario-11] Postgres NOT reachable on :$port — stand up the container first" >&2
        echo "  docker run -d --name aurora-phase-b-pg-<role> -p $port:5432 \\" >&2
        echo "    -e POSTGRES_USER=aurora -e POSTGRES_PASSWORD=aurora \\" >&2
        echo "    -e POSTGRES_DB=aurora postgres:16" >&2
        exit 1
    fi
done

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
