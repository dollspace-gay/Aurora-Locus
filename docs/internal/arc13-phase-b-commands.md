# Arc 13 Phase B exercise script

Localhost smoke-test script for the Phase B sweep of Arc 13
(`chainlink #70` — PLC operations correctness + completeness).
Mirrors the per-arc convention at
[`arc12-phase-b-commands.md`](arc12-phase-b-commands.md): curl
against `localhost`, `cargo` invocations for deterministic
test-infra checks, no deployment framing.

Drafted per §4.10 operator-driven convention: CC drafts the curls
below, skydeval executes against the live setup, CC interprets
the captured output, skydeval signs off.

> **Setup dependency**: the §6.8.2 scenarios require the mock PLC
> directory (mode-(b) strict, per [`../operator/phase-b-setup.md`](../operator/phase-b-setup.md)
> §"Signature-verification mode (Arc 13 §6.4 Step 4.5)" + §"Tombstone-
> op contract"), plus MailHog (or equivalent local SMTP-to-HTTP
> proxy) on ports 1025/8025 for Scenario 5's email-token flow.

## Prerequisites

- Working dir: `/mnt/d/- - CODING/RUST/aurora-locus`.
- Branch `skydeval/v0.5-cycle` at the Arc 13 Step 7 tip or
  descendants.
- Free ports 2582 (mock PLC, mode-(b)), 2583 (PDS A), 1025+8025
  (MailHog).
- `curl`, `jq`, `openssl`, `sqlite3` on the dev machine.
- Aurora-Locus binary built with `cargo build` (debug mode required
  for `dev.aurora.*` endpoints; release builds 404).

## Setup checklist (one-time per session)

1. Start the mock PLC at port 2582 in mode-(b) per
   [`../operator/phase-b-setup.md`](../operator/phase-b-setup.md).
   Confirm the mock accepts plc_tombstone ops + terminal-state
   semantics per the Arc 13 contract additions.
2. Start MailHog (`docker run -d -p 1025:1025 -p 8025:8025 mailhog/mailhog`
   or equivalent).
3. Start PDS A with `PDS_DID_PLC_URL=http://127.0.0.1:2582` +
   `PDS_SMTP_URL=smtp://127.0.0.1:1025` (or equivalent mailer
   config). Optionally set
   `PDS_IDENTITY_RECOVERY_DID_KEY=did:key:zRecoveryStub` to
   exercise §6.3.3 recovery-key support.
4. Health probes:

   ```bash
   curl -s http://127.0.0.1:2583/health | jq
   curl -s http://127.0.0.1:2582/ | jq    # mock PLC; shape depends on script
   open http://127.0.0.1:8025/             # MailHog web UI
   ```

---

## Scenario 1 — Account creation against strict-mode mock

§6.8.2 Scenario 1 — verifies Step 0.5 wire-shape foundation +
Step 0.6 DID-suffix derivation + Step 0.7 key separation +
Step 2 recovery-key support all land against a strict-mode
directory.

### 1.1 Create an account

```bash
CREATE=$(curl -s -X POST http://127.0.0.1:2583/xrpc/dev.aurora.createAccount \
  -H 'Content-Type: application/json' \
  -d '{"handle":"test1","email":"t1@localhost","password":"TestPassword123!"}')
export USER_DID=$(echo "$CREATE" | jq -r '.did')
echo "Created: $USER_DID"
```

### 1.2 Inspect the genesis op

```bash
curl -s "http://127.0.0.1:2583/xrpc/dev.aurora.federation.inspectAccount?did=${USER_DID}" | jq
curl -s "http://127.0.0.1:2582/${USER_DID}" | jq
curl -s "http://127.0.0.1:2582/${USER_DID}/log/audit" | jq '.[0].operation'
```

Expected genesis op shape:
- NO `did` field (chainlink #61 §1.4 fix).
- `type: "plc_operation"`.
- `rotationKeys` includes the PDS-wide rotation key did:key (with
  recovery key prepended if `PDS_IDENTITY_RECOVERY_DID_KEY` is
  set per §6.3.3 priority order).
- `verificationMethods.atproto` = per-actor signing key did:key
  (distinct from rotation key per §6.3.2).
- `services.atproto_pds.endpoint` = `http://127.0.0.1:2583`.
- `sig` is base64url (not hex) per chainlink #61 §1.1 fix.

Expected mock acceptance: HTTP 200 on the submit; audit log
contains the entry.

### 1.3 Negative-path test (closes recon §9.1)

```bash
# Synthesize a pre-Arc-13-style genesis op via the test utility.
# This proves the mock-mode-(b) actually checks signatures.
cargo test --test arc13_pre_arc13_synthetic --locked
# Then (operator-script): submit a synthetic op directly to the
# mock and confirm HTTP 400 InvalidSignature.
```

Expected: cargo test returns 2 passed. Operator-script submission
of the synthetic op MUST receive HTTP 400 (mode-(b) rejection).

---

## Scenario 2 — Handle update against mock PLC

§6.8.2 Scenario 2 — verifies Step 1.2 snapshot-mutator (no
diff-build).

```bash
LOGIN=$(curl -s -X POST http://127.0.0.1:2583/xrpc/com.atproto.server.createSession \
  -H 'Content-Type: application/json' \
  -d '{"identifier":"test1","password":"TestPassword123!"}')
export TOKEN=$(echo "$LOGIN" | jq -r '.accessJwt')

curl -s -X POST http://127.0.0.1:2583/xrpc/com.atproto.identity.updateHandle \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"handle":"newhandle"}'

# Inspect updated audit log; the new entry MUST include the FULL
# snapshot of fields, not just the changed alsoKnownAs.
curl -s "http://127.0.0.1:2582/${USER_DID}/log/audit" | jq '.[-1].operation'
```

Expected: HTTP 200; audit log's last entry has `rotationKeys`,
`verificationMethods`, `services`, `alsoKnownAs` ALL inherited
from prior op via mutator + `alsoKnownAs` containing the new
handle. `prev` matches the prior op's CID. Mock accepts (strict-
mode sig verify against prior op's rotation keys succeeds).

---

## Scenario 3 — Recovery-key priority order

§6.8.2 Scenario 3 — verifies §6.3.3 priority ordering.

```bash
# Re-create an account with recoveryKey input set.
CREATE3=$(curl -s -X POST http://127.0.0.1:2583/xrpc/dev.aurora.createAccount \
  -H 'Content-Type: application/json' \
  -d '{
    "handle":"test3",
    "email":"t3@localhost",
    "password":"TestPassword123!",
    "recoveryKey":"did:key:zMyAccountRecovery"
  }')
export USER3=$(echo "$CREATE3" | jq -r '.did')

# Note: dev.aurora.createAccount may not surface the recoveryKey
# input field if it routes to a different code path; use the
# canonical com.atproto.server.createAccount XRPC endpoint
# instead, which does.

curl -s "http://127.0.0.1:2582/${USER3}/log/audit" | jq '.[0].operation.rotationKeys'
```

Expected: `rotationKeys` order is `[did:key:zMyAccountRecovery,
config.recovery_did_key (if set), pds_rotation_key.did_key]`.
Per-account recovery first; PDS recovery second; PDS rotation
last.

---

## Scenario 4 — Tombstone via dev endpoint

§6.8.2 Scenario 4 — verifies §6.3.5 tombstone primitive + mock
terminal-state semantics.

```bash
RESP=$(curl -s -X POST http://127.0.0.1:2583/xrpc/dev.aurora.tombstoneDid \
  -H 'Content-Type: application/json' \
  -d "{\"did\":\"${USER_DID}\"}")
echo "$RESP" | jq

# Inspect audit log: last entry should be the tombstone.
curl -s "http://127.0.0.1:2582/${USER_DID}/log/audit" | jq '.[-1].operation.type'

# Attempt a regular update referencing the tombstone CID as prev;
# mock should reject (terminal-state per §6.4 Step 4.5).
# (This step requires manual construction or the existing
# updateHandle handler — it will fail because get_last_op returns
# DidTombstoned.)
curl -s -X POST http://127.0.0.1:2583/xrpc/com.atproto.identity.updateHandle \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"handle":"shouldfail"}' \
  -w "\nstatus=%{http_code}\n"
```

Expected:
- Tombstone response includes `tombstoneCid` + `prevCid`.
- Audit log's last entry has `type: "plc_tombstone"`.
- updateHandle returns HTTP 400 `DidTombstoned` (Aurora-Locus
  side: PdsError::DidTombstoned).

---

## Scenario 5 — Email-token confirmation flow

§6.8.2 Scenario 5 — verifies Step 3's email-token two-phase.

```bash
# 5.1 Request the signing token.
curl -s -X POST http://127.0.0.1:2583/xrpc/com.atproto.identity.requestPlcOperationSignature \
  -H "Authorization: Bearer $TOKEN" \
  -w "\nstatus=%{http_code}\n"

# 5.2 Inspect MailHog for the email + extract the token.
curl -s http://127.0.0.1:8025/api/v2/messages | jq '.items[0].Content.Body'
# Operator parses the token out of the body.
export PLC_TOKEN="<token from email>"

# 5.3 Sign with the token + a rotation-keys override.
curl -s -X POST http://127.0.0.1:2583/xrpc/com.atproto.identity.signPlcOperation \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d "{
    \"token\": \"${PLC_TOKEN}\",
    \"rotationKeys\": [\"did:key:zNewRotation\", \"did:key:zPDSRotation\"]
  }" | jq

# 5.4 Two-phase test: re-call sign with the SAME token.
curl -s -X POST http://127.0.0.1:2583/xrpc/com.atproto.identity.signPlcOperation \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d "{\"token\":\"${PLC_TOKEN}\",\"rotationKeys\":[\"did:key:zAnother\"]}" \
  -w "\nstatus=%{http_code}\n"
```

Expected:
- 5.1: HTTP 200 + `{}`.
- 5.2: MailHog shows an email with the token in the body.
- 5.3: HTTP 200 + `{operation: <signed op JSON>}`. The op has
  `rotationKeys` overridden + other fields inherited from prior
  op + a fresh sig.
- 5.4: HTTP 409 `TokenAlreadyConsumed` (Path B non-declared
  error per §6.3.6 round-4 F4 closure).

### 5.5 Transient-failure test (manual)

Trigger a fresh email token (rerun 5.1 + 5.2). Block PLC
directory traffic temporarily (e.g., `iptables -A OUTPUT -p tcp
--dport 2582 -j DROP`). Call signPlcOperation: get_last_op fails
(transient) → token NOT consumed. Unblock + retry with the same
token: succeeds.

Expected: token preservation across transient failures (two-phase
property per §6.3.6).

---

## Scenario 6 — Hard-fail on PLC directory unavailable

§6.8.2 Scenario 6 — verifies §6.3.7 hard-fail + Step 5 removal of
silent did:web fallback.

```bash
# Stop the mock PLC.
# (Operator command, e.g.: kill -9 $MOCK_PID)

# Attempt to create an account.
curl -s -X POST http://127.0.0.1:2583/xrpc/dev.aurora.createAccount \
  -H 'Content-Type: application/json' \
  -d '{"handle":"test6","email":"t6@localhost","password":"TestPassword123!"}' \
  -w "\nstatus=%{http_code}\n"

# Verify no actor row persisted.
sqlite3 /path/to/account.sqlite "SELECT did, handle FROM actor WHERE handle = 'test6.localhost';"
```

Expected:
- createAccount returns HTTP 5xx (error propagated from
  register_plc_did failure).
- sqlite3 query returns no rows (no partial actor state).
- PDS A's stderr log line at `ERROR` level:
  `PLC directory registration failed; hard-failing account
  creation per §6.3.7`.
- NO `did:web:` DID generated (silent fallback removed per Step 5).

---

## Section X — Regression baselines

```bash
cargo test --locked --lib 2>&1 | tail -3
cargo test --test arc12_routing_matrix --locked 2>&1 | tail -3
cargo test --test arc12_entryway_registration --locked 2>&1 | tail -3
cargo test --test arc13_pre_arc13_synthetic --locked 2>&1 | tail -3
```

Expected (post-Step-7 tip):
- Lib: `1019 passed; 0 failed; 0 ignored`.
- Arc 12 routing matrix: `12 passed; 0 failed; 8 ignored`.
- Arc 12 registration: `5 passed; 0 failed`.
- Arc 13 pre-Arc-13 synthetic driver: `2 passed; 0 failed`.

### Arc 12 Scenario 5b re-interpretation (Step 6 doc)

Arc 12 v4.1 Phase B Scenario 5b now exercises "rotation discipline
of the PDS-wide rotation key" (not the previously-conflated
per-account key per chainlink #60 §2). After Arc 13's Step 0.7
key separation, the rotation_key column is gone from
`plc_keys`; the PDS-wide key in `config.authentication.plc_rotation_key`
is what signs every op. Scenario 5b's mint flow exercises this
key's mint discipline — re-mint per forward, TTL ≤60s — against
the unchanged config-resident key material rather than the
removed per-account material.

No code change needed in Arc 12's Phase B script; the semantic
shift is documented here for Step 6 / §6.6.7 audit traceability.

---

## Teardown

```bash
pkill -f "target/.*aurora-locus"
docker kill mailhog
# kill mock PLC via whatever process model used to start it
rm -rf phase-b/pds-a
```
