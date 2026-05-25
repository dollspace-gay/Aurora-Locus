#!/usr/bin/env bash
# Arc 17 Scenario 14 — Tombstoned authority + v4.2 410→400 wire shift
# (CONSOLIDATED).
#
# Exercises Arc 17 authority_tombstoned classification (the production
# fetcher's PLC-tombstone consumption). The Arc 13 v4.2 raw 410→400 wire
# shift on the federation handlers is NOT exercised here — see
# arc17-phase-b-commands.md §Scenario 14 final notes and chainlink #134
# v0.6 disposition.
#
# IMPORTANT: This scenario assumes the mock-PLC tombstone-410 patch is
# IN PLACE permanently as of v0.6 (M1.4 made it permanent in
# phase-b/mock-plc.py). The v0.5 inline-patch-and-revert dance is no
# longer needed; this scenario just verifies the 410 fires.
#
# Source-of-record: docs/internal/arc17-phase-b-commands.md Scenario 14.

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
: "${A_DID:?Scenario 2 must run first}"
: "${B_DID:?Scenario 12 must run first}"
: "${B_JWT:?Scenario 12 must run first}"
: "${B_ENV:?Scenario 2 must run first}"
: "${PDS_DID_PLC_URL:?pb_env_init must have set PDS_DID_PLC_URL}"

B_LOG="/tmp/pds-b-${BACKEND}.log"

# ============================================================
# Block 1 — Create a SEPARATE account to tombstone (don't tombstone
# the main A_DID; tombstoning is terminal)
# ============================================================
echo
echo "[scenario-14] Block 1: create + tombstone a target account on A"
echo "============================================================"

TS_RESP=$(curl -sX POST "http://localhost:${A_PORT}/xrpc/dev.aurora.createAccount" \
    -H "Content-Type: application/json" \
    -d '{"handle":"tombstone-target.localhost","email":"ts@localhost","password":"phase-b-arc17-ts"}')
TS_DID=$(echo "$TS_RESP" | jq -r '.did // empty')
echo "tombstone target DID: $TS_DID"
case "$TS_DID" in
did:plc:*) : ;;
*) echo "[scenario-14] BAD target DID '$TS_DID' — abort" >&2; exit 1 ;;
esac

TOMBSTONE_RESP=$(curl -sX POST "http://localhost:${A_PORT}/xrpc/dev.aurora.tombstoneDid" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg did "$TS_DID" '{did:$did}')")
echo "tombstone response: $TOMBSTONE_RESP"
echo "expected: { did, tombstoneCid, prevCid }"

# Confirm mock-PLC returns 410 for the tombstoned DID (the v0.6
# permanent patch should already be in place).
STATUS=$(curl -s -o /dev/null -w '%{http_code}' "${PDS_DID_PLC_URL}/${TS_DID}")
echo "mock PLC status for tombstoned DID: $STATUS  (expected: 410)"
if [ "$STATUS" != "410" ]; then
    echo "[scenario-14] mock-PLC did not return 410 — confirm the v0.6 permanent" >&2
    echo "patch landed in phase-b/mock-plc.py (M1.4 / chainlink #109)" >&2
fi

# ============================================================
# Block 2 — Point B at TS_DID; expect 502 LexiconAuthorityTombstoned
# ============================================================
echo
echo "[scenario-14] Block 2: B fetches against tombstoned authority"
echo "============================================================"

pb_kill_instance b

# Re-point B at the tombstoned DID + flip to hard_fail.
sed -i "s|^export PDS_LEXICON_DID_AUTHORITY=.*|export PDS_LEXICON_DID_AUTHORITY=${TS_DID}|" "$B_ENV"
if grep -q "^export PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=" "$B_ENV"; then
    sed -i 's/^export PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=.*/export PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=hard_fail/' "$B_ENV"
else
    echo "export PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=hard_fail" >> "$B_ENV"
fi

pb_launch_instance b
pb_wait_for_ready b

WRITE_STATUS=$(curl -sX POST "http://localhost:${B_PORT}/xrpc/com.atproto.repo.createRecord" \
    -H "Authorization: Bearer ${B_JWT}" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg repo "$B_DID" \
        '{repo:$repo, collection:"com.example.lexicon.ts14", record:{msg:"tombstone test"}}')" \
    -o /dev/null -w '%{http_code}')
echo "Arc 17 tombstone write status: $WRITE_STATUS"
echo "expected: 502 LexiconAuthorityTombstoned"

echo "--- last 2 lexicon_fetch_failed lines ---"
grep 'lexicon_fetch_failed' "$B_LOG" | tail -2 \
    || echo "(NOT FOUND)"
echo
echo "expected: failure_class=authority_tombstoned"

# ============================================================
# Block 3 — Restore B's env to A_DID + warn for downstream scenarios
# ============================================================
echo
echo "[scenario-14] Block 3: restore B env for downstream"
echo "============================================================"

pb_kill_instance b
sed -i "s|^export PDS_LEXICON_DID_AUTHORITY=.*|export PDS_LEXICON_DID_AUTHORITY=${A_DID}|" "$B_ENV"
sed -i 's/^export PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=.*/export PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=warn/' "$B_ENV"
pb_launch_instance b
pb_wait_for_ready b

echo
echo "[scenario-14] decision-point:"
echo "  expected: 410 from mock-PLC for the tombstoned DID;"
echo "            B write = 502 LexiconAuthorityTombstoned;"
echo "            failure_class=authority_tombstoned in the log."
echo "  v0.6 note: the v4.2 raw 410->400 wire shift on the federation handlers"
echo "  is NOT exercised by this scenario; it's deferred to Cluster 2 / #134."
echo "  operator: confirm before Scenario 15."
