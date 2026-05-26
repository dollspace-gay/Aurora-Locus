#!/usr/bin/env bash
# Arc 17 Required-mode hard-reject — v0.6 Cluster 3 Member 3.2 (#147).
#
# Counterpart to Scenario 16 (which exercises the Optimistic-mode
# absorb): set VALIDATION_MODE=required on B, replay 16's violating
# write, and assert the hard-reject wire-shape (400 + body error name)
# + the no-commit invariant (per-actor record count unchanged across
# the rejected attempt). All legs of the production chain already
# exist (validate_write → should_propagate_validation_errors → Err →
# apply_writes short-circuits before commit-sign → PdsError →
# IntoResponse); this scenario WITNESSES the chain end-to-end live
# rather than implementing any leg of it.
#
# Source-of-record: docs/V06_DESIGN.md Cluster 3 Member 3.2 +
# docs/internal/v06-recon/V06_CLUSTER3_RECON.md.
#
# Prereqs (same as Scenario 16 — assumes the arc17 sequence has been
# walked through up to and including Scenario 15): B_DID, B_JWT,
# B_ADMIN_JWT, B_ENV, B_DATA. The lexicon for TARGET_NSID is expected
# to be cached from Scenarios 12/14/15 — block 2's SchemaViolation
# assertion depends on that cache state; without it, the validator
# falls through to the unknown-collection Required-mode rejection
# (still 400, wire body shape `Validation`/`InvalidRequest` instead
# of `SchemaViolation`). Both outcomes prove Required mode short-
# circuits — the decision-point names which shape the operator
# should expect based on cache state.

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

# Per-actor SQLite store path (backend-independent per Scenario 16's
# rationale — the `record` table lives only inside the actor store).
# Discovered by walking ${B_DATA}/actors/ because the shard dir is
# hash-derived from a non-bash-reproducible Rust hasher.
pb_record_count() {
    local safe_did="${B_DID//:/_}"
    local actor_db
    actor_db=$(find "${B_DATA}/actors" -type f -name 'store.sqlite' \
        -path "*/${safe_did}/*" -print -quit 2>/dev/null)
    if [ -z "$actor_db" ]; then
        echo "(per-actor store.sqlite not found under ${B_DATA}/actors/*/${safe_did}/ —"
        echo "  has B been seeded yet? Scenario 12 creates the account.)"
        return
    fi
    echo "(operator runs against B's per-actor SQLite — backend-independent:"
    echo "   sqlite3 \"${actor_db}\" \"SELECT count(*) FROM record\""
    echo " for the record-table count assertion.)"
    sqlite3 "$actor_db" "SELECT count(*) FROM record" 2>/dev/null \
        || echo "(sqlite3 not on PATH or store sealed — operator runs the command above)"
}

# ============================================================
# Block 1 — Toggle B to Required mode + ensure validate_imports=true
# ============================================================
echo
echo "[required-reject] Block 1: toggle B to VALIDATION_MODE=required"
echo "============================================================"

# Ensure PDS_LEXICON_VALIDATE_IMPORTS=true (mirror Scenario 16's
# precondition — the override is what makes a validate=false write
# go through the validator at all; without it, validate=false would
# skip validation entirely and the Required-mode short-circuit
# wouldn't fire on this code path).
if ! grep -q "^export PDS_LEXICON_VALIDATE_IMPORTS=true" "$B_ENV"; then
    if grep -q "^export PDS_LEXICON_VALIDATE_IMPORTS=" "$B_ENV"; then
        sed -i 's/^export PDS_LEXICON_VALIDATE_IMPORTS=.*/export PDS_LEXICON_VALIDATE_IMPORTS=true/' "$B_ENV"
    else
        echo "export PDS_LEXICON_VALIDATE_IMPORTS=true" >> "$B_ENV"
    fi
fi

# Toggle VALIDATION_MODE=required (the v0.6 Cluster 3 Member 3.2
# load-bearing switch — flips should_propagate_validation_errors's
# unconditional-true arm under Required mode, surfacing errors that
# Optimistic mode would absorb into a "validation_status: unknown"
# row).
if ! grep -q "^export VALIDATION_MODE=required" "$B_ENV"; then
    if grep -q "^export VALIDATION_MODE=" "$B_ENV"; then
        sed -i 's/^export VALIDATION_MODE=.*/export VALIDATION_MODE=required/' "$B_ENV"
    else
        echo "export VALIDATION_MODE=required" >> "$B_ENV"
    fi
fi

pb_kill_instance b
pb_launch_instance b
pb_wait_for_ready b
echo "(B should be running with VALIDATION_MODE=required and PDS_LEXICON_VALIDATE_IMPORTS=true)"

echo "--- B startup banner (confirm VALIDATION_MODE=required was loaded) ---"
grep -m1 'validation_mode' "$B_LOG" \
    || echo "(no validation_mode line on startup banner — the field is logged at config-load)"

echo "--- baseline record-table count (pre-violation) ---"
BASELINE_COUNT=$(pb_record_count | tail -1)
echo "baseline: $BASELINE_COUNT"

# ============================================================
# Block 2 — Lexicon-violating createRecord under Required mode
# ============================================================
echo
echo "[required-reject] Block 2: violating createRecord under Required mode"
echo "============================================================"

# The same violating payload Scenario 16 uses: missing the required
# `msg` field for TARGET_NSID. Under Optimistic + override-fires the
# response is 200 + absorbed; under Required it must hard-reject.
RESP_PATH=/tmp/scenario-required-reject-resp.json
WRITE_STATUS=$(curl -sX POST "http://localhost:${B_PORT}/xrpc/com.atproto.repo.createRecord" \
    -H "Authorization: Bearer ${B_JWT}" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg repo "$B_DID" --arg collection "$TARGET_NSID" \
        '{repo:$repo, collection:$collection,
          validate:false, record:{not_msg:"missing required field"}}')" \
    -o "$RESP_PATH" -w '%{http_code}')
echo "violating-write status: $WRITE_STATUS"
cat "$RESP_PATH" | jq . 2>/dev/null || cat "$RESP_PATH"

echo
echo "expected (under VALIDATION_MODE=required):"
echo "  status: 400 — Required mode hard-rejects; apply_writes short-circuits"
echo "           before commit-sign (validate_write returns Err; the loop at"
echo "           src/actor_store/repository.rs:514-517 propagates via \`?\`)."
echo "  body error name (cache state-dependent):"
echo "    - 'SchemaViolation' if the TARGET_NSID lexicon is cached (the chain's"
echo "      @lexicon/SchemaViolation tag → PdsError::SchemaViolation, the"
echo "      design's full assertion shape) — requires Scenarios 12/14/15 ran"
echo "      first to seed the cache;"
echo "    - 'InvalidRequest' if the cache is empty (handle_unknown's"
echo "      Required-mode arm → PdsError::Validation) — equally proves the"
echo "      hard-reject chain, but via the umbrella variant instead of the"
echo "      structured one. Either is a true positive."
echo "  NOT 200 — that would mean Required mode is not in effect; check"
echo "  startup banner + \$B_ENV for VALIDATION_MODE=required."

echo
echo "--- post-violation record-table count ---"
POST_COUNT=$(pb_record_count | tail -1)
echo "post-violation: $POST_COUNT"
echo
echo "expected: post-violation count == baseline count (no row written)."
echo "  load-bearing teeth: apply_writes short-circuits on Err BEFORE the"
echo "  record-write / commit-signing block; the no-commit invariant is"
echo "  what makes 'Required hard-rejects' a real test of the chain"
echo "  rather than a tautology of the wire response."

# ============================================================
# Block 3 — Restore VALIDATION_MODE for downstream scenarios
# ============================================================
echo
echo "[required-reject] Block 3: restore B to VALIDATION_MODE (default = optimistic)"
echo "============================================================"

pb_kill_instance b
# Removing the explicit Required override lets the launch fall back
# to the codebase default (Optimistic per validation/mod.rs:22's
# `#[default]`). Other arc17 scenarios assume Optimistic; leaving B
# in Required mode would silently break their absorb assertions.
sed -i '/^export VALIDATION_MODE=/d' "$B_ENV"
pb_launch_instance b
pb_wait_for_ready b

echo
echo "[required-reject] decision-point:"
echo "  expected: violating write status = 400 (NOT 200);"
echo "            body error name = SchemaViolation (cache seeded) or"
echo "            InvalidRequest (cache empty) — both prove Required hard-rejects;"
echo "            post-violation record count == baseline count (no commit)."
echo "  v0.6 Cluster 3 Member 3.2 (#147) — pure-test, no production-code edit."
echo "  operator: confirm the three signals before moving on; B is now back"
echo "            in Optimistic mode for downstream scenarios."
