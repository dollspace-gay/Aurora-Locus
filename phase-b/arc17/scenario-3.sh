#!/usr/bin/env bash
# Arc 17 Scenario 3 — Host the lexicon record on A.
#
# Seeds a fetchable com.atproto.lexicon.schema record at
# A_DID/<target.nsid>. A's validate-phase falls through to Optimistic
# because enabled=false; no recursion concern.
#
# Source-of-record: docs/internal/arc17-phase-b-commands.md Scenario 3.
# Prerequisite: Scenario 2 left A running with A_DID + A_JWT in env.

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

# shellcheck source=../lib/env.sh
source phase-b/lib/env.sh
pb_env_init
pb_env_echo_confirm

: "${A_DID:?Scenario 2 must run first to seed A_DID}"
: "${A_JWT:?Scenario 2 must run first to seed A_JWT}"
: "${A_PORT:?A_PORT must be set}"

TARGET_NSID="com.example.lexicon.target"

# ============================================================
# Block 1 — Scenario-call: host the lexicon record on A
# ============================================================
echo
echo "[scenario-3] Block 1: host lexicon record on A"
echo "============================================================"
echo "A_DID = $A_DID"
echo "TARGET_NSID = $TARGET_NSID"

LEXICON_JSON=$(cat <<JSON
{
  "lexicon": 1,
  "id": "$TARGET_NSID",
  "defs": {
    "main": {
      "type": "record",
      "key": "tid",
      "record": {
        "type": "object",
        "required": ["msg"],
        "properties": {
          "msg": { "type": "string" }
        }
      }
    }
  }
}
JSON
)

HOST_RESP=$(curl -sX POST "http://localhost:${A_PORT}/xrpc/com.atproto.repo.createRecord" \
    -H "Authorization: Bearer ${A_JWT}" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc \
        --arg repo "$A_DID" \
        --arg collection "com.atproto.lexicon.schema" \
        --arg rkey "$TARGET_NSID" \
        --argjson record "$LEXICON_JSON" \
        '{repo:$repo, collection:$collection, rkey:$rkey, record:$record}')")
echo "host response: $HOST_RESP"
echo "expected: { uri: 'at://${A_DID}/com.atproto.lexicon.schema/${TARGET_NSID}', cid: 'bafy...' }"

# ============================================================
# Block 2 — Side-effect-check: readback CAR via sync.getRecord
# ============================================================
echo
echo "[scenario-3] Block 2: side-effect-check via sync.getRecord"
echo "============================================================"

CAR_PATH="/tmp/scenario-3-readback.car"
curl -sf -o "$CAR_PATH" \
    "http://localhost:${A_PORT}/xrpc/com.atproto.sync.getRecord?did=${A_DID}&collection=com.atproto.lexicon.schema&rkey=${TARGET_NSID}" \
    || echo "(curl exited non-zero; sync.getRecord may have 4xx'd)"

CAR_BYTES=$(stat -c '%s' "$CAR_PATH" 2>/dev/null || echo "0")
echo "CAR bytes: $CAR_BYTES"
echo "expected: > 100 (a non-trivial CAR; 0 = NotFound; abort if < 100)"

echo
echo "[scenario-3] decision-point:"
echo "  TARGET_NSID = $TARGET_NSID  (used by Scenarios 12/15/16)"
echo "  operator: confirm host response has uri+cid AND readback CAR > 100 bytes."
