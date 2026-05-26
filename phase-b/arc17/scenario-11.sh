#!/usr/bin/env bash
# Arc 17 Scenario 11 — Backend matrix runner (sqlite | postgres).
#
# Runs the chained, state-dependent matrix (Scenarios 2 + 3 + 12 + 6a +
# 6b + 16) in sequence in ONE shell via `source` so A_DID / B_DID /
# A_JWT / B_JWT thread through. Backend-transparent scenarios (9 / 10 /
# 14 / 15) are NOT in the matrix and stay out.
#
# Backend-parameterized via $BACKEND (matches lib/env.sh's convention):
#
#   BACKEND=sqlite   ./phase-b/arc17/scenario-11.sh
#   BACKEND=postgres ./phase-b/arc17/scenario-11.sh
#
# Default is sqlite when unset, mirroring every other scenario. Postgres
# auto-provisions per-role containers via lib/instance.sh::pb_pg_provision
# (idempotent); SQLite skips container provisioning entirely (the data
# dirs self-provision per child scenario).
#
# Full dual-backend coverage = run both invocations. Settled Decision 3
# (no backend carve-out) means both must pass.
#
# Source-of-record: docs/internal/arc17-phase-b-commands.md Scenario 11.
#
# THIS SCRIPT IS A WRAPPER. The matrix DELIBERATELY does NOT call
# pb_fresh_data_dir between child scenarios — chained scenarios share
# state across launches (the bootstrap from one is the precondition for
# the next). This is the same backend-independent behavior under both
# sqlite and postgres.

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"
REPO_ROOT=$(pwd)

# shellcheck source=../lib/env.sh
source phase-b/lib/env.sh
# shellcheck source=../lib/instance.sh
source phase-b/lib/instance.sh

# Resolve $BACKEND default (sqlite) so the script can be invoked with
# or without an explicit BACKEND env. Every child scenario calls
# pb_env_init on entry which honors the same default; we resolve it
# here too so the provisioning gate below + the echo'd block labels
# show the actual backend in use.
: "${BACKEND:=sqlite}"
export BACKEND

# ============================================================
# Block 1 — Auto-provision Postgres containers (postgres backend only)
# ============================================================
echo
echo "[scenario-11] Block 1: auto-provision Postgres containers (BACKEND=$BACKEND)"
echo "============================================================"

if [ "$BACKEND" = "postgres" ]; then
    # The matrix re-runs multi-instance scenarios (scenario-2 needs
    # both A and B), so provision both roles up-front. pb_pg_provision
    # is idempotent — fast no-op if a container is already up. Doing
    # it here lets the matrix fail fast at the top if docker is
    # unavailable rather than part-way through scenario-2's launch.
    # pb_pg_provision needs A_DB_URL / B_DB_URL emitted by pb_env_init,
    # so init env first.
    pb_env_init >/dev/null
    pb_pg_provision a
    pb_pg_provision b
else
    echo "[scenario-11] sqlite backend — no container provisioning needed (data dirs self-provision per child scenario)"
fi

# ============================================================
# Block 2 — Confirm backend + init env
# ============================================================
echo
echo "[scenario-11] Block 2: confirm backend + init env (BACKEND=$BACKEND)"
echo "============================================================"

# Clear A_DID / B_DID / *_JWT so the matrix starts fresh — postgres and
# sqlite runs are separate substrates and shouldn't inherit each other's
# DIDs. scenario-2 re-seeds A; scenario-12 re-seeds B.
unset A_DID A_JWT A_ADMIN_JWT
unset B_DID B_JWT B_ADMIN_JWT

# Re-init env defaults under the resolved BACKEND. Idempotent with the
# Block 1 init under postgres; first init under sqlite.
pb_env_init
pb_env_echo_confirm

# ============================================================
# Block 3 — Re-run the matrix-row scenarios in order
# ============================================================
echo
echo "[scenario-11] Block 3: re-run matrix scenarios (2, 3, 12, 6a, 6b, 16) on $BACKEND"
echo "============================================================"

# Each re-invocation runs in the current shell so env state survives;
# `bash -c` would isolate and lose A_DID / B_DID. We deliberately
# source.
#
# Note: 6a/6b leave B in warn mode; 16 then runs against warn. Same
# sequencing under both backends — backend-independent.
#
# Belt-and-suspenders cd: each child scenario's entry includes
# `cd "$(git rev-parse --show-toplevel)"` so the cwd lands back at
# repo root on entry, but we re-cd here too so a future child that
# changes cwd mid-flight can't break the next iteration's repo-root-
# relative source path. The path `phase-b/arc17/${scn}.sh` resolves
# from $REPO_ROOT, which we re-anchor to before each iteration.

for scn in scenario-2 scenario-3 scenario-12 scenario-6a scenario-6b scenario-16; do
    cd "$REPO_ROOT"
    echo
    echo "============================================================"
    echo "[scenario-11] invoking phase-b/arc17/${scn}.sh under BACKEND=$BACKEND"
    echo "============================================================"
    # shellcheck source=/dev/null
    source "phase-b/arc17/${scn}.sh"
done

echo
echo "[scenario-11] decision-point:"
echo "  expected: each re-invoked scenario produces the SAME pass conditions"
echo "            on $BACKEND as on the other backend (Settled Decision 3:"
echo "            no backend carve-out)."
case "$BACKEND" in
postgres)
    echo "  postgres-specific machinery exercised: apply_writes FOR UPDATE"
    echo "            (scenario-16's validate-imports override), postgres-CAS"
    echo "            substrate, advisory-lock liveness, distributed-bucket"
    echo "            rate-limit path (off under PDS_RATE_LIMITS_ENABLED=false"
    echo "            in Phase B, but the substrate is still wired)."
    ;;
sqlite)
    echo "  sqlite-specific machinery exercised: per-actor store.sqlite"
    echo "            (scenario-16's record-table check), SQLite WAL +"
    echo "            file-flock liveness lock, single-instance account_db"
    echo "            (no maintenance_pool / distributed substrate)."
    ;;
esac
echo "  operator: confirm all six re-runs landed clean before sign-off."
echo "  full dual-backend coverage requires running BOTH:"
echo "    BACKEND=sqlite   ./phase-b/arc17/scenario-11.sh"
echo "    BACKEND=postgres ./phase-b/arc17/scenario-11.sh"
