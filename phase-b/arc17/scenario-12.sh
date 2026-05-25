#!/usr/bin/env bash
# Arc 17 Scenario 12 — Two-instance localhost federation (canonical
# end-to-end). The load-bearing scenario for sub-feature #47.
#
# B validates a record of an unknown NSID whose lexicon A hosts. B's
# resolver dispatches through the production fetcher → cache → validate.
#
# Source-of-record: docs/internal/arc17-phase-b-commands.md Scenario 12.

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

# shellcheck source=../lib/env.sh
source phase-b/lib/env.sh
# shellcheck source=../lib/creds.sh
source phase-b/lib/creds.sh
pb_env_init
pb_env_echo_confirm

: "${A_DID:?Scenario 2 must run first}"
: "${A_PORT:?A_PORT must be set}"
: "${B_PORT:?B_PORT must be set}"

TARGET_NSID="com.example.lexicon.target"
B_LOG="/tmp/pds-b-${BACKEND}.log"

# ============================================================
# Block 1 — Setup: B-side account (Scenario 12 needs it; not seeded
# in Scenario 2 which only seeds A).
# ============================================================
echo
echo "[scenario-12] Block 1: seed B-side account"
echo "============================================================"
pb_create_account b "bob.localhost" "bob@localhost" "phase-b-arc17-pw-b"
pb_echo_creds b

# ============================================================
# Block 2 — Capture baseline counter, fire the validate-routed write
# ============================================================
echo
echo "[scenario-12] Block 2: baseline + B createRecord (validate routes via lexicon-fetch)"
echo "============================================================"

PRE_FETCHES=$(curl -sf "http://localhost:${B_PORT}/metrics" 2>/dev/null \
    | grep -E '^aurora_lexicon_fetch_attempts_total' \
    | awk '{print $2}')
PRE_FETCHES=${PRE_FETCHES:-0}
echo "pre-test fetch_attempts_total: $PRE_FETCHES"

WRITE_RESP=$(curl -sX POST "http://localhost:${B_PORT}/xrpc/com.atproto.repo.createRecord" \
    -H "Authorization: Bearer ${B_JWT}" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg repo "$B_DID" --arg collection "$TARGET_NSID" \
        '{repo:$repo, collection:$collection, record:{msg:"hello federation"}}')")
echo "B createRecord response: $WRITE_RESP"
echo "expected: { uri: 'at://${B_DID}/${TARGET_NSID}/<rkey>', cid: 'bafy...' }"

# ============================================================
# Block 3 — Side-effect-check: lexicon_fetch_complete + counter +1
# ============================================================
echo
echo "[scenario-12] Block 3: side-effect-check"
echo "============================================================"

echo "--- lexicon_fetch_complete event (last 3 lines on B) ---"
grep 'lexicon_fetch_complete' "$B_LOG" | tail -3 \
    || echo "(NOT FOUND — falsifies that the fetch path actually ran)"

POST_FETCHES=$(curl -sf "http://localhost:${B_PORT}/metrics" 2>/dev/null \
    | grep -E '^aurora_lexicon_fetch_attempts_total' \
    | awk '{print $2}')
POST_FETCHES=${POST_FETCHES:-0}
DELTA=$((POST_FETCHES - PRE_FETCHES))
echo "post-test fetch_attempts_total: $POST_FETCHES (was $PRE_FETCHES, delta=$DELTA)"
echo "expected delta: 1"

# ============================================================
# Block 4 — Cache populated; second write hits cache (no new fetch)
# ============================================================
echo
echo "[scenario-12] Block 4: cache populated + second write hits cache"
echo "============================================================"

# Mint a B-side admin token to inspect the cache via lexicon admin endpoints.
curl -sX POST "http://localhost:${B_PORT}/xrpc/dev.aurora.grantAdmin" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg did "$B_DID" '{did:$did, role:"admin"}')" >/dev/null
ADMIN_RESP=$(curl -sX POST "http://localhost:${B_PORT}/xrpc/dev.aurora.mintToken" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg did "$B_DID" '{did:$did}')")
export B_ADMIN_JWT=$(echo "$ADMIN_RESP" | jq -r '.accessJwt // empty')
echo "B_ADMIN_JWT length = ${#B_ADMIN_JWT}"

echo "--- getCacheState for ${TARGET_NSID} ---"
curl -sf "http://localhost:${B_PORT}/xrpc/tools.aurora.lexicon.getCacheState?nsid=${TARGET_NSID}" \
    -H "Authorization: Bearer ${B_ADMIN_JWT}" | jq .
echo "expected: entries[0] with authorityDid=${A_DID}, isStale=false"

# Second write — same NSID, should hit cache (no new fetch).
PRE_FETCHES_2=$POST_FETCHES
curl -sX POST "http://localhost:${B_PORT}/xrpc/com.atproto.repo.createRecord" \
    -H "Authorization: Bearer ${B_JWT}" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg repo "$B_DID" --arg collection "$TARGET_NSID" \
        '{repo:$repo, collection:$collection, record:{msg:"second"}}')" >/dev/null
POST_FETCHES_2=$(curl -sf "http://localhost:${B_PORT}/metrics" 2>/dev/null \
    | grep -E '^aurora_lexicon_fetch_attempts_total' \
    | awk '{print $2}')
POST_FETCHES_2=${POST_FETCHES_2:-0}
echo "second-write fetch_attempts_total: $POST_FETCHES_2 (expected: $PRE_FETCHES_2 — no new fetch)"

echo
echo "[scenario-12] decision-point:"
echo "  expected: B createRecord = 200 with uri+cid;"
echo "            lexicon_fetch_complete event present on B;"
echo "            fetch_attempts_total delta = 1;"
echo "            getCacheState shows one entry;"
echo "            second write does NOT increment fetch_attempts_total."
echo "  operator: confirm all four before continuing."
