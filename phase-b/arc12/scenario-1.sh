#!/usr/bin/env bash
# Arc 12 Scenario 1 — Mint-then-flip: live verification of typed
# DidTombstoned propagation through cross-PDS service-auth (Cluster 2
# Member 2.2 / chainlink #144 + #134).
#
# What this proves: after #144, the typed `PdsError::DidTombstoned`
# variant emitted by `fetch_plc_document` (PLC-410 → typed mapping per
# Arc 13 v4.2) propagates ALL the way to the HTTP wire as
# `400 Bad Request` with body `{"error": "DidTombstoned", ...}`. Pre-
# #144 it would surface as `401 Unauthorized` with opaque
# `{"error": "Authentication", ...}` because the verifier paths
# swallowed the typed variant via `.map_err` wraps and the outer
# callers wrapped again with hardcoded `"Invalid token"` /
# `"service-auth verification failed: …"` Authentication.
#
# The 400-vs-401 wire diff IS the verification. #134 closes when this
# scenario lands HTTP 400.
#
# Cross-PDS surface: scenario uses the ADMIN-authenticated route via
# `admin_auth_from_token` (auth.rs:268+ → Layer 4 at :344-371). This is
# the live caller of the free-fn `service_auth::verify_service_jwt`
# that Member 2.2 site 7 patched. An A-minted ES256K service-auth JWT
# presented to a B admin endpoint triggers the AdminAuthContext
# extractor → falls through Layers 1-3 (not a local session, not
# HS256, passes ES256K pre-check) → Layer 4 calls
# verify_service_jwt(token, B.service_did, B.identity_resolver) →
# resolver hits mock-PLC 410 → fetch_plc_document maps to
# PdsError::DidTombstoned → site 5 maps to
# ServiceAuthError::DidTombstoned → site 7's pattern-match propagates
# via .into() → site 4's From impl routes to PdsError::DidTombstoned →
# IntoResponse → HTTP 400 `{"error": "DidTombstoned", ...}`.
#
# Why NOT the non-admin path via require_auth_forwarded /
# route_service_auth_fallback (site 8's target): that path short-
# circuits BEFORE the verifier at `ctx.is_trusted_iss(iss)`
# (auth.rs:1299-1305) — B only trusts the per-config peer-PDS DIDs +
# its own service DID + the entryway. A test account's DID isn't in
# that allowlist (the harness doesn't pre-populate it; it can't,
# because A creates the DID after B launches). So the federation
# method's site-1 + site-8 fixes are exercised by integration tests
# (when route_service_auth_fallback gets a trusted iss with a
# tombstoned DID), not by this scenario. Site 7 is the only
# typed-propagation path reachable from a fresh-account mint-then-
# flip without harness-config plumbing.
#
# Topology:
#   Block 1 — Setup-to-confirmed-up (mock-PLC + A + B)
#   Block 2 — Create test account on A (per-run UNIQUE handle/email
#             for Tier-2 cache-defeat — see below)
#   Block 3 — Mint a service-auth JWT for the test account, audience
#             = B's service DID (calls A's getServiceAuth XRPC under
#             test-account session JWT; #143's per-account-key fix is
#             NOT required for this scenario — the verifier's
#             tombstone-check fires before signature verification,
#             so any minted JWT suffices)
#   Block 4 — Tombstone the test account via dev.aurora.tombstoneDid
#             on A (mock-PLC `plc_tombstone` POST → state.tombstoned
#             = true)
#   Block 5 — Confirm mock-PLC now returns HTTP 410 for the
#             tombstoned DID (per Cluster 1 Member 1.4's permanent
#             tombstone-410 patch landed this cycle)
#   Block 6 — Present the minted JWT to B's authenticated XRPC
#             surface; capture HTTP status + body
#   Block 7 — Side-effect-check (operator judges): the 400-vs-401
#             diff is the test
#
# Cache-defeat (two tiers; both required to make the typed
# DidTombstoned REACH the verifier's match arm at all — without them
# the identity resolver's graceful-degradation arm at
# resolver.rs:211-219 returns the stale pre-tombstone `cached_doc`
# Ok(_) and the verifier sees no error to propagate):
#
#   Tier 1 — Fresh data dirs (harness default via Block 1's
#            pb_fresh_data_dir b). B's did_cache_db is empty at
#            launch.
#   Tier 2 — Per-run UNIQUE iss DID (scenario-script-only, ZERO code
#            change). Achieved by generating per-run-unique handle/
#            email so dev.aurora.createAccount mints a fresh DID
#            every invocation. Belt-and-suspenders against any
#            future scenario edit that would pre-warm B's cache.
#
# Backend handling — backend-transparent (run-once class). The
# scenario's primary assertion is the cross-PDS auth verifier's wire
# shape; the tombstone path doesn't touch backend-divergent DB writes.
# The account-create on A IS backend-divergent (it writes to A's
# account_db) but that's setup, not the asserted behavior — and B's
# verify path queries mock-PLC over HTTP, not B's DB. Standalone run
# for now (not in arc17's run-all.sh); when the arc12 orchestrator
# lands later this scenario joins the transparent-once group.
#
# Source-of-record: V06_DESIGN.md Cluster 2 Member 2.2.

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

# ============================================================
# Block 1 — Setup-to-confirmed-up (mock-PLC + A + B)
# ============================================================
echo
echo "[scenario-arc12-1] Block 1: setup-to-confirmed-up"
echo "============================================================"

# Tier-2 cache-defeat: per-run-unique handle + email so the test
# account mints a fresh DID on every invocation. B's did_cache_db
# guaranteed to miss on iss resolution at verify time regardless of
# residual harness state. Also disambiguates parallel runs.
RUN_SUFFIX=$(date +%s)
TEST_HANDLE="tombstone-target-${RUN_SUFFIX}.localhost"
TEST_EMAIL="tombstone-target-${RUN_SUFFIX}@localhost"
TEST_PASSWORD="phase-b-arc12-mint-then-flip-${RUN_SUFFIX}"
echo "[scenario-arc12-1] per-run unique account: ${TEST_HANDLE}"

pb_mock_plc_start
pb_mock_plc_wait

pb_kill_prior
pb_fresh_data_dir a
pb_fresh_data_dir b

# A: standard launch — no lexicon-specific config needed for this
# scenario (the verifier path doesn't touch lexicon resolution).
unset PDS_LEXICON_ENABLED
unset PDS_LEXICON_DID_AUTHORITY
pb_env_emit_role a
pb_launch_instance a
pb_wait_for_ready a
pb_grep_banner a

# B: standard launch. B doesn't need lexicon either — its only role
# in this scenario is to receive the minted JWT and run it through
# verify_service_jwt (which resolves iss against mock-PLC).
pb_env_emit_role b
pb_launch_instance b
pb_wait_for_ready b
pb_grep_banner b

# ============================================================
# Block 2 — Create test account on A (per-run UNIQUE for Tier-2)
# ============================================================
echo
echo "[scenario-arc12-1] Block 2: create test account on A"
echo "============================================================"

CREATE_RESP=$(curl -sX POST "http://localhost:${A_PORT}/xrpc/dev.aurora.createAccount" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc \
        --arg handle "$TEST_HANDLE" \
        --arg email "$TEST_EMAIL" \
        --arg password "$TEST_PASSWORD" \
        '{handle:$handle, email:$email, password:$password}')")
TEST_DID=$(echo "$CREATE_RESP" | jq -r '.did // empty')
TEST_JWT=$(echo "$CREATE_RESP" | jq -r '.accessJwt // empty')
echo "test DID:      ${TEST_DID}"
echo "test JWT len:  ${#TEST_JWT}"
case "$TEST_DID" in
did:plc:*) : ;;
*)
    echo "[scenario-arc12-1] BAD test DID '$TEST_DID' — abort" >&2
    echo "create-response body:" >&2
    echo "$CREATE_RESP" | jq . >&2
    exit 1
    ;;
esac

# ============================================================
# Block 3 — Mint service-auth JWT (audience = B's service DID)
# ============================================================
echo
echo "[scenario-arc12-1] Block 3: mint service-auth JWT for test DID"
echo "============================================================"

# B's service DID — env.sh emits `PDS_SERVICE_DID=did:web:localhost%3A$port`
# (literal `%3A` in the env var, which becomes the runtime value of
# state.service_did()). The verifier checks `claims.aud` byte-equal
# against state.service_did() per §5.5.6 — no normalization. So the
# JWT must carry the literal `%3A` in its aud claim.
#
# The audience-encoding subtlety: passing `did:web:localhost%3A2584`
# as a raw query-string arg gets URL-DECODED by axum's Query
# extractor (% sequences are interpreted on the wire). So
# `?aud=did:web:localhost%3A2584` arrives at the handler as
# `did:web:localhost:2584` — wrong shape. Pass it through
# `--data-urlencode` with `-G` so curl URL-encodes the literal `%`
# to `%25` on the wire (`?aud=did%3Aweb%3Alocalhost%253A2584`),
# which axum decodes back to `did:web:localhost%3A2584` — matching
# env.sh's emit + B's state.service_did().
B_SERVICE_DID="did:web:localhost%3A${B_PORT}"
echo "audience (B service DID): ${B_SERVICE_DID}"

MINT_RESP=$(curl -sG "http://localhost:${A_PORT}/xrpc/com.atproto.server.getServiceAuth" \
    --data-urlencode "aud=${B_SERVICE_DID}" \
    -H "Authorization: Bearer ${TEST_JWT}")
MINTED_JWT=$(echo "$MINT_RESP" | jq -r '.token // empty')
echo "minted JWT len: ${#MINTED_JWT}"
if [ -z "$MINTED_JWT" ] || [ "$MINTED_JWT" = "null" ]; then
    echo "[scenario-arc12-1] getServiceAuth failed to return a token — abort" >&2
    echo "mint-response body:" >&2
    echo "$MINT_RESP" | jq . >&2
    exit 1
fi
# Echo the JWT's three-segment shape sanity-check (header.claims.sig).
MINTED_SEG_COUNT=$(echo -n "$MINTED_JWT" | tr -cd '.' | wc -c)
if [ "$MINTED_SEG_COUNT" != "2" ]; then
    echo "[scenario-arc12-1] minted JWT does not have 3 dot-separated segments — abort" >&2
    exit 1
fi

# ============================================================
# Block 4 — Tombstone the test DID via dev.aurora.tombstoneDid on A
# ============================================================
echo
echo "[scenario-arc12-1] Block 4: tombstone test DID on A"
echo "============================================================"

TS_RESP=$(curl -sX POST "http://localhost:${A_PORT}/xrpc/dev.aurora.tombstoneDid" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg did "$TEST_DID" '{did:$did}')")
echo "tombstone response:"
echo "$TS_RESP" | jq .
echo "expected: { did, tombstoneCid, prevCid }"

# ============================================================
# Block 5 — Confirm mock-PLC returns HTTP 410 for tombstoned DID
# ============================================================
echo
echo "[scenario-arc12-1] Block 5: confirm mock-PLC 410 for tombstoned DID"
echo "============================================================"

PLC_STATUS=$(curl -s -o /dev/null -w '%{http_code}' "${PDS_DID_PLC_URL}/${TEST_DID}")
echo "mock-PLC GET ${PDS_DID_PLC_URL}/${TEST_DID} -> HTTP ${PLC_STATUS}"
echo "expected: 410 (Cluster 1 Member 1.4 permanent tombstone-410 patch)"
if [ "$PLC_STATUS" != "410" ]; then
    echo "[scenario-arc12-1] mock-PLC did not return 410 — the v0.6 permanent" >&2
    echo "tombstone-410 patch may have regressed in phase-b/mock-plc.py" >&2
    echo "(M1.4 / chainlink #109). Aborting before the verify step." >&2
    exit 1
fi

# ============================================================
# Block 6 — Present minted JWT to B admin endpoint; capture status + body
# ============================================================
echo
echo "[scenario-arc12-1] Block 6: present minted JWT to B admin endpoint"
echo "============================================================"

# Pick a B admin endpoint that runs the AdminAuthContext extractor →
# admin_auth_from_token → Layer 4 (free-fn verify_service_jwt). Any
# tools.aurora.* admin handler works; tools.aurora.lexicon.getCacheState
# is GET so no body to construct, and it's the same endpoint Scenario
# 9 admin-gate-tested last cycle (proves the extractor wiring).
#
# The verifier runs BEFORE finalize_admin_role's role lookup — so even
# though the test account doesn't have admin role on B, the verifier
# rejects the tombstoned-iss JWT with 400 well before the role gate
# fires. Post-#144, that's HTTP 400 DidTombstoned. Pre-#144 (or under
# a regression at sites 5/7), it would be HTTP 401 Authentication.
B_RESP_PATH=/tmp/scenario-arc12-1-b-resp.json
B_STATUS=$(curl -sX GET "http://localhost:${B_PORT}/xrpc/tools.aurora.lexicon.getCacheState" \
    -H "Authorization: Bearer ${MINTED_JWT}" \
    -o "$B_RESP_PATH" \
    -w '%{http_code}')
echo "B tools.aurora.lexicon.getCacheState with minted JWT: HTTP ${B_STATUS}"
echo "B response body:"
cat "$B_RESP_PATH" | jq . 2>/dev/null || cat "$B_RESP_PATH"
echo

# ============================================================
# Block 7 — Side-effect-check (operator judges)
# ============================================================
echo
echo "[scenario-arc12-1] Block 7: side-effect-check"
echo "============================================================"

B_LOG="/tmp/pds-b-${BACKEND}.log"

echo "--- B service-auth tracing for tombstoned DID (expected: at least one"
echo "    'issuer DID tombstoned' line per Member 2.2 site 6) ---"
grep -E 'service-auth: issuer DID tombstoned|DidTombstoned' "$B_LOG" | tail -5 \
    || echo "(NOT FOUND — typed-propagation may not have fired; check site 1/5/7/8)"

echo
echo "[scenario-arc12-1] decision-point:"
echo "  expected (POST-#144):"
echo "    B HTTP status = 400"
echo "    B body shape  = { \"error\": \"DidTombstoned\", \"message\": ... }"
echo "    B log         = at least one 'service-auth: issuer DID tombstoned'"
echo "                    (Member 2.2 site 6 tracing arm, exhaustive-match"
echo "                     compile requirement)"
echo "    mock-PLC      = 410 GET for ${TEST_DID} (Block 5 confirmed)"
echo
echo "  PRE-#144 wire shape (the diff that IS the verification):"
echo "    B HTTP status = 401"
echo "    B body shape  = { \"error\": \"Authentication\", \"message\":"
echo "                      \"Invalid token\" }  (route_service_auth_fallback"
echo "                       hardcoded swallow, pre-site-8)"
echo "                    OR \"service-auth verification failed: ...\""
echo "                       (admin extractor explicit wrap, pre-site-7)"
echo
echo "  operator: confirm 400-vs-401. Run #134 is closed when this scenario"
echo "  lands HTTP 400 against B with a typed DidTombstoned body. The 401"
echo "  shape is the v0.5 / pre-#144 baseline; HTTP 400 is the post-#144"
echo "  intended wire shape."
echo
echo "  scope of test: this scenario asserts the verifier's typed-propagation"
echo "  shape only. It does NOT depend on Member 2.1 (#143)'s per-account"
echo "  signing-key fix — the verifier's tombstone-check fires at"
echo "  resolve_did(iss) BEFORE signature verification, so any minted JWT"
echo "  reaches the tombstone check regardless of which key signed it."
echo "  Per-account-key correctness is exercised separately by Cluster 2"
echo "  Member 2.1's unit test (test_create_service_jwt_signature_verifies_"
echo "  against_per_account_key_only)."
echo
echo "  backend-transparent (run-once class): the assertion is the wire"
echo "  shape of B's verify path, which doesn't touch backend-divergent DB"
echo "  writes. The account-create on A is backend-divergent but is SETUP,"
echo "  not the asserted behavior."
