# Phase B environment setup (Arc 12)

Localhost setup procedure for the two-instance Phase B scenarios
described in V05_DESIGN.md §5.8. Three components: a mock PLC
directory (port 2582), PDS A (port 2583), and PDS B (port 2584).
Scenarios 5b/6 additionally require a test entryway stub (port
2585).

> **Audience**: Aurora-Locus operators running Phase B sweeps
> against locked code. Not a deployment guide — Phase B is a
> single-host smoke check, not a production topology. See
> [`multi-instance-deployment.md`](multi-instance-deployment.md)
> for production.

## Topology

```
        ┌──────────────┐    ┌──────────────┐
        │ PDS A        │    │ PDS B        │
        │ :2583        │    │ :2584        │
        │ peer_pds=B   │    │ peer_pds=A   │
        └──────┬───────┘    └──────┬───────┘
               │  PLC ops + audit  │
               ▼                   ▼
        ┌──────────────────────────────┐
        │ Mock PLC directory :2582     │
        │ (§5.8.2 mock contract)       │
        └──────────────────────────────┘

  Scenarios 5b/6 only:
        ┌──────────────┐
        │ Entryway stub │
        │ :2585         │
        └──────────────┘
```

## Mock PLC directory

A minimal HTTP server implementing §5.8.2's mock contract:

- `GET /{did}` → current DID document; 404 if unknown.
- `GET /{did}/log/audit` → operation log, oldest first; 404 if unknown.
- `POST /{did}` → append signed PLC op with sig + prev-chain checks.

Error-semantics requirements per round-4 Finding 4 are enumerated in
V05_DESIGN.md §5.8.2. Recommended implementation: a small Python or
Node.js script (~150-250 LoC). The script lives in the operator's
Phase B working tree, not the Aurora-Locus repo — Phase B environment
state is per-operator.

Reference contract checklist (verify against the mock before running
Scenarios 3, 5b, or any other PLC-touching scenario):

| Scenario               | Required mock behavior                          |
|------------------------|-------------------------------------------------|
| genesis-op accept      | Sig verifies against any rotation key in op     |
| update-op accept       | Sig verifies against prior op's rotation keys   |
| `prev` chain           | `prev` must equal current head CID              |
| append-only            | Rejected ops do not mutate log                  |
| malformed JSON         | 400 `{"error":"InvalidRequest", ...}`           |
| update-before-genesis  | 400 `{"error":"DidNotFound"}`                   |
| missing required field | 400 `{"error":"InvalidRequest", ...}`           |
| duplicate-op submit    | 400 `{"error":"InvalidPrev"}`                   |

## PDS A and PDS B startup

Both instances run from the same Aurora-Locus build, each pointed at
a distinct data dir + port. Env-var blocks for each:

**PDS A** (`PDS_PORT=2583`):

```bash
export PDS_PORT=2583
export PDS_HOSTNAME=127.0.0.1
export PDS_SERVICE_PUBLIC_URL=http://127.0.0.1:2583
export PDS_DATA_DIRECTORY=./phase-b/pds-a
export PDS_SERVICE_DID=did:web:127.0.0.1
export PDS_JWT_SECRET=$(openssl rand -hex 32)
export PDS_REPO_SIGNING_KEY_K256_PRIVATE_KEY_HEX=$(openssl rand -hex 32)
export PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX=$(openssl rand -hex 32)
export PDS_DID_PLC_URL=http://127.0.0.1:2582
export PDS_FEDERATION_ENABLED=true
export PDS_FEDERATION_PEER_PDS="did:plc:pdsbxxxxxxxxxxxxxxxxxx@http://127.0.0.1:2584"
cargo run --release  # or `cargo run` for the dev-routes builds
```

**PDS B** (`PDS_PORT=2584`): same shape; flip the peer entry to
point at A; use a separate data dir.

## Entryway stub (Scenarios 5b, 6 only)

For Scenarios 5b and 6, PDS A's environment additionally carries the
`PDS_ENTRYWAY_*` env-var quadruplet pointing at a stub server at port
2585. The stub:

- Implements the four forwarded NSIDs as echo endpoints:
  - `com.atproto.identity.signPlcOperation`
  - `com.atproto.identity.updateHandle`
  - `com.atproto.server.getSession`
  - `com.atproto.server.requestPasswordReset`
- Records incoming `Authorization` headers + bodies so the test can
  assert what Aurora-Locus actually forwarded.
- Returns either canned shapes (for the mint endpoints) or 200 `{}`
  (for the passthru endpoint).
- Owns its own ES256K keypair; the public key (SEC1 compressed, 66
  hex chars) goes into `PDS_ENTRYWAY_JWT_PUBLIC_KEY_HEX`.

PDS A's env for 5b/6:

```bash
export PDS_ENTRYWAY_URL=http://127.0.0.1:2585
export PDS_ENTRYWAY_ADMIN_TOKEN=<stub admin token>
export PDS_ENTRYWAY_JWT_PUBLIC_KEY_HEX=<stub pubkey hex>
export PDS_ENTRYWAY_DID=did:web:entryway.local
```

Restart PDS A after setting these — entryway config is loaded once
at startup per §5.5.7 / §5.3.3.1 trust-set immutability.

## Mode discipline (from §5.8.2)

- **Scenarios 1, 2, 3, 4, 5a**: both A and B in standalone mode (no
  `PDS_ENTRYWAY_*`).
- **Scenarios 5b, 6**: A in entryway mode pointing at the stub; B is
  irrelevant (stop B between 5a and 5b).

## Phase B exercise script

Per-arc exercise commands live at
[`../internal/arc12-phase-b-commands.md`](../internal/arc12-phase-b-commands.md)
per the §4.10 operator-driven convention. CC drafts curls; skydeval
executes; CC interprets the captured output; skydeval signs off.

## Teardown

`pkill -f "target/.*aurora-locus"` and remove the two data dirs.
The mock PLC and entryway stub teardown follow whatever process
model the operator used to start them.
