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
# Rate-limit caveat (documented; not auto-mitigated):
#
# N concurrent createRecord against the SAME DID (B_DID) hits the PDS's
# per-DID-per-endpoint check_did_endpoint() bucket
# (src/rate_limit.rs::check_did_endpoint, called from the createRecord
# handler). Burst tolerance is config-driven; under typical defaults a
# burst of ~10 same-DID requests may pass cleanly, OR may 429 some.
# If some return 429 BEFORE reaching the resolver, the delta=1
# assertion below is still satisfied coincidentally (only the requests
# that bypassed the limiter reach the de-dup gate). That's an easy-pass
# trap — delta=1 looks like single-flight working, but it's actually
# rate-limit masking the test.
#
# If you see N concurrent writes returning a mix of 200/429, the
# deterministic fix is to give each concurrent call its OWN account
# (per-DID buckets are fresh per DID; the de-dup gate is keyed by NSID,
# so distinct DIDs against same NSID still tests single-flight). N=10
# accounts in scenario-15 setup ~ +1s, but the assertion becomes real.
#
# Watch the http= lines surfaced below — any http=429 in the burst
# invalidates the delta=1 read.
#
# Background: PDS_RATE_LIMITS_ENABLED env var is loaded into config but
# never consulted by src/server.rs:98 (it unconditionally wires the
# middleware). Out-of-scope production-code fix would let Phase B set
# the env var to disable rate-limiting cleanly. See findings doc.
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
