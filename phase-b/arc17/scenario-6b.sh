#!/usr/bin/env bash
# Arc 17 Scenario 6b — Warn: record accepted with WARN log.
#
# Flips PDS_LEXICON_FETCH_FAILURE_BEHAVIOR to warn, kills A, writes a
# new-NSID record; expects HTTP 200 (Optimistic accept) plus
# lexicon_fetch_failed + lexicon_fetch_failed_warn_fallback events.
#
# Source-of-record: docs/internal/arc17-phase-b-commands.md Scenario 6b.

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

# shellcheck source=../lib/env.sh
source phase-b/lib/env.sh
# shellcheck source=../lib/instance.sh
source phase-b/lib/instance.sh
pb_env_init
pb_env_echo_confirm

: "${A_PORT:?A_PORT must be set}"
: "${B_PORT:?B_PORT must be set}"
: "${B_DID:?Scenario 12 must run first}"
: "${B_JWT:?Scenario 12 must run first}"
: "${B_ENV:?Scenario 2 must run first}"

B_LOG="/tmp/pds-b-${BACKEND}.log"

# ============================================================
# Block 1 — Flip B env to warn; restart B; kill A again
# ============================================================
echo
echo "[scenario-6b] Block 1: B env warn; B restart; A kill"
echo "============================================================"

# Flip hard_fail -> warn (idempotent).
if grep -q "^export PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=" "$B_ENV"; then
    sed -i 's/^export PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=.*/export PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=warn/' "$B_ENV"
else
    echo "export PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=warn" >> "$B_ENV"
fi

pb_kill_instance b
pb_launch_instance b
pb_wait_for_ready b

pb_kill_instance a
sleep 2

# ============================================================
# Block 2 — B writes a record of a new NSID; expect 200 (Optimistic)
# ============================================================
echo
echo "[scenario-6b] Block 2: B createRecord against new NSID (expect 200)"
echo "============================================================"

NEW_NSID="com.example.lexicon.warn6b"
WRITE_STATUS=$(curl -sX POST "http://localhost:${B_PORT}/xrpc/com.atproto.repo.createRecord" \
    -H "Authorization: Bearer ${B_JWT}" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg repo "$B_DID" --arg collection "$NEW_NSID" \
        '{repo:$repo, collection:$collection, record:{msg:"warn accepts"}}')" \
    -o /dev/null -w '%{http_code}')
echo "Warn write status: $WRITE_STATUS"
echo "expected: 200 (Optimistic accept; record landed)"

# ============================================================
# Block 3 — Side-effect-check: warn events on B
# ============================================================
echo
echo "[scenario-6b] Block 3: side-effect-check"
echo "============================================================"

echo "--- last 4 fetch-failed + warn-fallback lines ---"
grep -E 'lexicon_fetch_failed|lexicon_fetch_failed_warn_fallback' "$B_LOG" | tail -4 \
    || echo "(none found)"
echo
echo "expected: both lexicon_fetch_failed AND lexicon_fetch_failed_warn_fallback present"

# ============================================================
# Block 4 — Restart A for downstream scenarios
# ============================================================
echo
echo "[scenario-6b] Block 4: restart A for downstream scenarios"
echo "============================================================"
pb_launch_instance a
pb_wait_for_ready a

echo
echo "[scenario-6b] decision-point:"
echo "  expected: HTTP 200 (Optimistic accept);"
echo "            both lexicon_fetch_failed AND lexicon_fetch_failed_warn_fallback events;"
echo "            A restored and ready before continuing."
echo "  operator: confirm before Scenario 14."
