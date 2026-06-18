/// Unified error types for Aurora Locus PDS
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use proto_blue::lex_data::Cid;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::blob_store::store::QuarantinePublicReason;

/// Main error type for the PDS
#[derive(Error, Debug)]
pub enum PdsError {
    /// Database errors
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    /// Authentication errors
    #[error("Authentication failed: {0}")]
    Authentication(String),

    /// Authorization errors
    #[error("Not authorized: {0}")]
    Authorization(String),

    /// Validation errors
    #[error("Validation error: {0}")]
    Validation(String),

    /// Repository errors
    #[error("Repository error: {0}")]
    #[allow(dead_code)] // Future repository errors
    Repository(String),

    /// Blob storage errors
    #[error("Blob storage error: {0}")]
    BlobStorage(String),

    /// DID resolution errors
    #[error("DID resolution error: {0}")]
    #[allow(dead_code)] // Future DID resolution errors
    DidResolution(String),

    /// Identity resolution errors (genuine resolution failures — DNS
    /// timeout, PLC unreachable, network error, etc.). Falls through
    /// to HTTP 500. For the distinct "resolution completed but the
    /// handle does not resolve to any DID" case (client-input issue,
    /// not server-side failure), use `HandleNotFound` instead.
    #[error("Identity resolution error: {0}")]
    IdentityResolution(String),

    /// `com.atproto.identity.resolveHandle` completed resolution and
    /// determined the handle does not resolve to any DID. Per the
    /// ATProto lexicon for `com.atproto.identity.resolveHandle`, the
    /// canonical error name is `HandleNotFound`. Mapped to HTTP 400.
    /// Distinct from `IdentityResolution` so the not-found case
    /// (client error) doesn't get conflated with genuine resolution
    /// infrastructure failures (server error).
    #[error("Handle not found: {0}")]
    HandleNotFound(String),

    /// Rate limiting errors
    #[error("Rate limit exceeded")]
    RateLimitExceeded { retry_after: std::time::Duration },

    /// Not found errors
    #[error("Not found: {0}")]
    NotFound(String),

    /// Conflict errors (e.g., duplicate account)
    #[error("Conflict: {0}")]
    Conflict(String),

    /// Internal server errors
    #[error("Internal error: {0}")]
    Internal(String),

    /// ATProto SDK errors
    #[error("ATProto error: {0}")]
    #[allow(dead_code)] // Future ATProto errors
    AtProto(String),

    /// IO errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// proto-blue DAG-CBOR encoder error. Surfaces through the `?`
    /// operator in firehose-frame encoding (Arc 14 §7.4 Step 1.0(a)(iv)).
    /// Mapped to HTTP 500 (or WebSocket close 1011 for streaming).
    #[error("DAG-CBOR encoding error: {0}")]
    CborEncoding(String),

    /// Arc 16b §9.2 — `blob_metadata` row not found for the supplied
    /// CID. Emitted by `verify_blob_and_make_permanent` (STRICT) when
    /// a record-write references a CID with no upload. Mapped to HTTP
    /// 400 + `BlobNotFound` envelope per Arc 16e §9.5.3.5 R0c.A
    /// (matches bsky-PDS verbatim at
    /// `packages/pds/src/actor-store/blob/transactor.ts:259-260`).
    /// `#[allow(dead_code)]` while no production caller exists; Arc 16e
    /// Step 2 wires STRICT into the apply_writes Phase B path and
    /// removes the annotation.
    ///
    /// **Context**: this is the "record body references a CID with no
    /// uploaded blob" case (HTTP 400 = bad request from client input).
    /// Distinct from the admin-context "queried blob doesn't exist in
    /// storage" case (HTTP 404 = resource not found) which routes
    /// through `src/api/admin.rs::xrpc_blob_not_found_error` directly,
    /// bypassing this variant.
    #[error("Could not find blob: {0}")]
    #[allow(dead_code)]
    BlobNotFound(String),

    /// Arc 16e §9.5.3.5 — record body contains a CID that is malformed
    /// (unparseable multibase / multihash) OR non-DASL-compliant (not
    /// `CIDv1`, or not raw/DAG-CBOR codec, or not SHA-256 hash).
    /// Emitted by the validate-phase walker `extract_blob_cids` at
    /// `src/repository/blob_refs.rs` before any state mutation occurs.
    /// Mapped to HTTP 400 + typed `InvalidCid` envelope — deliberate
    /// stricter-typed-than-bsky-PDS posture per §9.5.5.10. bsky-PDS
    /// emits the `InvalidRequest` umbrella for the same condition;
    /// Aurora-Locus elects a typed wire code consistent with the
    /// Arc 14-era pattern (`RepoTakendown`, `RepoSuspended`, etc.).
    /// Stricter REJECTION codes cannot break federation interop —
    /// returned to the malformed-input client, never emitted to the
    /// firehose.
    #[error("Invalid CID in record body: {0}")]
    InvalidCid(String),

    /// JWT errors
    #[error("JWT error: {0}")]
    Jwt(String),

    /// Account taken down
    #[error("Account taken down: {0}")]
    AccountTakenDown(String),

    /// Account suspended
    #[error("Account suspended: {0}")]
    AccountSuspended(String),

    /// Arc 14 §7.3.5 — sync-namespace handler called against a DID
    /// with no actor row. Distinct from the generic `NotFound` so
    /// the wire-emitted `error` name is `"RepoNotFound"` per spec.
    /// Mapped to HTTP 404.
    #[error("Could not find repo for DID: {0}")]
    RepoNotFound(String),

    /// Arc 14 §7.3.5 — sync-namespace handler called against a
    /// takendown repo (and the caller is not admin/self). Mapped
    /// to HTTP 400 to match bsky-PDS `InvalidRequestError` default.
    #[error("Repo has been takendown: {0}")]
    RepoTakendown(String),

    /// Arc 14 §7.3.5 — sync-namespace handler called against a
    /// deactivated repo (and the caller is not admin/self). Mapped
    /// to HTTP 400.
    #[error("Repo has been deactivated: {0}")]
    RepoDeactivated(String),

    /// Arc 14 §7.3.5 — sync-namespace handler called against a
    /// suspended repo (and the caller is not admin/self). Mapped
    /// to HTTP 400. v0.5 source: test-affordance direct DB writes
    /// (no production setter; Arc 14 §7.1.2 segregation).
    #[error("Repo has been suspended: {0}")]
    RepoSuspended(String),

    /// Arc 14 §7.3.5 — sync-namespace handler called against a
    /// repo whose state is detected as desynchronized. Mapped
    /// to HTTP 400. v0.5 source: test-affordance direct DB writes
    /// (no production setter; Arc 14 §7.1.2 segregation).
    #[error("Repo has been desynchronized: {0}")]
    RepoDesynchronized(String),

    /// Arc 13 §6.3.4 / §6.3.6 — PLC directory's audit log for
    /// this DID's last accepted op is a `plc_tombstone`. The DID
    /// is terminally retired; no further ops are valid. Mapped
    /// to HTTP 400 in handler contexts.
    #[error("DID tombstoned: {0}")]
    DidTombstoned(String),

    /// Sequencer leader is on a different instance — caller should retry
    /// (load balancer will route to the leader on retry). Mapped to HTTP
    /// 503 Service Unavailable. See chainlink #89 / docs/AURORA_DESIGN.md §5.4.1.
    #[error("Sequencer leader is on a different instance: {0}")]
    NotLeader(String),

    /// Aurora Step 0.6 §2 (chainlink #130 / Arc 4): the embedded-ID action
    /// (`ResolveAppeal`/`EscalateAppeal` and similar) was called with a
    /// `subjects[0]` whose Subject *variant* doesn't match the variant
    /// resolved through the appeal's foreign-key chain. Mapped to HTTP 400.
    /// Distinct from `SubjectTargetMismatch` so operators can tell
    /// "wrong kind of subject" apart from "right kind, wrong identifier".
    #[error("Subject variant mismatch: expected {expected}, got {got}")]
    SubjectVariantMismatch { expected: String, got: String },

    /// Aurora Step 0.6 §2 (chainlink #130 / Arc 4): the embedded-ID action's
    /// `subjects[0]` matches the resolved variant but its *identifier* (DID
    /// for Repo, URI for Record, CID for Blob) doesn't. Mapped to HTTP 400.
    /// Distinct from `SubjectVariantMismatch` so operators can tell
    /// "wrong identifier" apart from "wrong kind of subject".
    #[error("Subject target mismatch: expected {expected}, got {got}")]
    SubjectTargetMismatch { expected: String, got: String },

    /// Aurora Step 0.6 §2 (chainlink #130 / Arc 4): defensive — appeal row
    /// has all three foreign-key columns NULL, so there's no target to
    /// validate against. Today's `submit_appeal` enforces "at least one set"
    /// via code-level invariant (no schema CHECK); orphan rows shouldn't
    /// exist but if one slipped in via direct SQL, this surfaces it. Mapped
    /// to HTTP 400.
    #[error("Orphaned appeal: appeal {appeal_id} has no FK to moderation/report/quarantine")]
    OrphanedAppeal { appeal_id: i64 },

    // ============================================================
    // Arc 16f §9.6.3.6 + §9.6.1.1 — CAR-import error vocabulary
    // (Aurora-owned tools.aurora.repo.importRepo namespace per
    // Option B). All variants below are emitted by the importRepo
    // handler + supporting primitives; the handler + fetch path
    // land in Steps 2-3 of the Arc 16f impl plan.
    // ============================================================

    /// Internal control-flow signal: TOLERANT's `verify_blob_tolerant_or_signal`
    /// returned `NeedsFetch` for one or more CIDs in a batch — the
    /// caller-driven fetch-and-retry loop at §9.6.3.5 consumes this
    /// and dispatches `fetch_blob_from_origin` for each CID before
    /// re-attempting `apply_writes`. NEVER reaches the wire under
    /// normal operation; if it does, surface as HTTP 500 defensively.
    #[error("Phase B needs origin-fetch for {} blob(s)", cids.len())]
    #[allow(dead_code)]
    NeedsBlobFetch { cids: Vec<Cid> },

    /// Arc 16f §9.6.3.6 — record body references a CID whose blob is
    /// quarantined. Validate-phase at §9.6.3.1 step 5 catches the
    /// common case before Phase A; this variant also fires from
    /// TOLERANT's defense-in-depth Phase B check when quarantine
    /// lands between validate-phase and Phase B. Mapped to HTTP 400.
    /// Wire payload exposes coarse `public_reason` only; operator-
    /// internal `blob_quarantine.reason` is NOT exposed (round-1 F20).
    #[error("Imported record references quarantined blob: {cid}")]
    #[allow(dead_code)]
    QuarantinedBlobReferenced {
        cid: Cid,
        public_reason: QuarantinePublicReason,
    },

    /// Arc 16f §9.6.3.3 / §9.6.3.5 — origin PDS returned a durable
    /// client-side failure for a blob fetch (4xx — typically 404 Not
    /// Found or 403 Forbidden). No retry per §9.6.3.3 step 6 (durable
    /// failures stay durable). Mapped to HTTP 502 Bad Gateway —
    /// origin's response was the cause; Aurora-Locus is the
    /// intermediary.
    #[error("Origin PDS rejected blob fetch for {cid}: {status_or_reason}")]
    #[allow(dead_code)]
    OriginFetchClientError {
        cid: Cid,
        status_or_reason: String,
    },

    /// Arc 16f §9.6.3.5 — fetch-and-retry loop exhausted its retry
    /// budget OR aggregated multiple per-CID failures within a single
    /// retry round. `per_cid_failures` carries the full operator-
    /// visible context per round-1 F18 closure. Mapped to HTTP 502
    /// Bad Gateway — origin-side failure was the root cause.
    #[error("Failed to fetch {} blob(s) from origin PDS", per_cid_failures.len())]
    #[allow(dead_code)]
    OriginFetchExhausted {
        per_cid_failures: Vec<(Cid, String)>,
    },

    /// Arc 16f §9.6.3.1 step 4 — CAR commit-chain signature verification
    /// failed against the importing DID's PLC-resolved signing key
    /// history. Mapped to HTTP 400 — bad client input.
    #[error("CAR commit-chain signature verification failed")]
    #[allow(dead_code)]
    InvalidCommitSignature,

    /// Arc 16f §9.6.3.1 step 1 — importRepo handler invoked for a
    /// DID that has no local actor (no prior createAccount).
    /// Precursor account-setup required. Mapped to HTTP 400.
    #[error("Actor not initialized; createAccount required before importRepo")]
    #[allow(dead_code)]
    ActorNotInitialized,

    /// Arc 16f §9.6.3.3 step 4 — pre-fetch HEAD response indicates
    /// the blob exceeds `service.max_blob_fetch_size`. Per round-1
    /// F10 closure, reject before downloading the body to keep
    /// per-blob memory bounded. Mapped to HTTP 413 Payload Too
    /// Large.
    #[error("Blob {cid} exceeds max_blob_fetch_size: {size} bytes")]
    #[allow(dead_code)]
    BlobTooLarge { cid: Cid, size: u64 },

    /// Arc 16f §9.6.3.1 step 1 — importRepo single-flight handler
    /// lock was contended (concurrent importRepo on the same
    /// importing DID). Round-1 F6 closure: try-acquire + fail-fast
    /// posture per §9.6.5.8. Mapped to HTTP 409 Conflict.
    #[error("Concurrent mutation in progress for this repo")]
    #[allow(dead_code)]
    ConcurrentMutation,

    /// Arc 16f §9.6.3.1 step 3 + Aurora error vocabulary — the
    /// uploaded CAR body fails Aurora's structural acceptance gates
    /// before Phase A can begin. Concrete causes that map to this
    /// variant for v0.5 Step 3:
    /// - Streaming size cap exceeded (HTTP 413 semantic, surfaced
    ///   as 400 here — see [skydeval]: a v0.6+ refinement could
    ///   split this into a dedicated `ImportTooLarge { size, limit }`
    ///   variant if operators need wire-distinguishable 413 vs 400;
    ///   v0.5 ships unified for variant-count economy).
    /// - CAR root commit's `did` field does not match the
    ///   authenticated importing DID.
    /// - `verify_diff_car` returned a structural / encoding error
    ///   (malformed CAR header, unparseable blocks, MST load
    ///   failure).
    ///
    /// Mapped to HTTP 400 `InvalidCar`.
    #[error("Invalid CAR: {0}")]
    #[allow(dead_code)]
    InvalidCar(String),

    /// Arc 17 §17.3.6 — lexicon fetch (DNS / DID-resolve / HTTP)
    /// exhausted retries OR surfaced a terminal failure. The
    /// `failure_class` field carries the round-1 F14 forensic-log
    /// taxonomy: `"dns_fail"`, `"did_fail"`, `"pds_unreachable"`,
    /// `"http_5xx"`, `"http_4xx"`, `"timeout"`,
    /// `"authority_tombstoned"`, `"authority_ambiguous"`,
    /// `"invalid_schema"`, `"invalid_signature"` (the last added
    /// alongside the §17.7 lexicon-fetch sig-verify wire-up,
    /// v0.6 Cluster 3 Member 3.1). Mapped to HTTP 502 per
    /// §17.3.6 wire-format alignment with Arc 16f's `OriginFetchExhausted`.
    #[error("Lexicon fetch failed for {nsid} ({failure_class}): {source_detail}")]
    #[allow(dead_code)]
    LexiconFetchFailed {
        nsid: String,
        failure_class: &'static str,
        source_detail: String,
    },

    /// Arc 17 §17.3.6 — fetched lexicon document failed schema
    /// validation in `proto_blue::lexicon::Lexicons::add` (the doc is
    /// structurally invalid as an ATProto lexicon). Mapped to HTTP
    /// 500 per §17.3.6 (server-side state corruption, not a client
    /// error).
    #[error("Lexicon document {nsid} failed schema validation: {detail}")]
    #[allow(dead_code)]
    LexiconInvalidSchema { nsid: String, detail: String },

    /// Arc 17 §17.3.6 — DNS TXT-resolved authority DID does not match
    /// the DID actually hosting the lexicon record. Mapped to HTTP
    /// 502 per §17.3.6.
    #[error("Lexicon authority mismatch for {nsid}: expected {expected}, found {found}")]
    #[allow(dead_code)]
    LexiconAuthorityMismatch {
        nsid: String,
        expected: String,
        found: String,
    },

    /// Arc 17 §17.3.6 (round-1 F5 closure) — DNS `_lexicon.<host>`
    /// TXT lookup returned multiple TXT records OR a single record
    /// with multiple `did=` entries. Aurora hard-fails per §17.3.1
    /// step 3c; matches bsky-PDS strict posture at the reference
    /// SHA (Step 0.0a ratification). Mapped to HTTP 502.
    #[error("Lexicon authority ambiguous for {nsid}: {} candidate(s)", candidates.len())]
    #[allow(dead_code)]
    LexiconAuthorityAmbiguous {
        nsid: String,
        candidates: Vec<String>,
    },

    /// Arc 17 §17.3.6 (round-1 F13 closure) — authority DID is
    /// tombstoned in PLC. Distinct from a generic `did_fail` so
    /// operators can grep `failure_class = "authority_tombstoned"`
    /// directly. Mapped to HTTP 502.
    #[error("Lexicon authority {did} for {nsid} is tombstoned")]
    #[allow(dead_code)]
    LexiconAuthorityTombstoned { nsid: String, did: String },

    /// Arc 17 §17.3.6 (round-1 F9 closure) — NSID fails ATProto spec
    /// segment validation (each segment must match
    /// `[a-z][a-z0-9-]*[a-z0-9]`, total ≥ 3 segments). Mapped to
    /// HTTP 400 (client supplied a malformed NSID).
    #[error("Invalid NSID: {nsid}")]
    #[allow(dead_code)]
    LexiconInvalidNsid { nsid: String },

    /// Arc 17 §17.3.6 (round-1 F7 closure) — record failed
    /// lexicon-driven schema validation against the fetched lexicon
    /// doc. Structured `field_path` is extracted from proto-blue's
    /// `ValidationError::InvalidValue { path, message }`;
    /// `expected_type` and `actual_summary` are heuristic-derived
    /// from the message text for v0.5 (proto-blue's structured-field
    /// shape may evolve in v0.6+). Mapped to HTTP 400.
    #[error("Schema violation in {collection} at {field_path}: {detail}")]
    #[allow(dead_code)]
    SchemaViolation {
        collection: String,
        field_path: String,
        expected: Option<String>,
        actual_summary: Option<String>,
        detail: String,
    },

    /// Arc 17 §17.3.6 (round-1 F2 closure) — record NSID matches the
    /// configured denylist; record is rejected outright with no
    /// lexicon fetch attempted. Distinct from `LexiconFetchFailed`
    /// (which is a fetch-attempt outcome). Mapped to HTTP 400.
    #[error("Namespace denied: {nsid}")]
    #[allow(dead_code)]
    NamespaceDenied { nsid: String },

    /// Arc 17 §17.3.7 — admin endpoint called against an instance
    /// where lexicon is disabled (`PDS_LEXICON_ENABLED=false`, the
    /// v0.5 default). The endpoint cannot fulfill the request because
    /// there is no resolver to delegate to. Mapped to HTTP 503 —
    /// "the lexicon subsystem is not available; enable it and
    /// retry." Distinct from `LexiconFetchFailed` (which is a runtime
    /// fetch outcome, not a configuration state).
    #[error("Lexicon subsystem is disabled (PDS_LEXICON_ENABLED=false)")]
    #[allow(dead_code)]
    LexiconDisabled,

    // v0.7 arc 1 — kryphocron dispatcher errors. The dispatcher in
    // `RepositoryManager::validate_write` returns these per v07_DESIGN.md
    // §6 Order A pseudocode. All map to HTTP 400 (client-side error)
    // unless otherwise noted at the variant. The arc 1 ship state
    // populates `KryphocronRecordNotYetSupported` for every registered
    // NSID since no dedicated endpoints exist yet; the others surface
    // when their respective edge conditions trip.

    /// Master-switch-off rejection for any NSID claiming the closed
    /// `tools.kryphocron.*` namespace. Per v07_DESIGN.md §6 lines
    /// 3247-3257: the wording is deliberately generic so the
    /// master-switch-off state is behaviorally indistinguishable
    /// from "kryphocron not compiled in" for clients, avoiding
    /// namespace-knowledge disclosure to probes.
    #[error("Unsupported namespace: {nsid}")]
    UnsupportedNamespace { nsid: String },

    /// NSID claims the closed `tools.kryphocron.*` namespace but is
    /// not present in `KRYPHOCRON_LEXICON_REGISTRY`. The kryphocron
    /// namespace is closed; A1's dynamic resolver does not get to
    /// accept records claiming it. Indicates a client trying to
    /// register a fake NSID under the kryphocron prefix.
    #[error("Unregistered NSID in closed kryphocron namespace: {nsid}")]
    KryphocronUnregisteredNsidInClosedNamespace { nsid: String },

    /// `Tier::from_nsid` returned a non-`NotRegistered` error variant
    /// for an NSID claiming the kryphocron namespace. Reserved for
    /// forward compatibility — currently unreachable because
    /// `UnknownNsid` has only the `NotRegistered` variant in
    /// kryphocron 0.1/0.2. Reachable when a future kryphocron release
    /// adds tier-classification variants (reserved / version-
    /// mismatched lexicons) and the dispatcher's `Ok(tier)` branch
    /// ships in arc 3+.
    #[error("kryphocron tier classification failed for {nsid}: {detail}")]
    #[allow(dead_code)]
    KryphocronTierClassificationFailed { nsid: String, detail: String },

    /// `Tier::from_nsid` returned a non-`NotRegistered` error variant
    /// for an NSID NOT claiming the kryphocron namespace. Same forward-
    /// compat posture as the kryphocron variant above.
    #[error("Tier classification failed for {nsid}: {detail}")]
    #[allow(dead_code)]
    TierClassificationFailed { nsid: String, detail: String },

    /// Registry says NSID is registered but `kryphocron::lexicons()`'s
    /// `get_def(NSID#main)` returned `None`. Indicates substrate-side
    /// drift between the registry (compiled-in metadata) and the
    /// runtime lexicon documents — should not happen in correct
    /// kryphocron 0.2+ builds. Mapped to HTTP 500. Reachable only when
    /// `kryphocron::lexicon_validate` ships (arc 3+ Ok(tier) branch).
    #[error("kryphocron lexicon definition missing for {def_uri}")]
    #[allow(dead_code)]
    KryphocronLexiconMissing { def_uri: String },

    /// `kryphocron::lexicons().get_def(NSID#main)` returned something
    /// other than `LexUserType::Record`. Indicates a malformed
    /// kryphocron lexicon JSON at the substrate (the `#main` def of
    /// a record lexicon must be a record). Mapped to HTTP 500.
    /// Reachable only when `kryphocron::lexicon_validate` ships
    /// (arc 3+ Ok(tier) branch).
    #[error("kryphocron lexicon at {def_uri} is not a record def")]
    #[allow(dead_code)]
    KryphocronLexiconNotRecord { def_uri: String },

    /// `proto_blue::lexicon::validate_record` failed on a kryphocron
    /// record. The supplied record does not conform to the declared
    /// lexicon shape. Mapped to HTTP 400. Reachable only when
    /// `kryphocron::lexicon_validate` ships (arc 3+ Ok(tier) branch).
    #[error("kryphocron lexicon validation failed for {nsid}: {detail}")]
    #[allow(dead_code)]
    KryphocronLexiconValidationFailed { nsid: String, detail: String },

    /// Per v07_DESIGN.md §8: a `tools.kryphocron.*` write arrived via
    /// the generic path when a dedicated endpoint is the legitimate
    /// origin. The optional `suggested_endpoint` directs the client.
    /// Arc 1 ship state never sets `suggested_endpoint` because no
    /// dedicated endpoints exist yet; arc 3+ populates it from the
    /// dedicated-endpoint registration code.
    #[error("kryphocron record requires the dedicated endpoint for {nsid}")]
    #[allow(dead_code)]
    KryphocronRecordRequiresDedicatedEndpoint {
        nsid: String,
        suggested_endpoint: Option<String>,
    },

    /// Per v07_DESIGN.md §8: a `tools.kryphocron.*` write arrived for
    /// a registered NSID that has no dedicated endpoint yet (arc 1
    /// default for every registered NSID; arc 3+ overrides for endpoints
    /// that ship). The error tells clients to wait for the endpoint
    /// to land rather than to find an existing alternative.
    #[error("kryphocron NSID {nsid} is registered but no endpoint exposes it yet")]
    KryphocronRecordNotYetSupported { nsid: String },

    /// v0.7 arc 2 step 4 — cascade token verification failed inside
    /// `bind_pipeline`'s `Cascade` arm. The token's
    /// `CascadeContext`-identity, source, or spent-marker check
    /// failed, OR no `CascadeContext` was active at the verify
    /// site. Mapped to HTTP 403 (the caller's authorization
    /// claim is invalid). Reachable when arc 2 step 5's
    /// dedicated endpoints and cascade-initiating handlers ship
    /// the producer side; arc 2 step 4 wires only the consumer.
    #[error("kryphocron cascade token invalid: {0}")]
    #[allow(dead_code)] // produced by step 5+ cascade flows
    KryphocronCascadeTokenInvalid(String),

    /// Arc H §7.2.5 / #282 — a `Cascade`-authorized write failed the
    /// `bind_pipeline` Cascade arm's per-source shape predicates (§2.4.1):
    /// originator ≠ repo owner, target collection ≠ `policy.audience`,
    /// operation ≠ `Update`, missing `swap_cid`, or a non-`BlockCascade`
    /// source reaching the audience path. Because the cascade handler builds
    /// every cascade `Update` from a live read and removes only the subject
    /// (§2.4.2), a correctly-built cascade *cannot* trip these checks — so this
    /// is unambiguously a bug or an attack, never a legitimate-but-unlucky
    /// case. The handler routes it to **abort the whole cascade pass, loudly**
    /// (§3.2, rev3 P-1), distinct from the best-effort swap/transient misses.
    /// Mapped to HTTP 403 (the write's authorization shape is invalid).
    #[error("kryphocron cascade write rejected: {0}")]
    KryphocronCascadeWriteRejected(String),

    /// Arc H §7.2.5 / #282 (M-10 / ST-3) — an `Update`/`Delete` carrying a
    /// `swap_cid` did not match the record's current CID at apply time
    /// (`repository.rs` CAS arm). For the block cascade this is the expected
    /// "a concurrent `manageAudience` edit moved the record between my live
    /// read and the apply" signal: the pinned write is stale, so the handler
    /// **skips that one audience and continues** (§3.2 best-effort) — never
    /// aborts. A dedicated variant (was untyped `Validation`) so the handler
    /// can route on it cleanly, type-distinct from shape-rejects and generic
    /// transients. Mapped to HTTP 409 Conflict.
    #[error("swap CID mismatch: {0}")]
    SwapCidMismatch(String),

    /// v0.7 arc 2 step 4 — `validate_write` reached the bind
    /// pipeline path (`kryphocron_authorization` is `Some(_)`)
    /// without an active lent shared-DB transaction on the
    /// storage layer. Indicates `apply_writes` did NOT open the
    /// relay-race scope (typically because the
    /// `RepositoryManager` was constructed via `::new()` without
    /// chaining `with_shared_pool`, the back-compat test-only
    /// path). Production handlers go through `for_writer` which
    /// always plumbs the shared pool, so this is unreachable in
    /// production.
    ///
    /// Mapped to HTTP 500 because it signals a programmer error,
    /// not a client error: callers who construct a kryphocron-
    /// authorized write must route it through a
    /// shared-pool-equipped `RepositoryManager`.
    #[error("kryphocron bind pipeline reached without an active apply_writes scope")]
    #[allow(dead_code)] // reachable only via programmer error
    KryphocronBindPipelineOutsideScope,

    /// v0.9 Arc D (#237a) — a private-tier record was stored under a
    /// kryphocron `CodecId` that does not match the codec currently
    /// installed in this deployment (cross-peer / cross-version codec
    /// skew, per kryphocron 0.3 §6.2). The record's bytes are valid, but
    /// this deployment has no codec to decode them, so an authorized
    /// reader's decode-on-read cannot produce plaintext. Mapped to HTTP
    /// 410 Gone with a clear codec-mismatch message rather than a generic
    /// 500: the condition is a deployment/codec-version state, not a
    /// server fault. The encoded form is still returnable to consumers
    /// that don't decode (federation, non-authorized readers); only the
    /// decode path fails closed.
    #[error(
        "record encoded under codec {stored} but this deployment has codec {installed} installed"
    )]
    KryphocronCodecUnavailable {
        /// The `CodecId` the record was stored under.
        stored: String,
        /// The `CodecId` currently installed in this deployment.
        installed: String,
    },
}

/// Manual PartialEq implementation for PdsError
/// Note: Database and Io variants cannot be truly compared due to underlying types
impl PartialEq for PdsError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (PdsError::Authentication(a), PdsError::Authentication(b)) => a == b,
            (PdsError::Authorization(a), PdsError::Authorization(b)) => a == b,
            (PdsError::Validation(a), PdsError::Validation(b)) => a == b,
            (PdsError::Repository(a), PdsError::Repository(b)) => a == b,
            (PdsError::BlobStorage(a), PdsError::BlobStorage(b)) => a == b,
            (PdsError::DidResolution(a), PdsError::DidResolution(b)) => a == b,
            (PdsError::IdentityResolution(a), PdsError::IdentityResolution(b)) => a == b,
            (
                PdsError::RateLimitExceeded { retry_after: a },
                PdsError::RateLimitExceeded { retry_after: b },
            ) => a == b,
            (PdsError::NotFound(a), PdsError::NotFound(b)) => a == b,
            (PdsError::Conflict(a), PdsError::Conflict(b)) => a == b,
            (PdsError::Internal(a), PdsError::Internal(b)) => a == b,
            (PdsError::AtProto(a), PdsError::AtProto(b)) => a == b,
            (PdsError::Jwt(a), PdsError::Jwt(b)) => a == b,
            (PdsError::AccountTakenDown(a), PdsError::AccountTakenDown(b)) => a == b,
            (PdsError::AccountSuspended(a), PdsError::AccountSuspended(b)) => a == b,
            (PdsError::NotLeader(a), PdsError::NotLeader(b)) => a == b,
            (
                PdsError::SubjectVariantMismatch { expected: ae, got: ag },
                PdsError::SubjectVariantMismatch { expected: be, got: bg },
            ) => ae == be && ag == bg,
            (
                PdsError::SubjectTargetMismatch { expected: ae, got: ag },
                PdsError::SubjectTargetMismatch { expected: be, got: bg },
            ) => ae == be && ag == bg,
            (
                PdsError::OrphanedAppeal { appeal_id: a },
                PdsError::OrphanedAppeal { appeal_id: b },
            ) => a == b,
            // Arc 16f §9.6.3.6 + §9.6.1.1 — import error variants.
            (
                PdsError::NeedsBlobFetch { cids: a },
                PdsError::NeedsBlobFetch { cids: b },
            ) => a == b,
            (
                PdsError::QuarantinedBlobReferenced { cid: ac, public_reason: ap },
                PdsError::QuarantinedBlobReferenced { cid: bc, public_reason: bp },
            ) => ac == bc && ap == bp,
            (
                PdsError::OriginFetchClientError { cid: ac, status_or_reason: ar },
                PdsError::OriginFetchClientError { cid: bc, status_or_reason: br },
            ) => ac == bc && ar == br,
            (
                PdsError::OriginFetchExhausted { per_cid_failures: a },
                PdsError::OriginFetchExhausted { per_cid_failures: b },
            ) => a == b,
            (PdsError::InvalidCommitSignature, PdsError::InvalidCommitSignature) => true,
            (PdsError::ActorNotInitialized, PdsError::ActorNotInitialized) => true,
            (
                PdsError::BlobTooLarge { cid: ac, size: asz },
                PdsError::BlobTooLarge { cid: bc, size: bsz },
            ) => ac == bc && asz == bsz,
            (PdsError::ConcurrentMutation, PdsError::ConcurrentMutation) => true,
            (PdsError::InvalidCar(a), PdsError::InvalidCar(b)) => a == b,
            // Arc 17 §17.3.6 — lexicon variants.
            (
                PdsError::LexiconFetchFailed { nsid: an, failure_class: af, source_detail: as_ },
                PdsError::LexiconFetchFailed { nsid: bn, failure_class: bf, source_detail: bs },
            ) => an == bn && af == bf && as_ == bs,
            (
                PdsError::LexiconInvalidSchema { nsid: an, detail: ad },
                PdsError::LexiconInvalidSchema { nsid: bn, detail: bd },
            ) => an == bn && ad == bd,
            (
                PdsError::LexiconAuthorityMismatch { nsid: an, expected: ae, found: af },
                PdsError::LexiconAuthorityMismatch { nsid: bn, expected: be, found: bf },
            ) => an == bn && ae == be && af == bf,
            (
                PdsError::LexiconAuthorityAmbiguous { nsid: an, candidates: ac },
                PdsError::LexiconAuthorityAmbiguous { nsid: bn, candidates: bc },
            ) => an == bn && ac == bc,
            (
                PdsError::LexiconAuthorityTombstoned { nsid: an, did: ad },
                PdsError::LexiconAuthorityTombstoned { nsid: bn, did: bd },
            ) => an == bn && ad == bd,
            (
                PdsError::LexiconInvalidNsid { nsid: an },
                PdsError::LexiconInvalidNsid { nsid: bn },
            ) => an == bn,
            (
                PdsError::SchemaViolation { collection: ac, field_path: ap, expected: ae, actual_summary: aas, detail: ad },
                PdsError::SchemaViolation { collection: bc, field_path: bp, expected: be, actual_summary: bas, detail: bd },
            ) => ac == bc && ap == bp && ae == be && aas == bas && ad == bd,
            (
                PdsError::NamespaceDenied { nsid: an },
                PdsError::NamespaceDenied { nsid: bn },
            ) => an == bn,
            (PdsError::LexiconDisabled, PdsError::LexiconDisabled) => true,
            // v0.7 arc 1 — kryphocron dispatcher variants.
            (
                PdsError::UnsupportedNamespace { nsid: a },
                PdsError::UnsupportedNamespace { nsid: b },
            ) => a == b,
            (
                PdsError::KryphocronUnregisteredNsidInClosedNamespace { nsid: a },
                PdsError::KryphocronUnregisteredNsidInClosedNamespace { nsid: b },
            ) => a == b,
            (
                PdsError::KryphocronTierClassificationFailed { nsid: an, detail: ad },
                PdsError::KryphocronTierClassificationFailed { nsid: bn, detail: bd },
            ) => an == bn && ad == bd,
            (
                PdsError::TierClassificationFailed { nsid: an, detail: ad },
                PdsError::TierClassificationFailed { nsid: bn, detail: bd },
            ) => an == bn && ad == bd,
            (
                PdsError::KryphocronLexiconMissing { def_uri: a },
                PdsError::KryphocronLexiconMissing { def_uri: b },
            ) => a == b,
            (
                PdsError::KryphocronLexiconNotRecord { def_uri: a },
                PdsError::KryphocronLexiconNotRecord { def_uri: b },
            ) => a == b,
            (
                PdsError::KryphocronLexiconValidationFailed { nsid: an, detail: ad },
                PdsError::KryphocronLexiconValidationFailed { nsid: bn, detail: bd },
            ) => an == bn && ad == bd,
            (
                PdsError::KryphocronRecordRequiresDedicatedEndpoint { nsid: an, suggested_endpoint: ae },
                PdsError::KryphocronRecordRequiresDedicatedEndpoint { nsid: bn, suggested_endpoint: be },
            ) => an == bn && ae == be,
            (
                PdsError::KryphocronRecordNotYetSupported { nsid: a },
                PdsError::KryphocronRecordNotYetSupported { nsid: b },
            ) => a == b,
            (
                PdsError::KryphocronCascadeTokenInvalid(a),
                PdsError::KryphocronCascadeTokenInvalid(b),
            ) => a == b,
            (
                PdsError::KryphocronCascadeWriteRejected(a),
                PdsError::KryphocronCascadeWriteRejected(b),
            ) => a == b,
            (PdsError::SwapCidMismatch(a), PdsError::SwapCidMismatch(b)) => a == b,
            (
                PdsError::KryphocronBindPipelineOutsideScope,
                PdsError::KryphocronBindPipelineOutsideScope,
            ) => true,
            // Database and Io errors cannot be compared, so we use error message comparison
            (PdsError::Database(a), PdsError::Database(b)) => a.to_string() == b.to_string(),
            (PdsError::Io(a), PdsError::Io(b)) => a.to_string() == b.to_string(),
            _ => false,
        }
    }
}

/// XRPC error response format.
///
/// Base shape is `{error, message}` — the bsky-PDS-compatible
/// envelope. Aurora-specific structured-error wire shapes (Arc 16f
/// §9.6.3.5 OriginFetchExhausted's `per_cid_failures`, §9.6.3.6
/// QuarantinedBlobReferenced's `cid` + `public_reason`) extend the
/// envelope via additive optional fields. `serde(skip_serializing_if
/// = "Option::is_none")` keeps existing wire shapes unchanged for
/// variants that don't populate the extensions — backward-compatible
/// for clients keying on `{error, message}` only.
///
/// Round-1 F20 invariant for QuarantinedBlobReferenced: the
/// operator-internal `blob_quarantine.reason` text (e.g. "csam",
/// "matched signature SHA256:...") must NEVER appear in any field
/// of this envelope. The `public_reason` field carries only the
/// coarse [`QuarantinePublicReason`] class, populated via
/// `from_internal_reason_str` at the validate-phase reject site.
/// See the `quarantined_blob_referenced_wire_shape_*` tests in
/// the integration suite for the invariant proof.
#[derive(Debug, Serialize, Deserialize)]
pub struct XrpcErrorResponse {
    pub error: String,
    pub message: String,
    /// Arc 16f §9.6.3.6 — QuarantinedBlobReferenced wire field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    /// Arc 16f §9.6.3.6 — QuarantinedBlobReferenced wire field.
    /// Carries the coarse `QuarantinePublicReason` class
    /// (e.g. "abuse"); never the operator-internal reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_reason: Option<String>,
    /// Arc 16f §9.6.3.5 — OriginFetchExhausted wire field.
    /// Per-CID failure context aggregated by the fetch-and-retry loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_cid_failures: Option<Vec<PerCidFailure>>,
}

/// Arc 16f §9.6.3.5 — per-CID failure entry inside
/// [`XrpcErrorResponse::per_cid_failures`].
#[derive(Debug, Serialize, Deserialize)]
pub struct PerCidFailure {
    pub cid: String,
    pub reason: String,
}

/// Convert PdsError to HTTP response
impl IntoResponse for PdsError {
    fn into_response(self) -> Response {
        // chainlink #104 Fix 2a: capture the underlying error Display
        // BEFORE the match consumes self, so 5xx responses can log it
        // server-side. The HTTP body intentionally strips this detail
        // (line below) to avoid leaking internals to clients; the log
        // is the only place operators see the root cause.
        let error_display = self.to_string();

        // Arc 16f §9.6.3.5 / §9.6.3.6 (chainlink #126) — extract
        // structured fields for the variants that need them on the
        // wire BEFORE the match consumes self. Round-1 F20 invariant:
        // QuarantinedBlobReferenced extracts ONLY the coarse public
        // class (via QuarantinePublicReason's serde-rename-lowercase),
        // never the operator-internal blob_quarantine.reason text.
        let (cid_field, public_reason_field, per_cid_failures_field) = match &self {
            PdsError::QuarantinedBlobReferenced { cid, public_reason } => (
                Some(cid.to_string()),
                serde_json::to_value(public_reason)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from)),
                None,
            ),
            PdsError::OriginFetchExhausted { per_cid_failures } => (
                None,
                None,
                Some(
                    per_cid_failures
                        .iter()
                        .map(|(c, r)| PerCidFailure {
                            cid: c.to_string(),
                            reason: r.clone(),
                        })
                        .collect::<Vec<_>>(),
                ),
            ),
            _ => (None, None, None),
        };

        let (status, error_code, message) = match self {
            PdsError::Authentication(_) => (
                StatusCode::UNAUTHORIZED,
                "AuthenticationRequired",
                self.to_string(),
            ),
            PdsError::Authorization(_) => (StatusCode::FORBIDDEN, "Forbidden", self.to_string()),
            PdsError::Validation(_) => {
                (StatusCode::BAD_REQUEST, "InvalidRequest", self.to_string())
            }
            PdsError::NotFound(_) => (StatusCode::NOT_FOUND, "NotFound", self.to_string()),
            PdsError::Conflict(_) => (StatusCode::CONFLICT, "Conflict", self.to_string()),
            PdsError::RateLimitExceeded { .. } => (
                StatusCode::TOO_MANY_REQUESTS,
                "RateLimitExceeded",
                "Rate limit exceeded".to_string(),
            ),
            PdsError::AccountTakenDown(_) => {
                (StatusCode::FORBIDDEN, "AccountTakedown", self.to_string())
            }
            PdsError::AccountSuspended(_) => {
                (StatusCode::FORBIDDEN, "AccountSuspended", self.to_string())
            }
            PdsError::DidTombstoned(_) => (
                StatusCode::BAD_REQUEST,
                "DidTombstoned",
                self.to_string(),
            ),
            PdsError::HandleNotFound(_) => (
                StatusCode::BAD_REQUEST,
                "HandleNotFound",
                self.to_string(),
            ),
            // Arc 14 §7.3.5 / §7.6.5: sync-namespace typed errors.
            // HTTP status code defaults verified against bsky-PDS via
            // Step 0 Sub-step 0.D recon (envelope already-correct
            // Case A; just extend allowed `error` values).
            PdsError::RepoNotFound(_) => {
                (StatusCode::NOT_FOUND, "RepoNotFound", self.to_string())
            }
            PdsError::RepoTakendown(_) => {
                (StatusCode::BAD_REQUEST, "RepoTakendown", self.to_string())
            }
            PdsError::RepoDeactivated(_) => (
                StatusCode::BAD_REQUEST,
                "RepoDeactivated",
                self.to_string(),
            ),
            PdsError::RepoSuspended(_) => {
                (StatusCode::BAD_REQUEST, "RepoSuspended", self.to_string())
            }
            PdsError::RepoDesynchronized(_) => (
                StatusCode::BAD_REQUEST,
                "RepoDesynchronized",
                self.to_string(),
            ),
            // Arc 16e §9.5.3.5 R0c.A — STRICT's "record references missing
            // blob" path. HTTP 400 + wire shape matches bsky-PDS verbatim at
            // packages/pds/src/actor-store/blob/transactor.ts:259-260.
            // (Pre-Arc-16e this was HTTP 404; flipped at chainlink #107
            // cross-arc PR. Admin storage-lookup miss stays 404 via the
            // separate xrpc_blob_not_found_error helper in api/admin.rs.)
            PdsError::BlobNotFound(_) => {
                (StatusCode::BAD_REQUEST, "BlobNotFound", self.to_string())
            }
            // Arc 16e §9.5.3.5 / §9.5.5.10 — validate-phase walker rejection.
            // Typed wire code (deliberate stricter-than-bsky-PDS posture per
            // §9.5.5.10); bsky-PDS uses InvalidRequest umbrella for the same
            // condition. See chainlink #106 for the wire-vocabulary decision.
            PdsError::InvalidCid(_) => {
                (StatusCode::BAD_REQUEST, "InvalidCid", self.to_string())
            }
            PdsError::NotLeader(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "NotLeader",
                self.to_string(),
            ),
            PdsError::Database(_)
            | PdsError::Internal(_)
            | PdsError::Io(_)
            | PdsError::CborEncoding(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError",
                "Internal server error".to_string(), // Don't leak details
            ),
            // Arc 16f §9.6.3.6 — record body references quarantined blob.
            // Wire shape per round-1 F20: `public_reason` exposes coarse
            // class only; operator-internal detail (blob_quarantine.reason
            // text) is NOT leaked.
            PdsError::QuarantinedBlobReferenced { .. } => (
                StatusCode::BAD_REQUEST,
                "QuarantinedBlobReferenced",
                self.to_string(),
            ),
            // Arc 16f §9.6.3.5 — fetch-and-retry loop exhausted retries
            // OR aggregated multiple per-CID failures. Wire shape's full
            // per_cid_failures payload lands when the Step 3 importRepo
            // handler ships; for now the standard XrpcErrorResponse
            // envelope carries the summary message.
            PdsError::OriginFetchExhausted { .. } => (
                StatusCode::BAD_GATEWAY,
                "OriginFetchExhausted",
                self.to_string(),
            ),
            // Arc 16f §9.6.3.3 — origin PDS returned a durable client
            // failure (typically 4xx) for a blob fetch. Aurora-Locus is
            // the intermediary; the origin was the proximate cause.
            PdsError::OriginFetchClientError { .. } => (
                StatusCode::BAD_GATEWAY,
                "OriginFetchClientError",
                self.to_string(),
            ),
            // Arc 16f §9.6.3.1 step 4 — commit-chain signature
            // verification failed against importing DID's PLC-resolved
            // signing key.
            PdsError::InvalidCommitSignature => (
                StatusCode::BAD_REQUEST,
                "InvalidCommitSignature",
                self.to_string(),
            ),
            // Arc 16f §9.6.3.1 step 1 — importRepo against a DID with
            // no local actor; precursor createAccount required.
            PdsError::ActorNotInitialized => (
                StatusCode::BAD_REQUEST,
                "ActorNotInitialized",
                self.to_string(),
            ),
            // Arc 16f §9.6.3.3 step 4 — pre-fetch HEAD indicates blob
            // exceeds max_blob_fetch_size; rejected without download.
            PdsError::BlobTooLarge { .. } => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "BlobTooLarge",
                self.to_string(),
            ),
            // Arc 16f §9.6.3.1 step 1 — single-flight handler lock
            // contended; try-acquire + fail-fast posture per §9.6.5.8.
            PdsError::ConcurrentMutation => (
                StatusCode::CONFLICT,
                "ConcurrentMutation",
                self.to_string(),
            ),
            // Arc 16f §9.6.3.1 step 3 — CAR body failed structural
            // acceptance gates (DID mismatch, oversize, decode error).
            PdsError::InvalidCar(_) => (
                StatusCode::BAD_REQUEST,
                "InvalidCar",
                self.to_string(),
            ),
            // Arc 16f §9.6.3.5 — internal control-flow signal that
            // SHOULD be consumed by the fetch-and-retry loop and never
            // reach the wire. If it does, surface as 500 defensively so
            // operators see the bug (likely a missing loop iteration in
            // the caller).
            PdsError::NeedsBlobFetch { .. } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError",
                "Internal server error".to_string(),
            ),
            // Arc 17 §17.3.6 wire-format mapping (aligned with Arc 16f
            // §9.6.3.5 OriginFetchExhausted at HTTP 502 per round-1
            // F15 closure).
            PdsError::LexiconFetchFailed { .. } => (
                StatusCode::BAD_GATEWAY,
                "LexiconFetchFailed",
                self.to_string(),
            ),
            PdsError::LexiconAuthorityMismatch { .. } => (
                StatusCode::BAD_GATEWAY,
                "LexiconAuthorityMismatch",
                self.to_string(),
            ),
            PdsError::LexiconAuthorityAmbiguous { .. } => (
                StatusCode::BAD_GATEWAY,
                "LexiconAuthorityAmbiguous",
                self.to_string(),
            ),
            PdsError::LexiconAuthorityTombstoned { .. } => (
                StatusCode::BAD_GATEWAY,
                "LexiconAuthorityTombstoned",
                self.to_string(),
            ),
            PdsError::LexiconInvalidSchema { .. } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "LexiconInvalidSchema",
                self.to_string(),
            ),
            PdsError::LexiconInvalidNsid { .. } => (
                StatusCode::BAD_REQUEST,
                "LexiconInvalidNsid",
                self.to_string(),
            ),
            PdsError::SchemaViolation { .. } => (
                StatusCode::BAD_REQUEST,
                "SchemaViolation",
                self.to_string(),
            ),
            PdsError::NamespaceDenied { .. } => (
                StatusCode::BAD_REQUEST,
                "NamespaceDenied",
                self.to_string(),
            ),
            PdsError::LexiconDisabled => (
                StatusCode::SERVICE_UNAVAILABLE,
                "LexiconDisabled",
                self.to_string(),
            ),
            // v0.7 arc 1 — kryphocron dispatcher errors.
            // Per v07_DESIGN.md §6 / §8: client errors are 400; substrate
            // drift (registry vs lexicon docs out of sync) is 500.
            PdsError::UnsupportedNamespace { .. } => (
                StatusCode::BAD_REQUEST,
                "UnsupportedNamespace",
                self.to_string(),
            ),
            PdsError::KryphocronUnregisteredNsidInClosedNamespace { .. } => (
                StatusCode::BAD_REQUEST,
                "KryphocronUnregisteredNsidInClosedNamespace",
                self.to_string(),
            ),
            PdsError::KryphocronTierClassificationFailed { .. } => (
                StatusCode::BAD_REQUEST,
                "KryphocronTierClassificationFailed",
                self.to_string(),
            ),
            PdsError::TierClassificationFailed { .. } => (
                StatusCode::BAD_REQUEST,
                "TierClassificationFailed",
                self.to_string(),
            ),
            PdsError::KryphocronLexiconMissing { .. } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "KryphocronLexiconMissing",
                self.to_string(),
            ),
            PdsError::KryphocronLexiconNotRecord { .. } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "KryphocronLexiconNotRecord",
                self.to_string(),
            ),
            PdsError::KryphocronLexiconValidationFailed { .. } => (
                StatusCode::BAD_REQUEST,
                "KryphocronLexiconValidationFailed",
                self.to_string(),
            ),
            PdsError::KryphocronRecordRequiresDedicatedEndpoint { .. } => (
                StatusCode::BAD_REQUEST,
                "KryphocronRecordRequiresDedicatedEndpoint",
                self.to_string(),
            ),
            PdsError::KryphocronRecordNotYetSupported { .. } => (
                StatusCode::BAD_REQUEST,
                "KryphocronRecordNotYetSupported",
                self.to_string(),
            ),
            PdsError::KryphocronCascadeTokenInvalid(_) => (
                StatusCode::FORBIDDEN,
                "KryphocronCascadeTokenInvalid",
                self.to_string(),
            ),
            PdsError::KryphocronCascadeWriteRejected(_) => (
                StatusCode::FORBIDDEN,
                "KryphocronCascadeWriteRejected",
                self.to_string(),
            ),
            PdsError::SwapCidMismatch(_) => (
                StatusCode::CONFLICT,
                "SwapCidMismatch",
                self.to_string(),
            ),
            PdsError::KryphocronBindPipelineOutsideScope => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "KryphocronBindPipelineOutsideScope",
                self.to_string(),
            ),
            // v0.9 Arc D (#237a) — codec skew on an authorized decode-on-read.
            // 410 Gone (not 500): the record is valid but undecodable here.
            PdsError::KryphocronCodecUnavailable { .. } => (
                StatusCode::GONE,
                "KryphocronCodecUnavailable",
                self.to_string(),
            ),
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError",
                self.to_string(),
            ),
        };

        // chainlink #104 Fix 2a: centrally log the underlying error
        // for any 5xx mapping. Without this, the client sees the
        // generic "Internal server error" body and tower_http logs
        // only "response failed, Status code: 500" — leaving operators
        // with nothing to diagnose. Handlers can add per-call domain
        // context (cid, auth_did, etc.) on top of this central log.
        if status.is_server_error() {
            tracing::warn!(
                status = status.as_u16(),
                error_code = error_code,
                error = %error_display,
                "PdsError mapped to 5xx response",
            );
        }

        let body = Json(XrpcErrorResponse {
            error: error_code.to_string(),
            message,
            cid: cid_field,
            public_reason: public_reason_field,
            per_cid_failures: per_cid_failures_field,
        });

        (status, body).into_response()
    }
}

/// Result type alias for PDS operations
pub type PdsResult<T> = Result<T, PdsError>;

/// Convert proto-blue's DAG-CBOR encoder error into a PdsError.
/// Used by the `?` operator in firehose-frame encoding (Arc 14 §7.4 Step 1).
impl From<proto_blue::lex_cbor::CborError> for PdsError {
    fn from(e: proto_blue::lex_cbor::CborError) -> Self {
        PdsError::CborEncoding(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    /// Arc 16e §9.5.3.5 R0c.A — `PdsError::BlobNotFound` maps to HTTP
    /// 400 with wire shape `{"error": "BlobNotFound", "message":
    /// "Could not find blob: <cid>"}` matching bsky-PDS verbatim at
    /// `packages/pds/src/actor-store/blob/transactor.ts:259-260`.
    /// Pre-#107 this was HTTP 404; the flip is the load-bearing
    /// behavior change in the cross-arc PR.
    #[tokio::test]
    async fn blob_not_found_maps_to_http_400_with_bsky_pds_wire_shape() {
        let err = PdsError::BlobNotFound("bafkreigh2akiscaildc".to_string());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body: XrpcErrorResponse = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(body.error, "BlobNotFound");
        assert_eq!(body.message, "Could not find blob: bafkreigh2akiscaildc");
    }

    /// Arc 16e §9.5.3.5 + §9.5.5.10 — `PdsError::InvalidCid` maps to
    /// HTTP 400 with typed wire shape `{"error": "InvalidCid",
    /// "message": "Invalid CID in record body: <cid>"}`. Deliberate
    /// stricter-than-bsky-PDS posture per chainlink #106 (bsky-PDS
    /// uses the `InvalidRequest` umbrella).
    #[tokio::test]
    async fn invalid_cid_maps_to_http_400_with_typed_wire_code() {
        let err = PdsError::InvalidCid("bafyrei-malformed-input".to_string());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body: XrpcErrorResponse = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(body.error, "InvalidCid");
        assert_eq!(
            body.message,
            "Invalid CID in record body: bafyrei-malformed-input"
        );
    }

    /// `PdsError::HandleNotFound` (the "resolveHandle completed but the
    /// handle does not resolve to any DID" case) maps to HTTP 400 with
    /// the lexicon-canonical `HandleNotFound` error name, per the
    /// `com.atproto.identity.resolveHandle` ATProto lexicon. Locks in
    /// the distinction from `IdentityResolution` (genuine resolution
    /// failure → 500) — see paired test below.
    #[tokio::test]
    async fn handle_not_found_maps_to_http_400_with_lexicon_error_name() {
        let err = PdsError::HandleNotFound("alice.example.com".to_string());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body: XrpcErrorResponse = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(body.error, "HandleNotFound");
        assert_eq!(body.message, "Handle not found: alice.example.com");
    }

    /// `PdsError::IdentityResolution` (the genuine resolution failure
    /// case — DNS timeout, PLC unreachable, etc.) preserves its HTTP
    /// 500 fall-through mapping. Paired with the `HandleNotFound`
    /// test above to ensure the two cases are NOT collapsed into one
    /// status — a real infrastructure failure is a server error, not
    /// a client error.
    #[tokio::test]
    async fn identity_resolution_failure_preserves_http_500() {
        let err = PdsError::IdentityResolution(
            "Failed to resolve handle: DNS query timed out".to_string(),
        );
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    fn quarantine_test_cid() -> Cid {
        use std::str::FromStr;
        Cid::from_str("bafkreigh2akiscaildcqabsyg3dfr6chu3fgpregiymsck7e7aqa4s52zy")
            .expect("valid CIDv1 raw multibase")
    }

    /// Arc 16f §9.6.3.6 — QuarantinedBlobReferenced wire shape carries
    /// `cid` and `public_reason` as separate JSON fields per design,
    /// not embedded in the message. Phase B Scenario 6 (chainlink
    /// #121, 2026-05-22) surfaced the gap: pre-#126, the IntoResponse
    /// impl discarded both fields via `..` and emitted only `{error,
    /// message}`. Post-#126, the wire shape matches §9.6.3.6 verbatim.
    #[tokio::test]
    async fn quarantined_blob_referenced_wire_shape_carries_cid_and_public_reason() {
        let cid = quarantine_test_cid();
        let err = PdsError::QuarantinedBlobReferenced {
            cid: cid.clone(),
            public_reason: QuarantinePublicReason::Abuse,
        };
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body: XrpcErrorResponse = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(body.error, "QuarantinedBlobReferenced");
        assert_eq!(
            body.cid.as_deref(),
            Some(cid.to_string().as_str()),
            "wire payload must carry the cid as a separate field, \
             not just embedded in message text"
        );
        assert_eq!(
            body.public_reason.as_deref(),
            Some("abuse"),
            "wire payload must carry the coarse QuarantinePublicReason \
             class (Abuse → \"abuse\" via serde rename_all=lowercase)"
        );
    }

    /// Arc 16f §9.6.3.6 round-1 F20 invariant: the wire payload exposes
    /// ONLY the coarse [`QuarantinePublicReason`] class. The operator-
    /// internal `blob_quarantine.reason` text (e.g. "csam", "matched
    /// signature SHA256:abc...") MUST NEVER appear in any wire field.
    ///
    /// The variant payload by construction only carries
    /// [`QuarantinePublicReason`] (an enum), so a wire leak would
    /// require either a future contributor adding the internal-reason
    /// text into the variant OR the variant's Display impl picking up
    /// the internal text somehow. This test sets the public class to
    /// Abuse — which maps from internal reason "csam" via
    /// `from_internal_reason_str` — and asserts the raw JSON bytes
    /// contain neither "csam" nor any other internal-reason marker.
    #[tokio::test]
    async fn quarantined_blob_referenced_does_not_leak_internal_reason() {
        let cid = quarantine_test_cid();
        // Construct via the production code path: `from_internal_reason_str("csam")`
        // returns Abuse — the SAME path validate_phase_blob_check uses.
        let public_reason = QuarantinePublicReason::from_internal_reason_str("csam");
        assert_eq!(public_reason, QuarantinePublicReason::Abuse);

        let err = PdsError::QuarantinedBlobReferenced { cid, public_reason };
        let resp = err.into_response();
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let raw = std::str::from_utf8(&bytes).expect("UTF-8 body");

        // Round-1 F20: no operator-internal reason words anywhere.
        // The check looks at the raw JSON bytes (not just deserialized
        // fields) so a leak in any new field gets caught — including
        // a future contributor accidentally adding the internal reason
        // to a debug-style additional field.
        for forbidden in ["csam", "dmca", "tos", "matched signature"] {
            assert!(
                !raw.to_lowercase().contains(forbidden),
                "wire payload leaked operator-internal reason word `{}`. \
                 Raw body: {}",
                forbidden,
                raw
            );
        }

        // Positive check: the coarse class IS present.
        assert!(
            raw.contains("\"public_reason\":\"abuse\""),
            "wire payload must carry the coarse public class. Raw body: {}",
            raw
        );
    }

    /// Arc 16f §9.6.3.5 — OriginFetchExhausted wire shape carries
    /// `per_cid_failures` as a structured array per design. Phase B
    /// Scenario 6 (chainlink #121) surfaced the parallel gap: pre-#126
    /// the IntoResponse impl discarded the per_cid_failures vec via
    /// `..` and emitted only `{error, message}` (explicit "lands when
    /// Step 3 ships" placeholder in the code). Post-#126 the wire
    /// shape matches §9.6.3.5 verbatim.
    #[tokio::test]
    async fn origin_fetch_exhausted_wire_shape_carries_per_cid_failures() {
        let cid_a = quarantine_test_cid();
        let cid_b = quarantine_test_cid();
        let err = PdsError::OriginFetchExhausted {
            per_cid_failures: vec![
                (cid_a.clone(), "404 Not Found".to_string()),
                (cid_b.clone(), "5xx after 3 retries".to_string()),
            ],
        };
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);

        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body: XrpcErrorResponse = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(body.error, "OriginFetchExhausted");
        let per_cid = body
            .per_cid_failures
            .expect("per_cid_failures must be serialized on the wire");
        assert_eq!(per_cid.len(), 2, "all failure entries land on wire");
        assert_eq!(per_cid[0].cid, cid_a.to_string());
        assert_eq!(per_cid[0].reason, "404 Not Found");
        assert_eq!(per_cid[1].cid, cid_b.to_string());
        assert_eq!(per_cid[1].reason, "5xx after 3 retries");
    }

    /// Backward-compat sanity: variants that don't populate the
    /// optional extension fields must NOT serialize them (no `null`
    /// pollution, no empty `cid: ""` etc.). `serde(skip_serializing_if
    /// = "Option::is_none")` carries this — confirm against the wire
    /// for one representative non-Arc-16f variant.
    #[tokio::test]
    async fn non_extended_variant_omits_optional_fields() {
        let err = PdsError::BlobNotFound("bafkrei...".to_string());
        let resp = err.into_response();
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let raw = std::str::from_utf8(&bytes).expect("UTF-8 body");
        assert!(
            !raw.contains("\"cid\""),
            "BlobNotFound must NOT emit a `cid` field; only Arc 16f \
             structured-error variants do. Raw body: {}",
            raw
        );
        assert!(
            !raw.contains("\"public_reason\""),
            "BlobNotFound must NOT emit a `public_reason` field. \
             Raw body: {}",
            raw
        );
        assert!(
            !raw.contains("\"per_cid_failures\""),
            "BlobNotFound must NOT emit a `per_cid_failures` field. \
             Raw body: {}",
            raw
        );
    }
}
