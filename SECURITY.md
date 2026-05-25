# Security Documentation - Aurora Locus PDS

## Phase 4: Cross-PDS Authentication Security

This document outlines the security measures implemented in Aurora Locus PDS for cross-PDS authentication and federation.

### Table of Contents
1. [Authentication Architecture](#authentication-architecture)
2. [Security Measures](#security-measures)
3. [Rate Limiting](#rate-limiting)
4. [Audit Logging](#audit-logging)
5. [Known Limitations](#known-limitations)
6. [Security Best Practices](#security-best-practices)

---

## Authentication Architecture

### Service Auth (Cross-PDS)

Aurora Locus implements **DID-based cryptographic verification** following the ATProto specification:

- **JWT Format**: ES256-signed JWTs with <60 second expiration
- **Claims**: `iss` (user DID), `aud` (service DID), `exp`, `lxm` (endpoint), `jti` (nonce)
- **Verification**: No callback to origin PDS - purely cryptographic via DID resolution
- **Signing Keys**: Extracted from user's DID document (`#atproto` verification method)

**Implementation**: [`src/federation/service_auth.rs`](src/federation/service_auth.rs)

### DPoP (Demonstrating Proof of Possession)

Client-to-PDS authentication with token binding:

- **Proof Format**: DPoP proof JWT with JWK public key in header
- **Binding**: Access tokens bound to specific device public keys
- **Nonce System**: Single-use nonces prevent replay attacks
- **Endpoint**: `/xrpc/com.aurora.dpop.getNonce` for nonce generation

**Implementation**: [`src/federation/dpop.rs`](src/federation/dpop.rs)

**⚠️ Current Limitation**: JWK-to-EC-key conversion is a placeholder (`jwk_to_decoding_key` function). Production deployment requires proper EC public key parsing from JWK format. Consider adding the `jsonwebtoken-jwk` crate.

### Unified Authentication

Endpoints support both local and cross-PDS authentication:

- **Local Auth**: Session tokens from account manager
- **Cross-PDS Auth**: Service auth JWTs with nonce verification
- **Fallback**: Tries local first, then cross-PDS
- **Authorization**: Always verifies `repo` parameter matches authenticated DID

**Implementation**: [`src/api/middleware.rs`](src/api/middleware.rs#L120-L202)

---

## Security Measures

### 1. Replay Prevention

**Nonce Tracking** (Service Auth):
- 120-second retention window
- Automatic cleanup every 5 minutes
- In-memory storage (can be swapped for Redis in production)
- **Implementation**: [`src/federation/nonce_store.rs`](src/federation/nonce_store.rs)

**DPoP Nonce System**:
- 5-minute expiration
- Single-use consumption
- Separate nonce store from service auth
- **Implementation**: [`src/federation/dpop.rs`](src/federation/dpop.rs#L46-L119)

### 2. Token Expiration

| Token Type | Max Lifetime | Notes |
|------------|--------------|-------|
| Service Auth JWT | <60 seconds | ATProto requirement |
| DPoP Proof | <60 seconds | Typical implementation |
| DPoP Nonce | 5 minutes | Generation to use window |
| Service Auth Nonce | 120 seconds | Allows for clock skew |

### 3. Cryptographic Verification

- **Algorithm**: ES256 (NIST P-256 elliptic curve)
- **Key Source**: DID documents (decentralized, tamper-proof)
- **Signature Validation**: Strict - no grace periods
- **Clock Skew**: Up to 2 minutes allowed for `iat` validation

### 4. Authorization Checks

All repo endpoints verify:
1. Request is authenticated (local OR cross-PDS)
2. `repo` parameter matches authenticated DID
3. User has permission to modify the specified repo

**Cross-repo access is strictly forbidden** - even with valid authentication.

---

## Rate Limiting

### Limits by Authentication Type

| Type | Requests/Second | Burst | Ratio to Local |
|------|-----------------|-------|----------------|
| Local Authenticated | 100 | 50 | 1x (baseline) |
| Cross-PDS | 10 | 5 | **10x stricter** |
| Unauthenticated | 10 | 10 | 10x stricter |
| Admin | 1000 | 100 | 10x more permissive |

### Cross-PDS Rate Limiting Rationale

Cross-PDS requests use **10x stricter limits** (10 req/s vs 100 req/s) to:

1. **Prevent Abuse**: Limit impact of compromised federated instances
2. **Resource Protection**: Prevent federation from overwhelming local users
3. **DoS Mitigation**: Rate limit distributed attacks across multiple PDSes
4. **Fair Usage**: Ensure local users get priority over federated requests

**Implementation**: [`src/rate_limit.rs`](src/rate_limit.rs#L120-L131)

**Enforcement**: Applied in repo endpoints before processing cross-PDS requests
**Files**: [`src/api/repo.rs`](src/api/repo.rs#L213-L217), [put_record](src/api/repo.rs#L262-L265), [delete_record](src/api/repo.rs#L300-L303)

---

## Audit Logging

All authentication events are logged with structured tracing:

### Logged Events

| Event | Level | Fields |
|-------|-------|--------|
| Authentication Success | INFO | `did`, `auth_type` (local/cross_pds), `is_app_password` |
| Authentication Failure | WARN | `error`, token validity |
| Service Auth Replay Attack | WARN | `jti`, reason |
| Rate Limit Exceeded | - | Prometheus metrics |
| Cross-PDS Authorization Mismatch | ERROR | `req.repo`, `auth_did` |
| DPoP Nonce Invalid | WARN | `nonce`, expiration status |

### Metrics

Prometheus metrics track:
- `RELAY_EVENTS_TOTAL` - By event type
- `RELAY_EVENT_PROCESSING_DURATION_SECONDS` - Histogram
- `RELAY_CONNECTION_STATUS` - Gauge (0=down, 1=up)
- Custom error counters via `metrics::record_error()`

**Implementation**: [`src/metrics.rs`](src/metrics.rs)

### Sensitive Data Handling

**Never logged**:
- JWTs (only jti/nonce identifiers)
- Private keys
- Full DID documents (only DIDs themselves)

---

## Known Limitations

### 1. DPoP JWK Parsing - ✅ RESOLVED

**File**: [`src/federation/dpop.rs`](src/federation/dpop.rs#L265-L362)

**Status**: Fixed. Full JWK-to-DecodingKey conversion implemented.

**Implementation**: The `jwk_to_decoding_key()` function now properly:
- Validates JWK kty (EC) and crv (P-256) fields
- Decodes base64url x/y coordinates
- Validates coordinate lengths (32 bytes each for P-256)
- Constructs uncompressed SEC1 point format (0x04 || x || y)
- Parses with p256 crate and validates point is on curve
- Converts to SPKI DER then PEM for jsonwebtoken compatibility

**Tests**: Comprehensive test coverage including valid keys, missing fields, unsupported curves, invalid base64, wrong key types.

### 2. Nonce Store Scalability

**Current**: In-memory HashMap
**Limitation**: Lost on restart, not shared across instances
**Production Fix**: Use Redis for distributed nonce tracking

**Files**:
- Service Auth: [`src/federation/nonce_store.rs`](src/federation/nonce_store.rs)
- DPoP: [`src/federation/dpop.rs`](src/federation/dpop.rs#L46-L89)

### 3. Signing Key Extraction - ✅ RESOLVED

**File**: [`src/identity/resolver.rs`](src/identity/resolver.rs#L461-L631)

**Status**: Fixed as of 2025-12-27.

**Implementation**: Full multibase-to-PEM key conversion now implemented:
- Multibase decoding (base58btc with 'z' prefix)
- Multicodec varint parsing (supports P-256: 0x1200, secp256k1: 0xe7)
- EC point decompression (33-byte compressed to full public key)
- PEM/SPKI encoding for jsonwebtoken compatibility

**Supported Key Types**:
- P-256 (secp256r1) - ES256 algorithm
- secp256k1 - ES256K algorithm

---

## Security Best Practices

### For Operators

1. **Monitor Rate Limits**: Watch for spikes in cross-PDS requests
2. **Review Logs**: Check for replay attack warnings regularly
3. **Update Dependencies**: Keep `jsonwebtoken`, `k256`, and crypto libs current
4. **DID Resolution**: Ensure identity resolver is healthy (impacts all auth)
5. **Nonce Cleanup**: Verify background jobs are running (check logs every 5 min)

### For Developers

1. **JWT Lifetime**: Never extend beyond 60 seconds (ATProto requirement)
2. **Nonce Reuse**: Always consume nonces after validation
3. **Authorization**: Always check `repo` matches `auth.did()` before mutations
4. **Rate Limiting**: Apply `check_cross_pds()` to all federated endpoints
5. **Error Messages**: Don't leak implementation details in auth failures

### Configuration

```bash
# .env
FEDERATION_ENABLED=true
FEDERATION_RELAY_URLS=wss://relay.example.com

# Rate limiting (optional overrides)
RATE_LIMIT_CROSS_PDS_RPS=10      # Default: 10 req/s
RATE_LIMIT_AUTHENTICATED_RPS=100 # Default: 100 req/s
```

---

## Threat Model

### Mitigated Threats

| Threat | Mitigation |
|--------|------------|
| Replay Attacks | Nonce tracking with 120s retention |
| Token Theft | Short-lived JWTs (<60s) |
| DID Spoofing | Cryptographic verification via DID resolution |
| Cross-Repo Access | Authorization checks on every mutation |
| DoS via Federation | 10x stricter rate limiting for cross-PDS |
| Compromised PDS | No trust in origin PDS - verify JWTs cryptographically |

### Unmitigated / Residual Risks

| Risk | Severity | Notes |
|------|----------|-------|
| Nonce Store Loss (Restart) | MEDIUM | In-memory store - switch to Redis for production |
| DID Document Tampering | LOW | Blockchain/PLC directory provides integrity |
| Clock Skew Attacks | LOW | 2-minute window for `iat` validation |

### Accepted Advisory: RUSTSEC-2026-0118 (hickory-proto NSEC3 unbounded-loop)

**Severity:** High (per RUSTSEC). **Status in Aurora Locus:** **accepted —
exposure proven unreachable in this build.**

**Background.** `hickory-proto` (transitively via `hickory-resolver`) carries
the NSEC3 unbounded-loop vulnerability advised at
[RUSTSEC-2026-0118](https://rustsec.org/advisories/RUSTSEC-2026-0118).
Upstream has not published a fix as of the v0.6 cycle; the bump to
`hickory-resolver 0.26.1` (M1.3, chainlink #152) patches the sibling
RUSTSEC-2026-0119 (Moderate) but cannot address the High because no
patched release exists.

**Why we accept rather than mitigate.** The vulnerable function
`verify_nsec3` lives in
`hickory-proto::dnssec::dnssec_dns_handle::nsec3_validation`. It has exactly
one call site, in `DnssecDnsHandle::verify_nsec_proof`. The `DnssecDnsHandle`
is only constructed by `ResolverBuilder::build()` when BOTH of the following
hold:

1. The `__dnssec` cargo feature is active on `hickory-resolver`.
2. `ResolverOpts.validate` is `true`.

Aurora Locus's [`Cargo.toml`](Cargo.toml) declares
`hickory-resolver = "0.26.1"` with default features only (`default =
["system-config", "tokio"]`); `__dnssec` is NOT in defaults and we activate
no `dnssec-ring` / `dnssec-aws-lc-rs` feature. `ResolverOpts::Default`
sets `validate: false`, and `parse_resolv_conf` in
`hickory-resolver/src/system_conf/unix.rs` never writes that field. Under
`TokioResolver::builder_tokio().build()` (the call shape we use at
[`src/federation/dns_resolver.rs`](src/federation/dns_resolver.rs)
`HickoryDnsTxtResolver::from_system`) the constructed handle is
unconditionally `LookupEither::Retry`, never
`LookupEither::Secure(DnssecDnsHandle)`. The NSEC3 validation path is
unreachable.

**Transitive scope note.** `Cargo.lock` resolves a SECOND copy of
`hickory-resolver 0.25.2` via `proto-blue-identity v0.3.2` (transitive
through `proto-blue`). Our direct usage in `src/federation/dns_resolver.rs`
is on the 0.26.1 binary and the reachability proof above governs it. The
proto-blue-identity transitive copy is outside the v0.6 M1.3 bump scope; if
that copy's DNS calls also flow through the unreachable-path shape (default
features + no DNSSEC validation) the same reasoning applies, but the
verification is upstream's responsibility and we have not audited it. A
future cycle where proto-blue ships a release pinning hickory `>= 0.26.1`
will collapse the duplicate.

**Reassessment triggers.** This disposition must reopen if ANY of:

- The `hickory-resolver` pin in `Cargo.toml` gains a `dnssec-ring` or
  `dnssec-aws-lc-rs` feature (enables `__dnssec`).
- Any code path constructs a `ResolverBuilder` and sets
  `options.validate = true` (no such site exists today; future Phase-B-only
  constructor paths must NOT touch `validate`).
- Upstream `hickory-proto` publishes a fix for RUSTSEC-2026-0118 — at that
  point bump and remove this acceptance entry.

**Verification surface.** Live exercise of the lexicon DNS path
(Scenario 13, Cluster 1 Member 1.2) confirms the resolver still produces the
expected behaviour post-bump. The unreachability of NSEC3 itself is a
static-source proof (above); there is no positive test (you cannot test
that unreachable code didn't run).

### Recently Resolved Risks

| Risk | Resolution Date | Notes |
|------|-----------------|-------|
| Signing Key Extraction | 2025-12-27 | Full multibase-to-PEM conversion implemented |
| DPoP Key Parsing | 2025-12-27 | JWK-to-EC key conversion fully implemented |

---

## Compliance

### ATProto Specification Conformance

- ✅ Service Auth JWT format (ES256, <60s, DID-based)
- ✅ DPoP proof structure (RFC 9449)
- ✅ JWK handling (full EC key parsing implemented)
- ✅ Nonce-based replay prevention
- ✅ No callback to origin PDS

### Security Standards

- OWASP Top 10: Addressed authentication, authorization, and rate limiting
- RFC 9449: DPoP implementation (partial - see limitations)
- RFC 7638: JWK Thumbprint for token binding

---

## Security Contact

For security vulnerabilities, please create a private security advisory on GitHub or contact the maintainers directly.

**Do not disclose security issues publicly until a fix is available.**

---

## Changelog

| Date | Version | Changes |
|------|---------|---------|
| 2025-12-27 | Phase 4.2 | Verified DPoP JWK parsing is complete, updated docs |
| 2025-12-27 | Phase 4.1 | Fixed multibase-to-PEM key conversion in identity resolver |
| 2025-11-05 | Phase 4 | Initial cross-PDS authentication implementation |

---

**Last Updated**: 2025-12-27
**Phase**: 4 - Cross-PDS Authentication
**Status**: Development (Not Production-Ready)
