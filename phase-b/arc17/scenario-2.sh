#!/usr/bin/env bash
# Arc 17 Scenario 2 — Bootstrap: launch both instances + seed A's account.
#
# Wipes per-role data dirs, writes A's env, launches A, waits for A ready,
# creates the seed account, captures DID/JWT, writes B's env with DID
# injected, launches B, waits for B ready + lexicon-resolver-wired event.
#
# Source-of-record: docs/internal/arc17-phase-b-commands.md Scenario 2.

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

# shellcheck source=../lib/env.sh
source phase-b/lib/env.sh
# shellcheck source=../lib/instance.sh
source phase-b/lib/instance.sh
# shellcheck source=../lib/data.sh
source phase-b/lib/data.sh
# shellcheck source=../lib/mock-plc.sh
source phase-b/lib/mock-plc.sh
# shellcheck source=../lib/creds.sh
source phase-b/lib/creds.sh

pb_env_init
pb_env_echo_confirm

# ============================================================
# Block 1 — Setup-to-confirmed-up (ends at credential echo)
# ============================================================
echo
echo "[scenario-2] Block 1: setup-to-confirmed-up"
echo "============================================================"

# Mock PLC up (idempotent — no-op if already running).
pb_mock_plc_start
pb_mock_plc_wait

# Kill stale instances; fresh data dirs.
pb_kill_prior
pb_fresh_data_dir a
pb_fresh_data_dir b

# A is LEXICON-HOST (enabled=false). Export scenario-set overrides so
# pb_env_emit_role picks them up.
export PDS_LEXICON_ENABLED=false
unset PDS_LEXICON_DID_AUTHORITY  # A doesn't run an override.
pb_env_emit_role a

# Launch A.
pb_launch_instance a
pb_wait_for_ready a
pb_grep_banner a

# Seed A's account.
pb_create_account a "alice.localhost" "alice@localhost" "phase-b-arc17-pw"
pb_echo_creds a

# B is LEXICON-CONSUMER (enabled=true; did_authority = A_DID).
export PDS_LEXICON_ENABLED=true
export PDS_LEXICON_DID_AUTHORITY="${A_DID}"
export PDS_LEXICON_FETCH_TIMEOUT_SECS=10
pb_env_emit_role b

# Launch B.
pb_launch_instance b
pb_wait_for_ready b
pb_grep_banner b

# ============================================================
# Block 2 — Side-effect-check (operator judges)
# ============================================================
echo
echo "[scenario-2] Block 2: side-effect-check"
echo "============================================================"

A_LOG="/tmp/pds-a-${BACKEND}.log"
B_LOG="/tmp/pds-b-${BACKEND}.log"

echo "--- A startup banner (expected: 'listening on' with port ${A_PORT}) ---"
grep -m1 'listening on' "$A_LOG" || echo "(banner line not yet emitted)"

echo "--- B startup banner (expected: 'listening on' with port ${B_PORT}) ---"
grep -m1 'listening on' "$B_LOG" || echo "(banner line not yet emitted)"

echo "--- B lexicon-resolver-wired event (expected: PRESENT) ---"
grep -m1 'lexicon resolver wired' "$B_LOG" || echo "(line not found; B may not have lexicon enabled)"

echo "--- A lexicon-resolver-wired event (expected: ABSENT — A has enabled=false) ---"
grep -m1 'lexicon resolver wired' "$A_LOG" \
    && echo "(WARN: lexicon-resolver-wired emitted on A despite enabled=false)" \
    || echo "(absent, as expected)"

echo
echo "[scenario-2] decision-point:"
echo "  A_DID  = ${A_DID:-<unset>}"
echo "  A_JWT  length = ${#A_JWT}"
echo "  A ready on :${A_PORT}, B ready on :${B_PORT}"
echo "  operator: confirm both banners + B lexicon-resolver-wired before continuing."
