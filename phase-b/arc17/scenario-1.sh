#!/usr/bin/env bash
# Arc 17 Scenario 1 — Regression carry-forward (Arc 12-16f + Arc 13 v4.2).
#
# Confirms cargo coverage hasn't regressed against the production-fetcher
# + AppContext wiring. Failing tests here ABORT Phase B; fix cargo-side
# before touching binaries.
#
# Source-of-record: docs/internal/arc17-phase-b-commands.md Scenario 1.

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

# shellcheck source=../lib/env.sh
source phase-b/lib/env.sh
pb_env_init

# ============================================================
# Block 1 — Scenario-call: cargo test --lib
# ============================================================
echo
echo "[scenario-1] cargo test --lib (lib regression baseline)"
echo "============================================================"
cargo test --lib 2>&1 | tail -3

# ============================================================
# Block 2 — Side-effect-check (operator judges; harness does NOT)
# ============================================================
echo
echo "[scenario-1] decision-point:"
echo "  expected: 'test result: ok. <N> passed; 0 failed; ...'"
echo "  operator: confirm zero failures before continuing to Scenario 2."
echo "  any failing tests here ABORT Phase B — fix cargo-side first."
