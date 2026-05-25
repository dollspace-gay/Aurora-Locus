#!/usr/bin/env bash
# Arc 17 Scenario 13 — Live-DNS strict-parse rejection (Cluster 1
# Member 1.2).
#
# Exercises the production HickoryDnsTxtResolver against three crafted
# DNS responses served by the in-tree custom-UDP responder
# (phase-b/dns_responder.rs, launched via lib/mock-dns.sh). The PDS
# resolves through a real `_lexicon.<authority>` TXT lookup — NO
# PDS_LEXICON_DID_AUTHORITY override; the override path is Scenario 10's
# specialty and would silently bypass the resolver here. The PDS is
# pointed at the mock responder via `PDS_LEXICON_DNS_NAMESERVER=127.0.0.1:5353`
# (the Phase-B-only constructor-injection knob added in M1.2 piece (c) at
# src/federation/dns_resolver.rs).
#
# Three sub-cases:
#
# - 13a — DNS authority returns TWO TXT records for one _lexicon name.
#   Live resolver returns Vec<String> length 2 -> strict-parse rejects
#   ambiguity -> validate-phase rejects with HTTP 502
#   LexiconAuthorityAmbiguous.
# - 13b — ONE TXT with TWO `did=` entries (two character-strings in one
#   record). Joined-chunks parse yields "did=did:plc:xyz1 did=did:plc:xyz2"
#   which contains two `did=` substrings -> strict-parse rejects.
# - 13c — ONE TXT, malformed (uppercase `DID=`, leading/trailing
#   whitespace). Strict-parse rejects (no `did=` prefix in the
#   case-sensitive match).
#
# Cache-defeat: per-sub-case unique NSIDs (Layer 3, load-bearing) +
# responder TTL=0 (Layer 1) + cache_size=0 on the Phase-B-only resolver
# constructor (Layer 2). The three NSIDs derive distinct authority
# hostnames per `nsid_authority` (reverse all-segments-minus-last):
#   com.example.test13a.foo -> test13a.example.com
#   com.example.test13b.foo -> test13b.example.com
#   com.example.test13c.foo -> test13c.example.com
#
# Side-effect-check the script HANDS the operator (does NOT auto-assert):
#   - wire status = 502, body { error: "LexiconAuthorityAmbiguous", ... }
#   - log: `lexicon_fetch_failed` with `failure_class = authority_ambiguous`
#   - log: `lexicon_dns_lookup` present AND `lexicon_authority_override_used`
#     absent (both halves load-bearing; chainlink #142)
#   - cache row count for the rejected NSID = 0
#
# Source-of-record: V06_DESIGN.md Cluster 1 Member 1.2.

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
# shellcheck source=../lib/mock-dns.sh
source phase-b/lib/mock-dns.sh
# shellcheck source=../lib/creds.sh
source phase-b/lib/creds.sh

pb_env_init
pb_env_echo_confirm

# ============================================================
# Per-sub-case constants — load-bearing for cache-defeat Layer 3.
# Each sub-case has its OWN authority hostname derived from its own
# NSID. The mock responder serves all three configs simultaneously
# (one HashMap; queries land on distinct keys).
# ============================================================

DNS_NAMESERVER="127.0.0.1:5353"
MOCK_DNS_CONFIG="/tmp/phase-b-scenario-13-dns.json"

NSID_13A="com.example.test13a.foo"
NSID_13B="com.example.test13b.foo"
NSID_13C="com.example.test13c.foo"

# Synthesize DIDs the responder will hand back. The strict-parse
# rejects the SHAPE (ambiguity), not the DID value — these can be
# arbitrary as long as they parse as did:plc: at the syntactic level.
DID_A1="did:plc:test13a000000000000001"
DID_A2="did:plc:test13a000000000000002"
DID_B1="did:plc:test13b000000000000001"
DID_B2="did:plc:test13b000000000000002"
DID_C1="did:plc:test13c000000000000001"

# ============================================================
# Block 1 — Setup-to-confirmed-up
# (Scenario 13 is single-instance; only B is needed because B is
# the lexicon-consumer. Skipping the two-instance bootstrap from
# Scenario 2.)
# ============================================================
echo
echo "[scenario-13] Block 1: setup-to-confirmed-up"
echo "============================================================"

# Mock PLC up (idempotent).
pb_mock_plc_start
pb_mock_plc_wait

# Write the mock-DNS config. All three sub-cases share one responder
# instance; queries on distinct names disambiguate.
cat > "$MOCK_DNS_CONFIG" <<JSON
{
  "records": [
    {
      "name": "_lexicon.test13a.example.com",
      "txt_records": [
        ["did=${DID_A1}"],
        ["did=${DID_A2}"]
      ]
    },
    {
      "name": "_lexicon.test13b.example.com",
      "txt_records": [
        ["did=${DID_B1}", "did=${DID_B2}"]
      ]
    },
    {
      "name": "_lexicon.test13c.example.com",
      "txt_records": [
        ["  DID=${DID_C1}  "]
      ]
    }
  ]
}
JSON
echo "[scenario-13] mock-dns config at $MOCK_DNS_CONFIG"

# Launch the mock-DNS responder.
MOCK_DNS_BIND="$DNS_NAMESERVER" pb_mock_dns_start "$MOCK_DNS_CONFIG"
MOCK_DNS_BIND="$DNS_NAMESERVER" pb_mock_dns_wait

# Kill prior PDS; fresh data dir for B.
pb_kill_prior
pb_fresh_data_dir b

# B env: LEXICON-CONSUMER, NO did_authority override (this is the
# crucial difference from Scenarios 2/12), PDS_LEXICON_DNS_NAMESERVER
# pointed at the mock responder. fetch_failure_behavior=hard_fail so
# 13a/b/c surface the 502 directly (warn would absorb).
export PDS_LEXICON_ENABLED=true
unset PDS_LEXICON_DID_AUTHORITY
export PDS_LEXICON_FETCH_FAILURE_BEHAVIOR=hard_fail
export PDS_LEXICON_FETCH_TIMEOUT_SECS=10
export PDS_LEXICON_DNS_NAMESERVER="$DNS_NAMESERVER"
pb_env_emit_role b

pb_launch_instance b
pb_wait_for_ready b
pb_grep_banner b

# Seed a B-side account to drive createRecord against.
pb_create_account b "bob.localhost" "bob@localhost" "phase-b-arc17-13"
pb_echo_creds b

# ============================================================
# Block 2 — Scenario-call 13a (two TXT records for one name)
# ============================================================
echo
echo "[scenario-13] Block 2: 13a — two TXT records (ambiguous)"
echo "============================================================"

RESP_PATH_13A=/tmp/scenario-13a-body.json
WRITE_STATUS_13A=$(curl -sX POST "http://localhost:${B_PORT}/xrpc/com.atproto.repo.createRecord" \
    -H "Authorization: Bearer ${B_JWT}" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg repo "$B_DID" --arg collection "$NSID_13A" \
        '{repo:$repo, collection:$collection, record:{msg:"13a ambiguity"}}')" \
    -o "$RESP_PATH_13A" -w '%{http_code}')
echo "13a write status: $WRITE_STATUS_13A"
cat "$RESP_PATH_13A" | jq . 2>/dev/null || cat "$RESP_PATH_13A"
echo
echo "expected: HTTP 502, body { error: 'LexiconAuthorityAmbiguous', ... }"

# ============================================================
# Block 3 — Scenario-call 13b (one TXT, two did= entries)
# ============================================================
echo
echo "[scenario-13] Block 3: 13b — one TXT with two did= entries"
echo "============================================================"

RESP_PATH_13B=/tmp/scenario-13b-body.json
WRITE_STATUS_13B=$(curl -sX POST "http://localhost:${B_PORT}/xrpc/com.atproto.repo.createRecord" \
    -H "Authorization: Bearer ${B_JWT}" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg repo "$B_DID" --arg collection "$NSID_13B" \
        '{repo:$repo, collection:$collection, record:{msg:"13b ambiguity"}}')" \
    -o "$RESP_PATH_13B" -w '%{http_code}')
echo "13b write status: $WRITE_STATUS_13B"
cat "$RESP_PATH_13B" | jq . 2>/dev/null || cat "$RESP_PATH_13B"
echo
echo "expected: HTTP 502, body { error: 'LexiconAuthorityAmbiguous', ... }"

# ============================================================
# Block 4 — Scenario-call 13c (malformed TXT — uppercase DID=, whitespace)
# ============================================================
echo
echo "[scenario-13] Block 4: 13c — malformed TXT (uppercase + whitespace)"
echo "============================================================"

RESP_PATH_13C=/tmp/scenario-13c-body.json
WRITE_STATUS_13C=$(curl -sX POST "http://localhost:${B_PORT}/xrpc/com.atproto.repo.createRecord" \
    -H "Authorization: Bearer ${B_JWT}" \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg repo "$B_DID" --arg collection "$NSID_13C" \
        '{repo:$repo, collection:$collection, record:{msg:"13c malformed"}}')" \
    -o "$RESP_PATH_13C" -w '%{http_code}')
echo "13c write status: $WRITE_STATUS_13C"
cat "$RESP_PATH_13C" | jq . 2>/dev/null || cat "$RESP_PATH_13C"
echo
echo "expected: HTTP 502, body { error: 'LexiconAuthorityAmbiguous' or similar"
echo "          strict-parse rejection — depending on parse_did_from_txt's"
echo "          taxonomy mapping for uppercase/whitespace, the error variant"
echo "          may surface as the generic AmbiguousAuthority or a more"
echo "          specific InvalidResponseStructure-equivalent. Operator"
echo "          confirms the rejection happened (status >= 400) and that"
echo "          NO record was committed."

# ============================================================
# Block 5 — Side-effect-check: distinguishing-log proof (chainlink #142)
# ============================================================
echo
echo "[scenario-13] Block 5: side-effect-check — log proof"
echo "============================================================"

B_LOG="/tmp/pds-b-${BACKEND}.log"

echo "--- lexicon_dns_lookup events (expected: 3+, one per sub-case) ---"
# Bare-event-name grep (Pretty subscriber inserts ANSI between the
# `event` field name and the value; never grep with `event:` quoted).
grep -E 'lexicon_dns_lookup' "$B_LOG" | tail -5 \
    || echo "(NOT FOUND — proves the live resolver path did NOT fire; check PDS_LEXICON_DID_AUTHORITY is unset on B and PDS_LEXICON_DNS_NAMESERVER is set)"

echo
echo "--- lexicon_authority_override_used events (expected: 0 — must be absent) ---"
ABSENCE_COUNT=$(grep -c 'lexicon_authority_override_used' "$B_LOG" 2>/dev/null || echo 0)
echo "match count: $ABSENCE_COUNT  (expected: 0)"

echo
echo "--- lexicon_fetch_failed events with authority_ambiguous classification ---"
grep -E 'lexicon_fetch_failed' "$B_LOG" | tail -5 \
    || echo "(no lexicon_fetch_failed events)"
echo "expected: at least one with failure_class=authority_ambiguous per sub-case"

# ============================================================
# Block 6 — Side-effect-check: cache rows NOT created for rejected NSIDs
# ============================================================
echo
echo "[scenario-13] Block 6: cache rows NOT created for rejected NSIDs"
echo "============================================================"

case "$BACKEND" in
sqlite)
    echo "(BACKEND=sqlite — operator runs:"
    echo "   for nsid in $NSID_13A $NSID_13B $NSID_13C; do"
    echo "     echo \"\$nsid: \$(sqlite3 \"${B_DATA}/account.sqlite\" \"SELECT count(*) FROM lexicon_cache WHERE nsid = '\$nsid'\")\""
    echo "   done"
    echo " expected each count: 0)"
    ;;
postgres)
    echo "(BACKEND=postgres — operator runs:"
    echo "   for nsid in $NSID_13A $NSID_13B $NSID_13C; do"
    echo "     echo \"\$nsid: \$(docker exec aurora-phase-b-pg-b psql -U aurora -d aurora -At -c \"SELECT count(*) FROM lexicon_cache WHERE nsid = '\$nsid'\")\""
    echo "   done"
    echo " expected each count: 0)"
    ;;
esac

# ============================================================
# Block 7 — Teardown
# ============================================================
echo
echo "[scenario-13] Block 7: teardown"
echo "============================================================"
MOCK_DNS_BIND="$DNS_NAMESERVER" pb_mock_dns_stop

echo
echo "[scenario-13] decision-point:"
echo "  per sub-case, operator confirms:"
echo "    - HTTP status 502 (or rejected-class 4xx/5xx for 13c)"
echo "    - body shape: { error: 'LexiconAuthorityAmbiguous', ... } (13a/b)"
echo "                  (13c's exact error variant TBD by parse_did_from_txt's"
echo "                   taxonomy — operator records what surfaces)"
echo "    - log: lexicon_dns_lookup PRESENT (DNS arm fired)"
echo "    - log: lexicon_authority_override_used ABSENT (override didn't fire)"
echo "    - cache: zero rows for each of $NSID_13A, $NSID_13B, $NSID_13C"
echo "  both halves of the log check are load-bearing — without both,"
echo "  the scenario can't prove the live resolver path fired."
echo
echo "  run on BOTH backends:"
echo "    BACKEND=sqlite   ./phase-b/arc17/scenario-13.sh"
echo "    BACKEND=postgres ./phase-b/arc17/scenario-13.sh"
echo "  no single-backend carve-out — the rule is uniform 'always run"
echo "  twice' (V06_DESIGN.md Settled Decision 3)."
