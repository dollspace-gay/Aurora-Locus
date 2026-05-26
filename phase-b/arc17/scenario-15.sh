#!/usr/bin/env bash
# Arc 17 Scenario 15 — Single-flight de-dup.
#
# N concurrent validate calls for the same uncached NSID → exactly ONE
# fetch (round-1 F6 closure; fetch_attempts_total increments by 1, not N).
#
# Source-of-record: docs/internal/arc17-phase-b-commands.md Scenario 15.

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

# shellcheck source=../lib/env.sh
source phase-b/lib/env.sh
pb_env_init
pb_env_echo_confirm

: "${B_PORT:?B_PORT must be set}"
: "${B_DID:?Scenario 12 must run first}"
: "${B_JWT:?Scenario 12 must run first}"
: "${B_ADMIN_JWT:?Scenario 12 must run first (mints B_ADMIN_JWT)}"

TARGET_NSID="com.example.lexicon.target"
B_LOG="/tmp/pds-b-${BACKEND}.log"

# ============================================================
# Block 1 — Evict everything; confirm empty
# ============================================================
echo
echo "[scenario-15] Block 1: evict-all + confirm empty"
echo "============================================================"

curl -sf -X POST "http://localhost:${B_PORT}/xrpc/tools.aurora.lexicon.evictCache" \
    -H "Authorization: Bearer ${B_ADMIN_JWT}" \
    -H "Content-Type: application/json" \
    -d '{"all":true}' | jq .

CACHE_AFTER_EVICT=$(curl -sf "http://localhost:${B_PORT}/xrpc/tools.aurora.lexicon.getCacheState" \
    -H "Authorization: Bearer ${B_ADMIN_JWT}" | jq '.entries | length')
echo "cache size after evict-all: $CACHE_AFTER_EVICT  (expected: 0)"
if [ "$CACHE_AFTER_EVICT" != "0" ]; then
    echo "[scenario-15] cache NOT empty; aborting burst — re-run after eviction settles" >&2
    exit 1
fi

# ============================================================
# Block 2 — Capture baseline counter; fire N concurrent writes
# ============================================================
#
# Rate-limiting is disabled in Phase B via PDS_RATE_LIMITS_ENABLED=false
# emitted by lib/env.sh (chainlink #153 wired the dead config knob in
# 58f5a13). The single-flight de-dup assertion (delta=1, not N) below
# is what this scenario tests; with rate-limits off, the N=10 concurrent
# burst all reaches the resolver and the de-dup gate (keyed by NSID at
# the resolver level) coalesces them to a single fetch. Earlier versions
# of this scenario carried a rate-limit-masking caveat (if N-1 of N
# 429'd before reaching the resolver, the surviving 1 would still
# produce delta=1 — coincidence, not de-dup); that caveat is moot under
# the harness-disabled rate limits. The http= surfacing below stays as
# a belt-and-suspenders diagnostic: if some future change re-enables
# rate-limiting (or runs scenario-15 against a non-Phase-B PDS), any
# http=429 in the burst will flag the masked-test risk.
#
# Recon resolution: createRecord doesn't actually call
# check_did_endpoint() (that's an email/account-delete handler check
# per src/api/server.rs:456, :621, :735). The pre-fix risk was really
# the middleware-level governor at rate_limit.rs:579-587 (per-account
# global authenticated limit, 100 req/sec + burst 50) — N=10 fits
# inside the burst. So even pre-fix, scenario-15's burst would
# probably not have 429'd from the per-DID-per-endpoint bucket. The
# blanket disable closes both possible 429 sources.
echo
echo "[scenario-15] Block 2: baseline counter + N concurrent writes"
echo "============================================================"

PRE_FETCHES=$(curl -sf "http://localhost:${B_PORT}/metrics" 2>/dev/null \
    | grep -E '^aurora_lexicon_fetch_attempts_total' \
    | awk '{print $2}')
PRE_FETCHES=${PRE_FETCHES:-0}
echo "pre-test fetch_attempts_total: $PRE_FETCHES"

N=10
for i in $(seq 1 $N); do
    curl -sX POST "http://localhost:${B_PORT}/xrpc/com.atproto.repo.createRecord" \
        -H "Authorization: Bearer ${B_JWT}" \
        -H "Content-Type: application/json" \
        -d "$(jq -nc --arg repo "$B_DID" --arg msg "concurrent $i" --arg collection "$TARGET_NSID" \
            '{repo:$repo, collection:$collection, record:{msg:$msg}}')" \
        -w 'http=%{http_code}\n' \
        >"/tmp/scenario-15-conc-${i}.out" 2>&1 &
done
wait
echo "all $N concurrent writes returned"

# Surface any 429s in the burst so the operator can see if the
# delta=1 assertion below is real or rate-limit-masked.
RL_429S=$(grep -l 'http=429' /tmp/scenario-15-conc-*.out 2>/dev/null | wc -l)
if [ "$RL_429S" -gt 0 ]; then
    echo "[scenario-15] WARNING: ${RL_429S} of $N concurrent writes returned 429"
    echo "[scenario-15] the delta=1 assertion below may be MASKED by rate-limiting"
    echo "[scenario-15] rather than actually testing single-flight de-dup."
    echo "[scenario-15] see the rate-limit caveat at the top of Block 2."
fi

# ============================================================
# Block 3 — Side-effect-check: counter delta + event count
# ============================================================
echo
echo "[scenario-15] Block 3: side-effect-check"
echo "============================================================"

POST_FETCHES=$(curl -sf "http://localhost:${B_PORT}/metrics" 2>/dev/null \
    | grep -E '^aurora_lexicon_fetch_attempts_total' \
    | awk '{print $2}')
POST_FETCHES=${POST_FETCHES:-0}
DELTA=$((POST_FETCHES - PRE_FETCHES))
echo "post-test fetch_attempts_total: $POST_FETCHES (was $PRE_FETCHES)"
echo "delta: $DELTA  (expected: 1)"

echo "--- last 5 lexicon_fetch_complete lines on B ---"
grep 'lexicon_fetch_complete' "$B_LOG" | tail -5 || echo "(none)"

echo
echo "[scenario-15] decision-point:"
echo "  expected: delta = 1 (NOT N=$N); exactly one lexicon_fetch_complete for the"
echo "            burst window. Delta > 1 = single-flight gate didn't fire."
echo "  operator: confirm before continuing."
