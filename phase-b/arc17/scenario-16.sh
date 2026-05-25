#!/usr/bin/env bash
# Arc 17 Scenario 16 — validate_imports override fires on validate:false
# writes (Optimistic absorption preserved).
#
# §17.3.4 PDS_LEXICON_VALIDATE_IMPORTS=true makes the validator FIRE on
# validate=Some(false) writes for unknown NSIDs that would otherwise be
# bypassed pre-Arc-17. Under Optimistic (default), the resulting
# SchemaViolation is absorbed → HTTP 200 + track_validation_failure row.
# The 400/no-commit outcome holds only under ValidationMode::Required
# (unit-covered; v0.6 Phase B candidate).
#
# Source-of-record: docs/internal/arc17-phase-b-commands.md Scenario 16
# (post-#138 correction).

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

# shellcheck source=../lib/env.sh
source phase-b/lib/env.sh
# shellcheck source=../lib/instance.sh
source phase-b/lib/instance.sh
pb_env_init
pb_env_echo_confirm

: "${B_PORT:?B_PORT must be set}"
: "${B_DID:?Scenario 12 must run first}"
: "${B_JWT:?Scenario 12 must run first}"
: "${B_ADMIN_JWT:?Scenario 12 must run first}"
: "${B_ENV:?Scenario 2 must run first}"
: "${B_DATA:?B_DATA must be set}"

B_LOG="/tmp/pds-b-${BACKEND}.log"
TARGET_NSID="com.example.lexicon.target"

# Helper: inspect record-table count via the active backend.
pb_record_count() {
    local query="SELECT count(*) FROM record WHERE did='${B_DID}'"
    case "$BACKEND" in
    sqlite)
        # Per-actor SQLite at ${B_DATA}/actors/<did-shard>/<did>/store.sqlite —
        # operators run their own DB inspect commands; here we just probe
        # an account-level count via the account_db (records replicate in
        # the per-account record table at account_db only for some
        # surfaces; the side-effect proof we WANT is the warn log).
        # Print a placeholder and hand the inspect command to the operator.
        echo "(BACKEND=sqlite — operator runs:"
        echo "   sqlite3 \"${B_DATA}/account.sqlite\" \"${query}\""
        echo " for the record-table count assertion.)"
        ;;
    postgres)
        echo "(BACKEND=postgres — operator runs:"
        echo "   docker exec aurora-phase-b-pg-b psql -U aurora -d aurora -At -c \"${query}\""
        echo " for the record-table count assertion.)"
        ;;
    esac
}

# ============================================================
# Block 1 — Baseline count + cache state; ensure validate_imports=true
# ============================================================
echo
echo "[scenario-16] Block 1: baseline + ensure validate_imports=true"
echo "============================================================"

if ! grep -q "^export PDS_LEXICON_VALIDATE_IMPORTS=" "$B_ENV"; then
    echo "export PDS_LEXICON_VALIDATE_IMPORTS=true" >> "$B_ENV"
    pb_kill_instance b
    pb_launch_instance b
    pb_wait_for_ready b
elif ! grep -q "^export PDS_LEXICON_VALIDATE_IMPORTS=true" "$B_ENV"; then
    sed -i 's/^export PDS_LEXICON_VALIDATE_IMPORTS=.*/export PDS_LEXICON_VALIDATE_IMPORTS=true/' "$B_ENV"
    pb_kill_instance b
    pb_launch_instance b
    pb_wait_for_ready b
fi
echo "(B should be running with PDS_LEXICON_VALIDATE_IMPORTS=true)"

echo "--- baseline record-table count ---"
pb_record_count
echo "--- ensure lexicon cached for ${TARGET_NSID} ---"
curl -sf "http://localhost:${B_PORT}/xrpc/tools.aurora.lexicon.getCacheState?nsid=${TARGET_NSID}" \
    -H "Authorization: Bearer ${B_ADMIN_JWT}" | jq '.entries | length'
echo "expected: 1 (cached from Scenario 12/15)"

# ============================================================
# Block 2 — Bad-record write with validate=false; override fires
# (under Optimistic: HTTP 200 + warn log; SchemaViolation absorbed)
# ============================================================
echo
echo "[scenario-16] Block 2: bad-record validate=false write"
echo "============================================================"

RESP_PATH=/tmp/scenario-16-resp.json
WRITE_STATUS=$(curl -sX POST "http://localhost:${B_PORT}/xrpc/com.atproto.repo.createRecord" \
    -H "Authorization: Bearer ${B_JWT}" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg repo "$B_DID" --arg collection "$TARGET_NSID" \
        '{repo:$repo, collection:$collection,
          validate:false, record:{not_msg:"missing required field"}}')" \
    -o "$RESP_PATH" -w '%{http_code}')
echo "override-fired write status: $WRITE_STATUS"
cat "$RESP_PATH" | jq . || true
echo
echo "expected (under Optimistic, the default):"
echo "  status: 200 — SchemaViolation absorbed (per #137 bypass-set design)"
echo "  log: 'Validation failed ... accepting in Optimistic mode' line on B"
echo "  (the 400 + no-commit shape holds only under ValidationMode::Required)"

echo "--- last 1 'accepting in Optimistic mode' line ---"
grep 'accepting in Optimistic mode' "$B_LOG" | tail -1 \
    || echo "(NOT FOUND — override didn't fire; check PDS_LEXICON_VALIDATE_IMPORTS)"

# ============================================================
# Block 3 — Toggle: PDS_LEXICON_VALIDATE_IMPORTS=false → validator
# bypassed; NO warn line for the equivalent write.
# ============================================================
echo
echo "[scenario-16] Block 3: toggle validate_imports=false"
echo "============================================================"

pb_kill_instance b
sed -i 's/^export PDS_LEXICON_VALIDATE_IMPORTS=.*/export PDS_LEXICON_VALIDATE_IMPORTS=false/' "$B_ENV"
pb_launch_instance b
pb_wait_for_ready b

curl -sX POST "http://localhost:${B_PORT}/xrpc/com.atproto.repo.createRecord" \
    -H "Authorization: Bearer ${B_JWT}" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg repo "$B_DID" --arg collection "$TARGET_NSID" \
        '{repo:$repo, collection:$collection,
          validate:false, record:{not_msg:"missing required field 2"}}')" \
    -o /dev/null -w '%{http_code}'
echo ""

echo "--- count of 'accepting in Optimistic mode' lines since this restart ---"
grep -c 'accepting in Optimistic mode' "$B_LOG" || echo "0"
echo "expected: 0 since restart (validator bypassed under validate_imports=false)"

# ============================================================
# Block 4 — Restore validate_imports=true for downstream
# ============================================================
echo
echo "[scenario-16] Block 4: restore validate_imports=true"
echo "============================================================"

pb_kill_instance b
sed -i 's/^export PDS_LEXICON_VALIDATE_IMPORTS=.*/export PDS_LEXICON_VALIDATE_IMPORTS=true/' "$B_ENV"
pb_launch_instance b
pb_wait_for_ready b

echo
echo "[scenario-16] decision-point:"
echo "  expected: status 200 + 'accepting in Optimistic mode' log line for the"
echo "            validate_imports=true write (override fired, SchemaViolation absorbed);"
echo "            NO warn line for the validate_imports=false write (override didn't fire);"
echo "            under Optimistic, both writes COMMIT (record-table PRE+2)."
echo "  v0.6 note: Required-mode hard-reject is unit-covered; live coverage is a"
echo "  separate Phase B candidate (see Cluster 3 Member 3.2 / #147)."
echo "  operator: confirm before Scenario 10."
