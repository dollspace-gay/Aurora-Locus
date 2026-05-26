#!/usr/bin/env bash
# Arc 17 Phase B — full-set orchestrator.
#
# One command per backend:
#
#   BACKEND=sqlite   ./phase-b/arc17/run-all.sh
#   BACKEND=postgres ./phase-b/arc17/run-all.sh
#
# Topology (encoded once, here):
#
#   STEP 1 — GATE: cargo test --lib (scenario-1's content). Backend-
#            independent regression baseline. Fail-fast: if cargo test
#            fails, ABORT the whole run; don't launch anything.
#   STEP 2 — MATRIX SIX: scenarios 2 -> 3 -> 12 -> 6a -> 6b -> 16,
#            chained on $BACKEND via sourcing. Delegated to
#            phase-b/arc17/scenario-11.sh (the backend-parameterized
#            matrix runner; commit 2620b35 scenario-11 honor backend).
#            scenario-11 already provisions Postgres containers under
#            BACKEND=postgres and skips that under sqlite; it captures
#            REPO_ROOT and cd's before each child source.
#   STEP 3 — TRANSPARENT FOUR: scenarios 9, 10, 14, 15. Backend-
#            independent (admin endpoints, DNS-log greps, tombstone via
#            mock-PLC, single-flight de-dup — none touch backend-
#            divergent paths). Run ONCE PER SESSION, marker-gated at
#            /tmp/aurora-phase-b/transparent-four.done. On the second
#            invocation (other backend), STEP 3 SKIPS with a loud
#            printed reason + the exact rm command to force a re-run.
#            They source AFTER scenario-11 because they read its
#            chain-exported state (A_DID, B_DID, B_ADMIN_JWT, B_ENV,
#            PDS_DID_PLC_URL).
#   STEP 4 — SCENARIO-13: live-DNS strict-parse. Standalone setup
#            (its own pb_kill_prior + pb_fresh_data_dir b + fresh
#            account); destroys STEP 2's A/B which is why it runs LAST
#            (after STEP 3 has read the matrix state). Per backend.
#
# Setup-only-never-judgment: this script orchestrates the topology;
# it does NOT decide whether scenarios passed. Every scenario's
# decision-point block prints in full for the OPERATOR to read and
# confirm. The orchestrator's exit code reflects setup outcome (gate
# failure, sourcing error) only — NOT scenario semantic correctness.
#
# Reset transparent-four marker (force STEP 3 to run again):
#   rm /tmp/aurora-phase-b/transparent-four.done
#
# Marker survives matrix re-runs: it lives in /tmp/aurora-phase-b/
# root, not the per-role data subdirs that scenario-2 wipes.
#
# State threading: scenario-11 SOURCES its children (not subshells)
# so A_DID / B_DID / B_ADMIN_JWT / B_ENV thread through. The
# orchestrator does the same — every component is sourced so the
# chain-exported state survives into the next step.
#
# cd discipline: REPO_ROOT captured at the top; cd "$REPO_ROOT" before
# each source so a child's internal cd can't break a subsequent source.

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"
REPO_ROOT=$(pwd)

# Resolve $BACKEND default (matches lib/env.sh's convention).
: "${BACKEND:=sqlite}"
export BACKEND

TRANSPARENT_MARKER=/tmp/aurora-phase-b/transparent-four.done
GATE_LOG=/tmp/aurora-pb-run-all-gate.log

# ============================================================
# Banner — topology preview + marker state
# ============================================================
echo
echo "============================================================"
echo "[run-all] arc17 Phase B full-set orchestrator"
echo "[run-all] BACKEND=$BACKEND"
echo "[run-all] topology:"
echo "[run-all]   STEP 1: gate (cargo test --lib)"
echo "[run-all]   STEP 2: matrix six (2, 3, 12, 6a, 6b, 16) via scenario-11 on $BACKEND"
echo "[run-all]   STEP 3: transparent four (9, 10, 14, 15) — once-only via marker"
echo "[run-all]   STEP 4: scenario-13 (live-DNS, standalone) on $BACKEND"
if [ -f "$TRANSPARENT_MARKER" ]; then
    echo "[run-all] NOTE: transparent-four marker EXISTS — STEP 3 will SKIP"
    echo "[run-all]   marker: $TRANSPARENT_MARKER"
    echo "[run-all]   to force STEP 3 to run again: rm $TRANSPARENT_MARKER"
else
    echo "[run-all] NOTE: transparent-four marker ABSENT — STEP 3 will run"
fi
echo "[run-all] setup-only-never-judgment: every scenario's decision-point"
echo "[run-all]   prints in full for the operator to judge. Orchestrator"
echo "[run-all]   automates RUNNING, not deciding-whether-passed."
echo "============================================================"

# ============================================================
# STEP 1 — Gate: cargo test --lib (scenario-1 content)
# ============================================================
echo
echo "============================================================"
echo "[run-all] STEP 1: gate — cargo test --lib"
echo "============================================================"
cd "$REPO_ROOT"

# NOT sourcing scenario-1: its `cargo test ... | tail -3` pipeline
# (pipefail + tail-success) swallows cargo's exit code, and its last
# command is an echo so a `source` would always return 0. Inline the
# gate here so we can read cargo's actual exit via PIPESTATUS[0] and
# ABORT the whole run on regression.

echo "[run-all] running cargo test --lib (warm-cache: ~30-60s; cold: much longer)"
cargo test --lib 2>&1 | tee "$GATE_LOG" | tail -5
GATE_RC=${PIPESTATUS[0]}

echo
echo "[scenario-1] decision-point:"
echo "  expected: 'test result: ok. <N> passed; 0 failed; ...'"
echo "  operator: any failing tests here ABORT Phase B — fix cargo-side first."

if [ "$GATE_RC" -ne 0 ]; then
    echo
    echo "[run-all] ABORT: cargo test --lib failed (rc=$GATE_RC)" >&2
    echo "[run-all]   full log: $GATE_LOG" >&2
    echo "[run-all]   downstream scenarios NOT launched." >&2
    exit 1
fi

# ============================================================
# STEP 2 — Matrix six on $BACKEND (via scenario-11)
# ============================================================
echo
echo "============================================================"
echo "[run-all] STEP 2: matrix six on $BACKEND (delegated to scenario-11)"
echo "============================================================"
cd "$REPO_ROOT"

# Source so scenario-11's chain-exported state (A_DID, A_JWT, A_PORT,
# B_DID, B_JWT, B_ADMIN_JWT, B_ENV, B_PORT, PDS_DID_PLC_URL, etc.)
# threads into the orchestrator's shell for STEP 3's transparent four.
# scenario-11 itself sources its matrix children, so the export chain
# is end-to-end sourced — no subshell loses state.
# shellcheck source=/dev/null
source phase-b/arc17/scenario-11.sh

# ============================================================
# STEP 3 — Transparent four (9, 10, 14, 15), once-only via marker
# ============================================================
echo
echo "============================================================"
echo "[run-all] STEP 3: transparent four (9, 10, 14, 15)"
echo "============================================================"

mkdir -p "$(dirname "$TRANSPARENT_MARKER")"

TRANSPARENT_RAN=no
if [ -f "$TRANSPARENT_MARKER" ]; then
    echo "[run-all] SKIPPED: transparent four already ran this session"
    MARKER_MTIME=$(stat -c '%y' "$TRANSPARENT_MARKER" 2>/dev/null \
        || stat -f '%Sm' "$TRANSPARENT_MARKER" 2>/dev/null \
        || echo unknown)
    echo "[run-all]   marker: $TRANSPARENT_MARKER (created $MARKER_MTIME)"
    echo "[run-all]   to force them to run again on the next invocation:"
    echo "[run-all]     rm $TRANSPARENT_MARKER"
    echo "[run-all]   rationale: 9 / 10 / 14 / 15 are backend-independent"
    echo "[run-all]     (admin endpoints, DNS-log greps, tombstone via mock-PLC,"
    echo "[run-all]     single-flight de-dup — no backend-divergent paths)."
    echo "[run-all]     Running them per-backend would duplicate identical work."
else
    # SUBSHELL each transparent scenario instead of bare source. Reason:
    # scenario-15 ends Block 2 with bare `wait` (no PID args), which
    # waits for ALL of the current shell's background jobs — including
    # mock-plc.py launched earlier via pb_mock_plc_start (which does
    # `python3 mock-plc.py & ; echo $! >pid` AT FUNCTION-TOP-LEVEL, so
    # the bg job lands in the calling shell's job table). mock-plc
    # runs forever, so scenario-15's `wait` would block forever in
    # the orchestrator's shell. A subshell isolates its own job table
    # while inheriting env exports (A_DID / B_DID / B_ADMIN_JWT etc.
    # from the matrix chain), so `wait` only sees the 10 burst curls
    # and returns when they complete. The transparent four are
    # terminal — none of their exports are needed by downstream
    # steps — so isolating them costs nothing. Subshelling all four
    # is defensive: scenario-15 is the only current `wait` consumer,
    # but a future scenario that adds the same pattern wouldn't
    # re-trip the bug.
    for scn in scenario-9 scenario-10 scenario-14 scenario-15; do
        cd "$REPO_ROOT"
        echo
        echo "============================================================"
        echo "[run-all] invoking phase-b/arc17/${scn}.sh (transparent; once-only; subshell-isolated)"
        echo "============================================================"
        # shellcheck source=/dev/null
        ( source "phase-b/arc17/${scn}.sh" )
    done
    # Mark complete so a subsequent run-all invocation (e.g. on the
    # other backend) skips them. Operator removes the marker to re-run.
    : > "$TRANSPARENT_MARKER"
    TRANSPARENT_RAN=yes
    echo
    echo "[run-all] transparent four COMPLETE; marker written at $TRANSPARENT_MARKER"
    echo "[run-all]   subsequent run-all invocations will SKIP STEP 3"
    echo "[run-all]   rm the marker to force them to run again"
fi

# ============================================================
# STEP 4 — scenario-13 on $BACKEND (live-DNS, standalone)
# ============================================================
echo
echo "============================================================"
echo "[run-all] STEP 4: scenario-13 (live-DNS strict-parse) on $BACKEND"
echo "============================================================"
cd "$REPO_ROOT"

# scenario-13 has its own setup (pb_kill_prior + pb_fresh_data_dir b +
# new account "bob.localhost"). It DESTROYS the matrix-launched A/B
# from STEP 2, which is why STEP 4 runs LAST (after STEP 3 has read
# the matrix state). Per backend — backend-divergent in the per-actor
# store path under sqlite vs the pg account_db under postgres.
#
# Subshelled for the same job-table-isolation reason as STEP 3 (also
# defensive: scenario-13 doesn't currently use bare `wait`, but a
# future addition wouldn't re-trip the bug). scenario-13's exports
# aren't needed by anything downstream — only the orchestrator's
# summary follows, and it reads $BACKEND + $TRANSPARENT_RAN only.
# shellcheck source=/dev/null
( source phase-b/arc17/scenario-13.sh )

# ============================================================
# Final — orchestrator summary (what ran, not whether passed)
# ============================================================
echo
echo "============================================================"
echo "[run-all] DONE: orchestrator ran the full topology on BACKEND=$BACKEND"
echo "[run-all]   STEP 1 gate: cargo test --lib passed (rc=0)"
echo "[run-all]   STEP 2 matrix six: 2, 3, 12, 6a, 6b, 16 on $BACKEND"
if [ "$TRANSPARENT_RAN" = "yes" ]; then
    echo "[run-all]   STEP 3 transparent four: 9, 10, 14, 15 ran (marker now set)"
else
    echo "[run-all]   STEP 3 transparent four: SKIPPED (marker pre-existed)"
fi
echo "[run-all]   STEP 4 scenario-13: ran on $BACKEND"
echo "[run-all]"
echo "[run-all] EVERY scenario's decision-point printed above. OPERATOR judges."
echo "[run-all] for full dual-backend coverage, run on the OTHER backend too:"
case "$BACKEND" in
sqlite)
    echo "[run-all]   BACKEND=postgres ./phase-b/arc17/run-all.sh"
    ;;
postgres)
    echo "[run-all]   BACKEND=sqlite ./phase-b/arc17/run-all.sh"
    ;;
esac
echo "============================================================"
