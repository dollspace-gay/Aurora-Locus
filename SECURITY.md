# Security

Aurora-Locus is an ATProto Personal Data Server in Rust. Security-significant
features include OAuth 2.1 with mandatory PKCE, DPoP sender-bound tokens per
RFC 9449, ES256K cross-PDS service-auth, Argon2id password hashing, multi-axis
rate limiting (distributed across instances under the Postgres backend), and a
hash-chained audit log of every administrative decision. Operator-facing
detail lives in [`docs/operator/`](docs/operator/); this document covers the
security posture summary, the vulnerability-reporting process, and the
advisories the project has accepted as reachable-unaffected.

## Reporting a vulnerability

Open a [private security advisory on GitHub](https://github.com/skydeval/Aurora-Locus/security/advisories/new),
or contact the maintainer directly. Please do not disclose publicly before a
fix is available; we aim to acknowledge reports within a few days and provide
a fix or remediation timeline.

## Authentication architecture

- **Cross-PDS service auth** ([`src/service_auth.rs`](src/service_auth.rs),
  [`src/federation/service_auth.rs`](src/federation/service_auth.rs)) — ES256K
  JWTs, signed with the per-account signing key (the `#atproto` verification
  method published in each account's DID document). Token expiration is
  capped at 1 hour with a lexicon method (`lxm`) and 1 minute without.
  Verification resolves the issuer's DID document via the identity layer and
  validates the signature against the `#atproto` verification method.
  Strict-expiration (`leeway = 0`); the `iat` claim tolerates up to 2 minutes
  of clock skew.
- **DPoP** ([`src/federation/dpop.rs`](src/federation/dpop.rs)) — RFC 9449
  proof-of-possession on resource requests. JWK key parsing implemented;
  invalid proofs return HTTP 400 (no silent Bearer downgrade). Strict
  expiration (`leeway = 0`).
- **Nonce replay prevention** ([`src/federation/nonce_store.rs`](src/federation/nonce_store.rs))
  — in-memory `jti` tracking, 120-second expiry (2× the maximum service-auth
  JWT lifetime).
- **Local sessions** ([`src/api/middleware.rs`](src/api/middleware.rs),
  [`src/account/manager.rs`](src/account/manager.rs)) — session JWTs are
  resolved first; service-auth + DPoP fall through after.
- **Identity resolution** ([`src/identity/resolver.rs`](src/identity/resolver.rs))
  — handle and DID resolution via the `proto-blue-identity` SDK, with
  two-tier (in-memory + on-disk) caching and stale-fallback semantics.

## Cryptography

- **ES256K (secp256k1)** — ATProto-canonical signing for repo commits, PLC
  rotation, and outbound service-auth JWTs.
- **ES256 (P-256)** — DPoP per RFC 9449.
- **Argon2id** — password hashing.
- **Inbound verification** — DID-document `#atproto` verification method
  resolution via the identity layer; the per-account signing key is the
  canonical signer for service-auth, not a server-wide key.
- **Clock-skew tolerance** — strict-expiration (`leeway = 0`) on
  service-auth and DPoP; `iat` (issued-at) on service-auth allows up to 2
  minutes of skew. No other skew tolerance is wired.

## Rate limiting

Enforced in middleware on the request path ([`src/rate_limit.rs`](src/rate_limit.rs)).
On by default. Defaults (requests per second, burst 50):

| Class | RPS |
|---|---:|
| Authenticated | 100 |
| Unauthenticated | 10 |
| Admin | 1000 |
| Cross-PDS (service-auth) | 10 |

Cross-PDS is 10× stricter than local authenticated to bound abuse from
peer-PDS callers. Outbound handle and DID resolution are separately
budgeted at 50 RPS each. Admin UI static assets bypass the limiter by
default (page-load fan-out exceeds the per-IP unauthenticated quota); a
reverse proxy is expected to provide asset DDoS protection in production.

Under multi-instance Postgres deployments with `PDS_DISTRIBUTED_STATE_MODE=distributed`,
cross-instance coherence is provided by the `DistributedRateLimiter`
([`src/distributed/postgres_cas.rs`](src/distributed/postgres_cas.rs)) so a
client can't 4×-multiply its budget by rotating across instances. Per-instance
in-memory governor remains as defense-in-depth on substrate-consult failure.

## Audit logging

Every administrative decision writes a hash-chained `audit_chain_entry` row
([`src/admin/audit_chain.rs`](src/admin/audit_chain.rs)) atomically with the
underlying mutation; per-row and chain-level verification fields are exposed
via `tools.aurora.admin.getAuditTrail`. Live-streaming consumers subscribe to
chain entries via `tools.aurora.admin.subscribeModEvents` with
`includeAuditChain: true`. Forensic export
(`tools.aurora.admin.exportAccountForensic`) produces a tar bundle whose
manifest's `bundle_hash` covers the complete tar bytes; the chain row's
rationale records the same hash. External verification reproducible per
[`docs/operator/audit-chain-verification.md`](docs/operator/audit-chain-verification.md).

## Accepted advisories

Reachable-unaffected dependency vulnerabilities. Each is documented with the
reachability argument and the trigger that would reopen the decision.

### hickory-proto NSEC3 closest-encloser unbounded loop (RUSTSEC-2026-0118, High)

The vulnerable function `verify_nsec3` lives in
`hickory-proto::dnssec::dnssec_dns_handle::nsec3_validation`. It is reachable
only when the `__dnssec` cargo feature is enabled AND `ResolverOpts.validate
= true`.

Aurora-Locus declares `hickory-resolver = "0.26.1"` with default features
only; no `dnssec-ring` or `dnssec-aws-lc-rs` features.
`ResolverOpts::Default` sets `validate: false`; `parse_resolv_conf` never
overrides it. The constructed resolver handle is `LookupEither::Retry`,
never the `Secure(DnssecDnsHandle)` arm that calls the vulnerable path.

The earlier duplicate `hickory-proto 0.25.2` copy via `proto-blue-identity`
was eliminated by the `proto-blue 0.3.3` bump; the tree now contains only
`hickory 0.26.1`. Verification surface: Phase B Scenario 13 (live DNS-TXT
authority resolution) confirms the resolver behavior is unchanged after the
bump.

Reassessment triggers: any `dnssec-*` cargo feature added, any code path
setting `validate = true` on the resolver, or an upstream `hickory-proto`
fix on the validation path.

### rustls-webpki via aws-smithy (RUSTSEC-2026-0098 / 0099 / 0104; Low / Low / High)

Three advisories on `rustls-webpki 0.101.7`: a panic in CRL parsing on a
malformed BIT STRING in the IDP extension (High); URI name constraints
incorrectly accepted (Low); wildcard-asserting cert name constraints
incorrectly accepted (Low).

Aurora-Locus's direct TLS path uses `rustls-webpki 0.103.13` via `rustls
0.23.34` — patched for all three. The vulnerable `0.101.7` copy is pulled
in only by `aws-smithy-http-client 1.1.10` (the AWS SDK's internal HTTP
client) via `rustls 0.21.12`. The vulnerable code runs only during TLS
handshakes against the endpoints the AWS SDK calls — AWS's own service
endpoints, presenting valid AWS certificates. Aurora-Locus does not feed
attacker-controlled CRLs or certificates into the AWS SDK's TLS path.

Resolution clears automatically when `aws-smithy-http-client` ships a
version on `rustls 0.23.x`. The latest release as of this writing (1.1.10)
still pins `rustls 0.21`; this is upstream-blocked. Forcing an incompatible
bump would either fail or fork the AWS SDK's HTTP client.

Reassessment triggers: `aws-smithy-http-client` ships on `rustls 0.23.x`;
or an operator-side configuration begins feeding attacker-controlled CRLs
or certificates to the AWS SDK.

## Known limitations

- **In-memory nonce store** ([`src/federation/nonce_store.rs`](src/federation/nonce_store.rs))
  is per-instance by default. DPoP JTI replay tracking is cross-instance
  coherent under `PDS_DISTRIBUTED_STATE_MODE=distributed` via the
  Postgres-CAS substrate; the federation service-auth nonce store
  remains in-process.
- **Federation is off by default** (`PDS_FEDERATION_ENABLED=false`).
  Operators opting in publish events to relays and may expose federation
  endpoints; review [`docs/operator/configuration.md`](docs/operator/configuration.md)
  §17 before enabling in production.
