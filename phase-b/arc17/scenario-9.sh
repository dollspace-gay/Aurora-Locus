#!/usr/bin/env bash
# Arc 17 Scenario 9 — Admin endpoints against the live fetcher.
#
# Exercises getCacheState / fetchNow / evictCache against the cache
# populated by Scenario 12. Confirms the auth-FIRST gate order: plain
# JWT → 403; admin JWT → 503 LexiconDisabled on A (where lexicon is
# disabled).
#
# Source-of-record: docs/internal/arc17-phase-b-commands.md Scenario 9.

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

# shellcheck source=../lib/env.sh
source phase-b/lib/env.sh
pb_env_init
pb_env_echo_confirm

: "${A_DID:?Scenario 2 must run first}"
: "${A_JWT:?Scenario 2 must run first}"
: "${A_PORT:?A_PORT must be set}"
: "${B_PORT:?B_PORT must be set}"
: "${B_ADMIN_JWT:?Scenario 12 must run first (mints B_ADMIN_JWT)}"

TARGET_NSID="com.example.lexicon.target"

# ============================================================
# Block 1 — Scenario-call: getCacheState / fetchNow / evictCache
# ============================================================
echo
echo "[scenario-9] Block 1: admin endpoints against cached state"
echo "============================================================"

echo "--- getCacheState (full list) ---"
curl -sf "http://localhost:${B_PORT}/xrpc/tools.aurora.lexicon.getCacheState" \
    -H "Authorization: Bearer ${B_ADMIN_JWT}" | jq .
echo "expected: entries: [ { nsid: '${TARGET_NSID}', ... } ]"

PRE_FETCHES=$(curl -sf "http://localhost:${B_PORT}/metrics" 2>/dev/null \
    | grep -E '^aurora_lexicon_fetch_attempts_total' \
    | awk '{print $2}')
PRE_FETCHES=${PRE_FETCHES:-0}
echo "pre-fetchNow fetch_attempts_total: $PRE_FETCHES"

echo "--- fetchNow on already-cached entry (cache short-circuit; no new fetch) ---"
curl -sf -X POST "http://localhost:${B_PORT}/xrpc/tools.aurora.lexicon.fetchNow" \
    -H "Authorization: Bearer ${B_ADMIN_JWT}" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg nsid "$TARGET_NSID" '{nsid:$nsid}')" | jq .

echo "--- evictCache (drop the entry) ---"
curl -sf -X POST "http://localhost:${B_PORT}/xrpc/tools.aurora.lexicon.evictCache" \
    -H "Authorization: Bearer ${B_ADMIN_JWT}" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg nsid "$TARGET_NSID" '{nsid:$nsid}')" | jq .
echo "expected: { evicted: 1 }"

echo "--- getCacheState after evict (expected: empty entries) ---"
curl -sf "http://localhost:${B_PORT}/xrpc/tools.aurora.lexicon.getCacheState" \
    -H "Authorization: Bearer ${B_ADMIN_JWT}" | jq .

POST_EVICT=$(curl -sf "http://localhost:${B_PORT}/metrics" 2>/dev/null \
    | grep -E '^aurora_lexicon_fetch_attempts_total' \
    | awk '{print $2}')
POST_EVICT=${POST_EVICT:-0}

echo "--- fetchNow AFTER evict (expected: +1 fetch_attempts) ---"
curl -sf -X POST "http://localhost:${B_PORT}/xrpc/tools.aurora.lexicon.fetchNow" \
    -H "Authorization: Bearer ${B_ADMIN_JWT}" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg nsid "$TARGET_NSID" '{nsid:$nsid}')" | jq .
POST_FETCH=$(curl -sf "http://localhost:${B_PORT}/metrics" 2>/dev/null \
    | grep -E '^aurora_lexicon_fetch_attempts_total' \
    | awk '{print $2}')
POST_FETCH=${POST_FETCH:-0}
echo "fetchNow-after-evict delta: $((POST_FETCH - POST_EVICT))  (expected: 1)"

# ============================================================
# Block 2 — Auth gate order (auth-FIRST per #138 doc correction)
# ============================================================
echo
echo "[scenario-9] Block 2: auth-FIRST gate order"
echo "============================================================"

echo "--- no-auth on B (expected: 401) ---"
NOAUTH_STATUS=$(curl -s -o /dev/null -w '%{http_code}' \
    "http://localhost:${B_PORT}/xrpc/tools.aurora.lexicon.getCacheState")
echo "status = $NOAUTH_STATUS"

echo "--- A getCacheState with plain user JWT (expected: 403 — auth gate fires before disabled-config check) ---"
A_PLAIN_STATUS=$(curl -s -o /dev/null -w '%{http_code}' \
    -H "Authorization: Bearer ${A_JWT}" \
    "http://localhost:${A_PORT}/xrpc/tools.aurora.lexicon.getCacheState")
echo "status = $A_PLAIN_STATUS"

# Mint an admin token on A to observe the 503 underneath the auth gate.
curl -sX POST "http://localhost:${A_PORT}/xrpc/dev.aurora.grantAdmin" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg did "$A_DID" '{did:$did, role:"admin"}')" >/dev/null
A_ADMIN_RESP=$(curl -sX POST "http://localhost:${A_PORT}/xrpc/dev.aurora.mintToken" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg did "$A_DID" '{did:$did}')")
export A_ADMIN_JWT=$(echo "$A_ADMIN_RESP" | jq -r '.accessJwt // empty')

echo "--- A getCacheState with admin JWT (expected: 503 LexiconDisabled — A has enabled=false) ---"
A_ADMIN_STATUS=$(curl -s -o /dev/null -w '%{http_code}' \
    -H "Authorization: Bearer ${A_ADMIN_JWT}" \
    "http://localhost:${A_PORT}/xrpc/tools.aurora.lexicon.getCacheState")
echo "status = $A_ADMIN_STATUS"

echo
echo "[scenario-9] decision-point:"
echo "  expected: getCacheState lists the entry;"
echo "            evictCache returns {evicted:1};"
echo "            fetchNow-after-evict delta = 1;"
echo "            no-auth = 401, A+plain JWT = 403, A+admin JWT = 503 LexiconDisabled."
echo "  operator: confirm all five before continuing."
