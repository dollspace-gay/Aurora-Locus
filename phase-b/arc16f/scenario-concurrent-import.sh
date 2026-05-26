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
# docs/internal/v06-recon/V06_CLUSTER3_RECON.md + chainlink #155.
#
# Standalone — owns its own setup (mock PLC + fresh data dir + fresh
# account + fixture CAR). Does NOT depend on other arc scripts.
#
# ── v0.5 importRepo precondition: target actor must be EMPTY ──
#
# Aurora v0.5 importRepo applies the CAR's records as additive
# `WriteOp::Create`s against an empty MST per src/api/repo_import.rs:277-280
# ("No prior-repo snapshot — Aurora's v0.5 importRepo applies imported
# commits as additive diffs against an empty MST. v0.6+ may pass the
# current repo's VerifiedRepo here to support incremental import.").
# Importing INTO a populated repo (e.g. self-importing a CAR exported
# from the same DID's current state, without first clearing the
# actor's records) lands every record as a Create-on-existing-key,
# which proto-blue's MST.add rejects with the literal message
# "Key already exists: <collection>/<rkey>" — surfacing on the wire
# as HTTP 500 InternalServerError. That known-failure mode is itself
# documented at src/api/repo_import.rs:357-367 (the v5.3 / chainlink
# #124 retry-into-collision closure).
#
# So this scenario seeds records on B, exports the CAR, then kills B,
# wipes B's PER-ACTOR store (preserving account.sqlite + plc_keys,
# the v5.2 / chainlink #123 "federation-into-fresh-instance case"
# described verbatim at src/api/repo_import.rs:236-243), restarts B,
# and ONLY THEN issues the race. The winner runs against the empty
# actor that the import path's idempotent ctx.actor_store.create()
# re-materialises at :244 → applies the 3 records cleanly → 200. The
# loser hits the lock → 409 ConcurrentMutation. The pre-race seed +
# wipe is the documented v0.5 precondition (account-seeded, actor-
# empty), not state-suppression of a defect.
#
# Authorship note (recon 2026-05-26): an earlier draft of this script
# raced two POSTs against the populated actor and counted equal
# pre/post record counts as the no-double-write proof. That was wrong
# in two ways: the winner crashed at "Key already exists" (was a 500,
# not the asserted 200); and the equal count consistent with both
# "winner succeeded idempotently" and "winner crashed before any
# write" — the count could not distinguish success from failure. The
# corrected shape below makes the winner do REAL work (import into
# empty), so the count delta IS the no-double-write proof
# (post-race == 3 reflects one complete import; not 3 + double-write,
# not 0 from a winner-crashed).
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
EXPECTED_IMPORT_RECORD_COUNT=3

# Per-actor record count helper — same shape as arc17/scenario-16.sh.
# The `record` table is backend-independent (lives in per-actor
# SQLite regardless of PDS_DB_BACKEND).
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

# Path to B's per-actor directory for $B_DID. Used by the wipe step
# to clear actor records while leaving account.sqlite + plc_keys
# untouched. Shard is hash-derived (DefaultHasher % 256, not
# bash-reproducible across Rust versions), so we resolve it by walk.
pb_actor_dir_for_b_did() {
    local safe_did="${B_DID//:/_}"
    find "${B_DATA}/actors" -type d -name "${safe_did}" -print -quit 2>/dev/null
}

# ============================================================
# Block 1 — Setup-to-confirmed-up + seed records + export CAR
# ============================================================
echo
echo "[concurrent-import] Block 1: setup + seed source records + export CAR"
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

# Seed 3 records (no blob refs — feed posts with text only, so the
# import path doesn't need the blob-fetch primitive). SQLite seed
# sleep between createRecords avoids SQLITE_BUSY on the writer-lock
# (Phase B harness convention).
echo
echo "[concurrent-import] seeding ${EXPECTED_IMPORT_RECORD_COUNT} records on B…"
for i in $(seq 1 "${EXPECTED_IMPORT_RECORD_COUNT}"); do
    curl -sX POST "http://localhost:${B_PORT}/xrpc/com.atproto.repo.createRecord" \
        -H "Authorization: Bearer ${B_JWT}" \
        -H "Content-Type: application/json" \
        -d "$(jq -nc --arg repo "$B_DID" \
            '{repo:$repo, collection:"app.bsky.feed.post",
              record:{"$type":"app.bsky.feed.post",
                      text:"seed-record-'"$i"'",
                      createdAt:"2026-01-01T00:00:00Z"}}')" \
        -o /dev/null -w "  seed-${i} status: %{http_code}\n"
    sleep 0.3
done

SEED_COUNT=$(pb_record_count B_DID)
echo "B record count (post-seed): ${SEED_COUNT}"
test "${SEED_COUNT}" = "${EXPECTED_IMPORT_RECORD_COUNT}" \
    || { echo "[concurrent-import] FATAL: expected ${EXPECTED_IMPORT_RECORD_COUNT} records after seed, got '${SEED_COUNT}' — aborting"; exit 1; }

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
# Block 2 — Empty B's actor store (keep account.sqlite + plc_keys);
# restart B. The v5.2 (chainlink #123) federation-into-fresh-
# instance precondition documented at src/api/repo_import.rs:236-243.
# ============================================================
echo
echo "[concurrent-import] Block 2: empty B's actor store + restart"
echo "============================================================"

# Kill B first so SQLite file locks release. Find the per-actor
# directory before kill so the path resolution doesn't race.
ACTOR_DIR=$(pb_actor_dir_for_b_did)
if [ -z "${ACTOR_DIR}" ]; then
    echo "[concurrent-import] FATAL: could not locate B's per-actor dir under ${B_DATA}/actors/ — aborting"
    exit 1
fi
echo "B per-actor dir: ${ACTOR_DIR}"

pb_kill_instance b

# Remove the per-actor records (store.sqlite + WAL + SHM + any blob
# files; the latter are a no-op for our no-blob-refs fixture). The
# account.sqlite + plc_keys row are untouched — they live at
# ${B_DATA}/account.sqlite, distinct from the per-actor path.
rm -rf "${ACTOR_DIR}"
echo "wiped per-actor dir; account.sqlite + plc_keys preserved at ${B_DATA}/account.sqlite"

# Confirm plc_keys row for B_DID survives the wipe — the v5.2 entry
# gate depends on it (without the plc_keys row, importRepo would
# fail ActorNotInitialized at :207 before ever reaching the lock).
ACCT_DB="${B_DATA}/account.sqlite"
PLC_ROWS=$(sqlite3 "${ACCT_DB}" "SELECT count(*) FROM plc_keys WHERE did='${B_DID}'" 2>/dev/null || echo "?")
echo "plc_keys rows for ${B_DID} post-wipe: ${PLC_ROWS}"
test "${PLC_ROWS}" = "1" \
    || { echo "[concurrent-import] FATAL: plc_keys row missing after wipe (got ${PLC_ROWS}) — would fail ActorNotInitialized before reaching the lock; aborting"; exit 1; }

pb_launch_instance b
pb_wait_for_ready b
pb_grep_banner b

# Pre-race count — should be 0 (or no actor dir yet; the import path
# re-materialises it via ctx.actor_store.create() at :244 on first
# import call). The pre-race count is the baseline for the no-
# double-write invariant: post-race == EXPECTED_IMPORT_RECORD_COUNT
# proves one complete import landed.
PRE_RACE_COUNT=$(pb_record_count B_DID)
echo "B record count (pre-race, post-wipe): ${PRE_RACE_COUNT:-0}"
echo "  expected 0 or missing-dir — the actor store will be re-"
echo "  materialised idempotently by the first importRepo's :244 call."

# ============================================================
# Block 3 — Race two concurrent importRepo POSTs against the
# same DID. Capture both wire statuses + response bodies.
# ============================================================
echo
echo "[concurrent-import] Block 3: race two concurrent importRepo POSTs"
echo "============================================================"

# Fire both in background. The lock is held for the duration of the
# CAR parse + verify + apply, which is much longer than two curl-
# start latencies on localhost — collision is reliable with two
# requests, no need for higher fan-out.
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

# Named-PID waits per Phase B harness memory (`wait` builtin can't
# be `timeout`-wrapped; named PIDs + bounded curl timeouts are the
# right primitive).
wait "${PID_1}"
wait "${PID_2}"

STATUS_1=$(cat "${STATUS_PATH_1}")
STATUS_2=$(cat "${STATUS_PATH_2}")
echo "request-1 status: ${STATUS_1}"
echo "request-1 body:"
jq . < "${RESP_PATH_1}" 2>/dev/null || cat "${RESP_PATH_1}"
echo
echo "request-2 status: ${STATUS_2}"
echo "request-2 body:"
jq . < "${RESP_PATH_2}" 2>/dev/null || cat "${RESP_PATH_2}"

# ============================================================
# Block 4 — Loser-shape assertion + no-double-write invariant
# ============================================================
echo
echo "[concurrent-import] Block 4: side-effect-check"
echo "============================================================"

# Round-3 #3 PIN: the loser must be the reject shape (409
# ConcurrentMutation), NOT wait-then-succeed. The disjunction was
# the recon HYPOTHESIS; the test pins the recon-confirmed truth so a
# future regression that silently flipped reject→wait would fail
# loud here.
WINNER_COUNT=0
LOSER_COUNT=0
UNEXPECTED_STATUSES=()
for s in "${STATUS_1}" "${STATUS_2}"; do
    case "$s" in
    200) WINNER_COUNT=$((WINNER_COUNT + 1)) ;;
    409) LOSER_COUNT=$((LOSER_COUNT + 1)) ;;
    *)   UNEXPECTED_STATUSES+=("$s") ;;
    esac
done
echo "wire-shape tally: winners(200)=${WINNER_COUNT}  losers(409)=${LOSER_COUNT}  other=${UNEXPECTED_STATUSES[*]:-none}"
echo
echo "expected (load-bearing):"
echo "  winners(200) == 1  (one request acquired the lock and imported the CAR"
echo "                       into the now-empty actor store — REAL work)"
echo "  losers(409)  == 1  (the other surfaced ConcurrentMutation, the recon-pinned"
echo "                       round-3 #3 loser-shape from src/error.rs:728-732)"
echo "  the 409 body should include error name 'ConcurrentMutation'"
echo
echo "NOT expected (regression signals):"
echo "  any 500 'Key already exists' → the actor-empty precondition wasn't met;"
echo "                                  Block 2's wipe didn't take effect."
echo "  winners(200) == 2            → lock not enforcing (both ran serially-but-"
echo "                                  uncontended; re-run, or raise concurrency."
echo "                                  If reproducible: the lock regressed.)"
echo "  losers(409)  == 2            → both rejected (one winner ran to completion"
echo "                                  BEFORE either request issued — investigate."
echo "  any 400 'ActorNotInitialized'→ Block 2's wipe took TOO MUCH (cleared the"
echo "                                  plc_keys row); the pre-race check should"
echo "                                  have caught this."

echo
echo "--- forensic log signature on B ---"
echo "expected one 'importRepo concurrent mutation rejected' warn line (the loser):"
grep 'importRepo concurrent mutation rejected' "${B_LOG}" | tail -5 \
    || echo "(NOT FOUND — the lock's warn-emit at repo_import.rs:217 didn't fire)"

echo
echo "expected one 'import_repo_starting' info line (the winner crossed validate-phase):"
grep 'import_repo_starting' "${B_LOG}" | tail -5 \
    || echo "(NOT FOUND — winner never reached :335; investigate)"

# No-double-write invariant — load-bearing teeth. Post-race count
# must equal EXPECTED_IMPORT_RECORD_COUNT (one import's worth). With
# the corrected shape: pre-race=0, post-race=N proves one COMPLETE
# import landed; pre-race=0, post-race!=N means either the winner
# crashed (post=0 / partial), or the lock failed and two imports
# clashed at the unique-key constraint (post varies, often a 500
# from the loser).
POST_RACE_COUNT=$(pb_record_count B_DID)
echo
echo "--- no-double-write invariant: post-race record count ---"
echo "pre-race (post-wipe): ${PRE_RACE_COUNT:-0}"
echo "post-race:            ${POST_RACE_COUNT}"
echo "expected:             ${EXPECTED_IMPORT_RECORD_COUNT}  (one complete import)"
echo
echo "  expected (load-bearing): post-race == ${EXPECTED_IMPORT_RECORD_COUNT}."
echo "  This is the corrected no-double-write invariant — pre-race=0 was the"
echo "  documented v5.2 federation-into-fresh-instance state; post-race==N"
echo "  proves the winner did a full import. Anything OTHER than"
echo "  ${EXPECTED_IMPORT_RECORD_COUNT} indicates either a half-apply (winner"
echo "  crashed) or a double-write (lock failed and the second import created"
echo "  duplicate keys on top — proto-blue's MST would reject the second's"
echo "  Create-on-existing-key with the Block 4 NOT-expected '500 Key already"
echo "  exists' signal)."

# ============================================================
# Block 5 — Decision-point
# ============================================================
echo
echo "[concurrent-import] decision-point:"
echo "  expected (load-bearing):"
echo "    1. winners(200) == 1 and losers(409) == 1"
echo "    2. the 409 response body's error == 'ConcurrentMutation' (round-3 #3 pin)"
echo "    3. post-race record count == ${EXPECTED_IMPORT_RECORD_COUNT}"
echo "       (one complete import, not zero, not double)"
echo "    4. 'importRepo concurrent mutation rejected' warn line in B's log"
echo "    5. one 'import_repo_starting' info line in B's log"
echo "  v0.6 Cluster 3 Member 3.3 (#155) — verification-only, no production touch."
echo "  v0.6+ kill-mid coverage is the natural companion to the cross-process"
echo "  pg_try_advisory_lock variant (src/api/repo_import.rs:57-65), not to"
echo "  this in-process scenario."
echo "  operator: confirm all five signals; the lock holds."
