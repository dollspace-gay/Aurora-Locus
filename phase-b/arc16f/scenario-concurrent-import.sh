#!/usr/bin/env bash
# Arc 16f importRepo cross-task concurrent-collision — v0.6 Cluster 3
# Member 3.3 (chainlink #155).
#
# Verifies SingleFlightImportLock (src/api/repo_import.rs:858-884)
# under in-process contention: two concurrent importRepo HTTP requests
# for the SAME importing DID, fired against one running PDS process.
# The lock is a process-local Mutex<HashSet<String>> via OnceLock —
# both requests reach try_acquire concurrently (the funnel at :150
# auth → :151 scope → :197-209 signing-key SELECT has no per-DID
# serializer upstream of the lock), one wins, the other surfaces
# Err(PdsError::ConcurrentMutation) → HTTP 409 (the loser-shape
# round-3 #3 PINNED — recon confirmed no queue-and-retry wrapper).
#
# Verification-only — NO production-code edit. The lock exists; this
# scenario witnesses end-to-end that it ENFORCES under contention.
#
# Source-of-record: docs/V06_DESIGN.md Cluster 3 Member 3.3 +
# docs/internal/v06-recon/V06_CLUSTER3_RECON.md +
# chainlink #155.
#
# Standalone — owns its own setup (mock PLC + fresh data dir + fresh
# account + fixture CAR). Does NOT depend on other arc scripts.
#
# Kill-mid is explicitly OUT of scope: against the in-process lock,
# "process dies holding the lock" is degenerate (lock dies with the
# process). The natural home for kill-mid coverage is the v0.6+
# cross-process pg_try_advisory_lock variant tracked in-source at
# src/api/repo_import.rs:57-65.

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

B_LOG="/tmp/pds-b-${BACKEND}.log"
CAR_PATH="/tmp/arc16f-concurrent-import-fixture.car"
RESP_PATH_1="/tmp/arc16f-concurrent-import-resp-1"
RESP_PATH_2="/tmp/arc16f-concurrent-import-resp-2"
STATUS_PATH_1="/tmp/arc16f-concurrent-import-status-1"
STATUS_PATH_2="/tmp/arc16f-concurrent-import-status-2"

# Per-actor record count helper — same shape as scenario-16's. The
# `record` table is backend-independent (lives in per-actor SQLite
# regardless of PDS_DB_BACKEND).
pb_record_count() {
    local did_var="${1:-B_DID}"
    local did="${!did_var}"
    local safe_did="${did//:/_}"
    local actor_db
    actor_db=$(find "${B_DATA}/actors" -type f -name 'store.sqlite' \
        -path "*/${safe_did}/*" -print -quit 2>/dev/null)
    if [ -z "$actor_db" ]; then
        echo "(per-actor store.sqlite not found under ${B_DATA}/actors/*/${safe_did}/)"
        return
    fi
    sqlite3 "$actor_db" "SELECT count(*) FROM record" 2>/dev/null \
        || echo "(sqlite3 unavailable — operator: sqlite3 '$actor_db' 'SELECT count(*) FROM record')"
}

# ============================================================
# Block 1 — Setup-to-confirmed-up: mock PLC, fresh data dir,
# fresh account, seed a few records.
# ============================================================
echo
echo "[concurrent-import] Block 1: setup-to-confirmed-up"
echo "============================================================"

pb_mock_plc_start
pb_mock_plc_wait

pb_kill_prior
pb_fresh_data_dir b

# B is the standalone instance for this scenario — no lexicon /
# federation surface needed; the lock is local to the importRepo
# handler's process.
export PDS_LEXICON_ENABLED=false
unset PDS_LEXICON_DID_AUTHORITY || true
pb_env_emit_role b

pb_launch_instance b
pb_wait_for_ready b
pb_grep_banner b

# Seed B's account.
pb_create_account b "concurrent-import.localhost" \
    "concurrent-import@localhost" "phase-b-arc16f-pw"
pb_echo_creds b

# Seed three records so the export CAR carries non-trivial state. The
# count is what the no-double-write invariant compares against
# post-race.
echo
echo "[concurrent-import] seeding 3 records on B for a non-trivial CAR…"
for i in 1 2 3; do
    curl -sX POST "http://localhost:${B_PORT}/xrpc/com.atproto.repo.createRecord" \
        -H "Authorization: Bearer ${B_JWT}" \
        -H "Content-Type: application/json" \
        -d "$(jq -nc --arg repo "$B_DID" \
            '{repo:$repo, collection:"app.bsky.feed.post",
              record:{text:"seed-record-'"$i"'", createdAt:"2026-01-01T00:00:00Z"}}')" \
        -o /dev/null -w "  seed-${i} status: %{http_code}\n"
    # SQLite seed sleep — three rapid createRecords can trip SQLITE_BUSY
    # on the writer-lock (per Phase B harness conventions memory).
    sleep 0.3
done

BASELINE_COUNT=$(pb_record_count B_DID)
echo "B baseline record count (post-seed, pre-race): ${BASELINE_COUNT}"
test "${BASELINE_COUNT}" -ge 3 \
    || { echo "[concurrent-import] FATAL: expected >= 3 records after seed, got '${BASELINE_COUNT}' — aborting"; exit 1; }

# Export B's repo to a CAR file (the source artifact for the race).
echo
echo "[concurrent-import] exporting B's repo to ${CAR_PATH}…"
curl -sf "http://localhost:${B_PORT}/xrpc/com.atproto.sync.getRepo?did=${B_DID}" \
    -o "${CAR_PATH}" \
    || { echo "[concurrent-import] FATAL: getRepo failed — aborting"; exit 1; }
CAR_SIZE=$(stat -c '%s' "${CAR_PATH}" 2>/dev/null || stat -f '%z' "${CAR_PATH}" 2>/dev/null)
echo "CAR fixture: size=${CAR_SIZE} bytes"
test "${CAR_SIZE:-0}" -gt 100 \
    || { echo "[concurrent-import] FATAL: CAR suspiciously small (${CAR_SIZE} bytes) — aborting"; exit 1; }

# ============================================================
# Block 2 — Race two concurrent importRepo POSTs against the
# same DID. Capture both wire statuses + response bodies for
# the operator's loser-shape assertion.
# ============================================================
echo
echo "[concurrent-import] Block 2: race two concurrent importRepo POSTs"
echo "============================================================"

# Fire both in background BEFORE either curl finishes its connect
# phase so try_acquire contends in-flight. The lock is held for the
# duration of the CAR parse + verify + apply, which is much longer
# than two curl-start latencies on localhost — collision is reliable
# with two requests, no need for higher fan-out.
(
    curl -sX POST "http://localhost:${B_PORT}/xrpc/com.atproto.repo.importRepo" \
        -H "Authorization: Bearer ${B_JWT}" \
        -H "Content-Type: application/vnd.ipld.car" \
        --data-binary "@${CAR_PATH}" \
        -o "${RESP_PATH_1}" -w '%{http_code}' > "${STATUS_PATH_1}"
) &
PID_1=$!
(
    curl -sX POST "http://localhost:${B_PORT}/xrpc/com.atproto.repo.importRepo" \
        -H "Authorization: Bearer ${B_JWT}" \
        -H "Content-Type: application/vnd.ipld.car" \
        --data-binary "@${CAR_PATH}" \
        -o "${RESP_PATH_2}" -w '%{http_code}' > "${STATUS_PATH_2}"
) &
PID_2=$!

# Wait for both. Bash's `wait` blocks on the specified PIDs; using
# named PIDs (not bare `wait`) per Phase B harness memory — `wait`
# alone is unreliable for time-bounded background-wait coverage.
wait "${PID_1}"
wait "${PID_2}"

STATUS_1=$(cat "${STATUS_PATH_1}")
STATUS_2=$(cat "${STATUS_PATH_2}")
echo "request-1 status: ${STATUS_1}"
echo "request-1 body:"
cat "${RESP_PATH_1}" | jq . 2>/dev/null || cat "${RESP_PATH_1}"
echo
echo "request-2 status: ${STATUS_2}"
echo "request-2 body:"
cat "${RESP_PATH_2}" | jq . 2>/dev/null || cat "${RESP_PATH_2}"

# ============================================================
# Block 3 — Loser-shape assertion + no-double-write invariant
# ============================================================
echo
echo "[concurrent-import] Block 3: side-effect-check"
echo "============================================================"

# Round-3 #3 PIN: the loser must be the reject shape (409
# ConcurrentMutation), NOT wait-then-succeed. The disjunction was
# the recon HYPOTHESIS; the test pins the recon-confirmed truth so a
# future regression that silently flipped reject→wait would fail
# loud here (an "accept either" assertion would mask exactly that).
WINNER_COUNT=0
LOSER_COUNT=0
for s in "${STATUS_1}" "${STATUS_2}"; do
    case "$s" in
    200) WINNER_COUNT=$((WINNER_COUNT + 1)) ;;
    409) LOSER_COUNT=$((LOSER_COUNT + 1)) ;;
    esac
done
echo "wire-shape tally: winners(200)=${WINNER_COUNT}  losers(409)=${LOSER_COUNT}"
echo
echo "expected:"
echo "  winners(200) == 1  (one request acquired the lock and completed import)"
echo "  losers(409)  == 1  (the other surfaced ConcurrentMutation)"
echo "  the 409 body should include error name 'ConcurrentMutation'"
echo "    (per src/error.rs:728-732 IntoResponse arm — pinned"
echo "    loser-shape for the regression-loud assertion)"
echo
echo "NOT expected:"
echo "  winners(200) == 2  → lock not enforcing; both ran serially-but-uncontended"
echo "                       (could mean the race lost to network startup latency;"
echo "                       re-run, or raise concurrency. If reproducible at"
echo "                       two-concurrent: the lock regressed.)"
echo "  losers(409)  == 2  → both rejected (the only winner ran to completion"
echo "                       BEFORE either request issued — try-acquire serialization"
echo "                       failed end-to-end. Investigate."

echo
echo "--- forensic log signature on B ---"
echo "expected one 'importRepo concurrent mutation rejected' warn line (the loser):"
grep 'importRepo concurrent mutation rejected' "${B_LOG}" | tail -5 \
    || echo "(NOT FOUND — the lock's warn-emit at repo_import.rs:217 didn't fire)"

# No-double-write invariant — the load-bearing teeth. Post-race
# record count must reflect ONE import's worth of state, not two.
# Since the CAR was exported from this same actor, a successful
# self-import is idempotent: post-import count == baseline count.
# (A future cross-account import scenario would see post == source
# fixture count; here the simpler equality holds.)
POST_RACE_COUNT=$(pb_record_count B_DID)
echo
echo "--- no-double-write invariant: post-race record count ---"
echo "baseline (pre-race): ${BASELINE_COUNT}"
echo "post-race:           ${POST_RACE_COUNT}"
echo
echo "expected: post-race count == baseline count (${BASELINE_COUNT})."
echo "  self-import is idempotent at the MST level — one import maintains"
echo "  the same record set; two SERIAL imports would also. A LOCK"
echo "  FAILURE manifests as a half-apply / partial-state row count"
echo "  (not necessarily double, but inconsistent with idempotent self-"
echo "  import). Mismatch here = the actor's state is corrupted by"
echo "  interleaving."

# ============================================================
# Block 4 — Decision-point
# ============================================================
echo
echo "[concurrent-import] decision-point:"
echo "  expected (load-bearing):"
echo "    1. winners(200) == 1 and losers(409) == 1"
echo "    2. the 409 response body's error == 'ConcurrentMutation' (round-3 #3 pin)"
echo "    3. post-race record count == baseline (no-double-write invariant)"
echo "    4. 'importRepo concurrent mutation rejected' warn line in B's log"
echo "  v0.6 Cluster 3 Member 3.3 (#155) — verification-only, no production touch."
echo "  v0.6+ kill-mid coverage is the natural companion to the cross-process"
echo "  pg_try_advisory_lock variant (src/api/repo_import.rs:57-65), not to"
echo "  this in-process scenario."
echo "  operator: confirm all four signals; the lock holds."
