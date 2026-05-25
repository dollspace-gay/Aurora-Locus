# Arc 12 Phase B exercise script

Localhost smoke-test script for the Phase B sweep of Arc 12
(`chainlink #68` — entryway architecture). Mirrors the Arc 7-10
convention at
[`arc10-phase-b-commands.md`](arc10-phase-b-commands.md): curl
against `localhost`, `cargo` invocations for deterministic
test-infra checks, no deployment framing.

Drafted per the §4.10 operator-driven convention: CC drafts the
curls below, skydeval executes against the live setup, CC
interprets the captured output, skydeval signs off.

> **Setup dependency**: the §5.8.2 scenarios require the mock PLC
> directory + two PDS instances + (for 5b/6) a test entryway stub
> per [`../operator/phase-b-setup.md`](../operator/phase-b-setup.md).
> Provision those before running the sections below.

## Prerequisites

- Working dir: `/mnt/d/- - CODING/RUST/aurora-locus`.
- Branch `skydeval/v0.5-cycle` at the Arc 12 Step 5 tip or
  descendants.
- Free ports 2582 (mock PLC), 2583 (PDS A), 2584 (PDS B), 2585
  (entryway stub, 5b/6 only).
- `curl`, `jq`, `openssl`, `sqlite3` on the dev machine.
- Aurora-Locus binary built with `cargo build` (debug mode required
  for `dev.aurora.*` endpoints; release builds 404).

## Setup checklist (one-time per session)

1. Start the mock PLC at port 2582 (see
   [`../operator/phase-b-setup.md#mock-plc-directory`](../operator/phase-b-setup.md#mock-plc-directory)).
2. Start PDS A with the env block from
   [`../operator/phase-b-setup.md#pds-a-and-pds-b-startup`](../operator/phase-b-setup.md#pds-a-and-pds-b-startup).
3. Start PDS B with the mirror env block (port 2584, peer = A).
4. Health probes:

   ```bash
   curl -s http://127.0.0.1:2583/health | jq
   curl -s http://127.0.0.1:2584/health | jq
   curl -s http://127.0.0.1:2582/      # mock PLC; shape depends on script
   ```

5. Skip the entryway stub start for Sections 1-5a; start it before
   Section 5b.

---

## Scenario 1 — Two-instance startup with substrate fixes

§5.8.2 Scenario 1 — verifies Step 0.5 substrate gaps (Gap 1 self-URL
+ Gap 2 peer_pds parser + Gap 3 PdsDiscovery bootstrap) land in
both instances.

### 1.1 service_url() correct on each

```bash
# PDS A's PLC-genesis service URL is propagated via
# /.well-known/did.json's serviceEndpoint.
curl -s http://127.0.0.1:2583/.well-known/did.json | jq '.service[0]'
curl -s http://127.0.0.1:2584/.well-known/did.json | jq '.service[0]'
```

Expected: A's serviceEndpoint = `http://127.0.0.1:2583`;
B's = `http://127.0.0.1:2584`. (Both should be `http` — the
localhost-aware scheme detection from Step 0.5 Gap 1.)

### 1.2 listKnownPeers shows the cross-registration

```bash
# A sees B
curl -s http://127.0.0.1:2583/xrpc/dev.aurora.federation.listKnownPeers \
  | jq

# B sees A
curl -s http://127.0.0.1:2584/xrpc/dev.aurora.federation.listKnownPeers \
  | jq
```

Expected: each instance lists one peer entry with `source: "config"`
(or, if PdsDiscovery has fired runtime discovery, `source:
"discovery"` — both are correct).

---

## Scenario 2 — Account creation produces correct PLC service URL

```bash
# Create an account on PDS A.
CREATE_A=$(curl -s -X POST http://127.0.0.1:2583/xrpc/dev.aurora.createAccount \
  -H 'Content-Type: application/json' \
  -d '{"handle":"userA","email":"a@localhost","password":"TestPassword123!"}')
export USER_A_DID=$(echo "$CREATE_A" | jq -r '.did')
echo "User A DID: $USER_A_DID"

# Read the genesis op back from the mock PLC.
curl -s "http://127.0.0.1:2582/${USER_A_DID}" | jq '.service'
```

Expected: `service[].serviceEndpoint == "http://127.0.0.1:2583"`
(the §5.3.2 Gap 1 closure landed in the PLC genesis op CBOR, not
just the runtime well-known doc).

---

## Scenario 3 — inspectAccount surfaces full identity state

§5.8.1 affordance.

```bash
curl -s "http://127.0.0.1:2583/xrpc/dev.aurora.federation.inspectAccount?did=${USER_A_DID}" \
  | jq
```

Expected:

```json
{
  "did": "did:plc:...",
  "actorPresent": true,
  "handle": "userA.localhost",
  "hasRotationKey": true,
  "hasAtprotoSigningKey": true,
  "isServiceDid": false,
  "isPeerPds": false,
  "isEntrywayDid": false
}
```

`hasAtprotoSigningKey: true` validates the Step 1.5 substrate-gap
closure landed: the column populates for new accounts.

---

## Scenario 4 — Tuple-routing matrix (§5.6.2) is in code

```bash
cargo test --test arc12_routing_matrix --locked 2>&1 | tail -5
```

Expected: `12 passed; 0 failed; 8 ignored`. The 8 ignored entries
each cite Step-1.3 / EntrywayConfig wiring in their ignore reason
— Phase B does NOT need to drive them, they're covered by Step 1
+ Step 3's static-text registration tests.

---

## Scenario 5a — Forwarded handlers in standalone mode (no entryway)

§5.8.2 Scenario 5a — confirms standalone behavior is unchanged.

### 5a.1 getSession returns local account info

```bash
LOGIN=$(curl -s -X POST http://127.0.0.1:2583/xrpc/com.atproto.server.createSession \
  -H 'Content-Type: application/json' \
  -d '{"identifier":"userA","password":"TestPassword123!"}')
export USER_A_TOKEN=$(echo "$LOGIN" | jq -r '.accessJwt')

curl -s http://127.0.0.1:2583/xrpc/com.atproto.server.getSession \
  -H "Authorization: Bearer $USER_A_TOKEN" \
  | jq
```

Expected: `did`, `handle`, `email` for userA. NOT forwarded
(no entryway configured).

### 5a.2 requestPasswordReset hits local mailer path

```bash
curl -s -X POST http://127.0.0.1:2583/xrpc/com.atproto.server.requestPasswordReset \
  -H 'Content-Type: application/json' \
  -d '{"identifier":"userA"}' \
  | jq
```

Expected: `{}` plus a tracing log line about token generation
(grep PDS A's stdout for `password reset` / `generate_password_reset_token`).

### 5a.3 Static-text registration test confirms forwarded routes are wired

```bash
cargo test --test arc12_entryway_registration --locked
```

Expected: `5 passed; 0 failed`.

### 5a.4 Stop PDS B before Section 5b

```bash
# PDS B is irrelevant to 5b/6 per §5.8.2 mode discipline.
pkill -f "target/.*aurora-locus.*PDS_PORT=2584"
```

---

## Scenario 5b — Forwarded handlers with entryway stub

§5.8.2 Scenario 5b — requires the entryway stub at port 2585 +
PDS A restarted with the `PDS_ENTRYWAY_*` env block per
[`../operator/phase-b-setup.md#entryway-stub-scenarios-5b-6-only`](../operator/phase-b-setup.md#entryway-stub-scenarios-5b-6-only).

### 5b.1 Mint a service-auth JWT for the user

```bash
curl -s -X POST http://127.0.0.1:2583/xrpc/dev.aurora.federation.mintServiceToken \
  -H 'Content-Type: application/json' \
  -d "{
    \"userDid\": \"${USER_A_DID}\",
    \"aud\": \"did:web:entryway.local\",
    \"lxm\": \"com.atproto.server.getSession\"
  }" \
  | jq
```

Expected: `{"accessJwt": "<jwt>"}`. Decode the JWT payload to
confirm shape:

```bash
TOKEN=$(curl -s -X POST http://127.0.0.1:2583/xrpc/dev.aurora.federation.mintServiceToken \
  -H 'Content-Type: application/json' \
  -d "{\"userDid\":\"${USER_A_DID}\",\"aud\":\"did:web:entryway.local\",\"lxm\":\"x.y\"}" \
  | jq -r '.accessJwt')
echo "$TOKEN" | awk -F. '{print $2}' | base64 -d 2>/dev/null | jq
```

Expected payload fields: `iss=${USER_A_DID}`,
`aud=did:web:entryway.local`, `lxm="x.y"`, `iat` ≈ now,
`exp = iat + 60`, `jti` (UUID).

### 5b.2 simulateForward against the entryway stub

```bash
curl -s -X POST http://127.0.0.1:2583/xrpc/dev.aurora.federation.simulateForward \
  -H 'Content-Type: application/json' \
  -d "{
    \"nsid\": \"com.atproto.server.getSession\",
    \"userDid\": \"${USER_A_DID}\",
    \"stubUrl\": \"http://127.0.0.1:2585\",
    \"body\": {}
  }" \
  | jq
```

Expected: `outboundUrl` = `http://127.0.0.1:2585/xrpc/com.atproto.server.getSession`,
`headers` contains `authorization: Bearer eyJ...` (the minted
service-auth JWT), `stubStatus` = 200 (the stub's echo response).

### 5b.3 Real forwarded call via getSession handler

```bash
curl -s http://127.0.0.1:2583/xrpc/com.atproto.server.getSession \
  -H "Authorization: Bearer $USER_A_TOKEN" \
  | jq
```

Expected: response shape from the entryway stub (not PDS A's
local account_manager). The stub log should show one inbound call
with an `Authorization: Bearer …` header carrying a fresh
service-auth JWT.

---

## Scenario 6 — Per-endpoint audience policy

§5.6.7 / §5.3.4 — forwarded routes accept both PDS-DID and
entryway-DID aud; non-forwarded reject entryway-DID aud.

### 6.1 Forwarded route accepts entryway-DID aud

```bash
# Mint a service-auth JWT with aud = entryway DID.
TOKEN_ENTRYWAY_AUD=$(curl -s -X POST http://127.0.0.1:2583/xrpc/dev.aurora.federation.mintServiceToken \
  -H 'Content-Type: application/json' \
  -d "{\"userDid\":\"${USER_A_DID}\",\"aud\":\"did:web:entryway.local\",\"lxm\":\"com.atproto.server.getSession\"}" \
  | jq -r '.accessJwt')

curl -s http://127.0.0.1:2583/xrpc/com.atproto.server.getSession \
  -H "Authorization: Bearer $TOKEN_ENTRYWAY_AUD" \
  -w "\nstatus=%{http_code}\n"
```

Expected: 200 (forwarded route's allowlist accepts entryway DID).

### 6.2 Non-forwarded route rejects entryway-DID aud

```bash
curl -s http://127.0.0.1:2583/xrpc/com.atproto.server.refreshSession \
  -H "Authorization: Bearer $TOKEN_ENTRYWAY_AUD" \
  -w "\nstatus=%{http_code}\n"
```

Expected: 401 (non-forwarded route's allowlist is PDS-DID only;
entryway-DID aud rejects at audience check).

---

## Section X — Regression baselines

```bash
cargo test --locked --lib 2>&1 | tail -3
cargo test --test arc12_routing_matrix --locked 2>&1 | tail -3
cargo test --test arc12_entryway_registration --locked 2>&1 | tail -3
```

Expected (post-Step-5 tip):
- Lib: `1007 passed; 0 failed; 0 ignored`.
- Routing matrix: `12 passed; 0 failed; 8 ignored`.
- Registration: `5 passed; 0 failed`.

---

## Teardown

```bash
pkill -f "target/.*aurora-locus"
# kill mock PLC + entryway stub via whatever process model used to start them
rm -rf phase-b/pds-a phase-b/pds-b
```
