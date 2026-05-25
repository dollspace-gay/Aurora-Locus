#!/usr/bin/env bash
# Arc 17 Scenario 6a — HardFail: record rejected with HTTP 502
# LexiconFetchFailed.
#
# Restarts B with PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=hard_fail, kills A
# so B's outbound connect is refused (→ pds_unreachable), confirms a NEW
# (uncached) NSID validate gets rejected as 502 LexiconFetchFailed.
#
# Source-of-record: docs/internal/arc17-phase-b-commands.md Scenario 6a.

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
: "${A_ENV:?Scenario 2 must run first}"

B_LOG="/tmp/pds-b-${BACKEND}.log"

# ============================================================
# Block 1 — Restart B with hard_fail; A stays up through B startup
# ============================================================
echo
echo "[scenario-6a] Block 1: restart B with PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=hard_fail"
echo "============================================================"

# Append the flag to B's env (idempotent if already set).
if ! grep -q "^export PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=" "$B_ENV"; then
    echo "export PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=hard_fail" >> "$B_ENV"
else
    sed -i 's/^export PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=.*/export PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=hard_fail/' "$B_ENV"
fi

pb_kill_instance b
pb_launch_instance b
pb_wait_for_ready b

# ============================================================
# Block 2 — Kill A so B's next fetch hits connect-refused
# ============================================================
echo
echo "[scenario-6a] Block 2: kill A — engineer connect-refused"
echo "============================================================"

pb_kill_instance a
sleep 2
if curl -sf "http://localhost:${A_PORT}/xrpc/com.atproto.server.describeServer" >/dev/null 2>&1; then
    echo "[scenario-6a] A still up — abort and rerun" >&2
    exit 1
fi
echo "A is down — outbound connects from B will be refused"

# ============================================================
# Block 3 — B writes a record of a NEW NSID; expect 502 LexiconFetchFailed
# ============================================================
echo
echo "[scenario-6a] Block 3: B createRecord against new NSID (expect 502)"
echo "============================================================"

NEW_NSID="com.example.lexicon.hardfail6a"
RESP_BODY_PATH=/tmp/scenario-6a-body.json
WRITE_STATUS=$(curl -sX POST "http://localhost:${B_PORT}/xrpc/com.atproto.repo.createRecord" \
    -H "Authorization: Bearer ${B_JWT}" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg repo "$B_DID" --arg collection "$NEW_NSID" \
        '{repo:$repo, collection:$collection, record:{msg:"will fail"}}')" \
    -o "$RESP_BODY_PATH" -w '%{http_code}')
echo "HardFail write status: $WRITE_STATUS"
echo "HardFail write body:"
cat "$RESP_BODY_PATH"
echo
echo "expected status: 502"
echo "expected body shape: { error: 'LexiconFetchFailed', message: '...' }"

# ============================================================
# Block 4 — Side-effect-check: lexicon_fetch_failed event on B
# ============================================================
echo
echo "[scenario-6a] Block 4: side-effect-check (lexicon_fetch_failed event)"
echo "============================================================"

echo "--- last 2 lexicon_fetch_failed lines ---"
grep 'lexicon_fetch_failed' "$B_LOG" | tail -2 \
    || echo "(NOT FOUND — falsifies the HardFail path)"
echo
echo "expected: failure_class=pds_unreachable, nsid=${NEW_NSID}"

# ============================================================
# Block 5 — Restart A so subsequent scenarios run against both up
# ============================================================
echo
echo "[scenario-6a] Block 5: restart A for downstream scenarios"
echo "============================================================"
pb_launch_instance a
pb_wait_for_ready a

echo
echo "[scenario-6a] decision-point:"
echo "  expected: HTTP 502 LexiconFetchFailed; failure_class=pds_unreachable;"
echo "            A restored and ready before continuing."
echo "  operator: confirm before Scenario 6b."
