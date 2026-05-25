#!/usr/bin/env bash
# Arc 17 Scenario 10 — did_authority override / DNS-never-hit
# confirmation.
#
# Confirms the override has been the load-bearing knob throughout this
# Phase B: B's log has NO entries from hickory_resolver or from
# lexicon_resolver::resolve_authority_did (the DNS-resolution path).
#
# Source-of-record: docs/internal/arc17-phase-b-commands.md Scenario 10.
# Live-DNS coverage (the inverse — override absent, real DNS exercised)
# is M1.2's Scenario 13, not this one.

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

# shellcheck source=../lib/env.sh
source phase-b/lib/env.sh
pb_env_init
pb_env_echo_confirm

: "${A_DID:?Scenario 2 must run first}"

B_LOG="/tmp/pds-b-${BACKEND}.log"

# ============================================================
# Block 1 — Grep B's log for any DNS-related events
# ============================================================
echo
echo "[scenario-10] Block 1: grep B log for DNS-related lines"
echo "============================================================"
echo "expected: empty output. Any match indicates the override didn't fire."

# Bare event-name greps; NOT 'event: \"...\"' (Pretty subscriber inserts ANSI).
grep -iE 'hickory|resolve_txt|_lexicon\.|DnsTxtResolver' "$B_LOG" | head -5 \
    || echo "(empty — override fired as expected; DNS path never touched)"

# ============================================================
# Block 2 — Confirm every lexicon_fetch_complete has authority_did=A_DID
# ============================================================
echo
echo "[scenario-10] Block 2: indirect override-fired confirmation"
echo "============================================================"

echo "--- last 2 lexicon_fetch_complete lines on B ---"
grep 'lexicon_fetch_complete' "$B_LOG" | tail -2 \
    || echo "(no lexicon_fetch_complete events — Scenario 12 may not have run)"

echo
echo "expected: every event line shows authority_did=${A_DID}"
echo "          (DNS would have resolved to some other DID for a non-override path)"

echo
echo "[scenario-10] decision-point:"
echo "  expected: empty grep for hickory / DnsTxtResolver lines;"
echo "            every lexicon_fetch_complete event has authority_did=${A_DID}."
echo "  operator: confirm before continuing."
