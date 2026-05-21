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

    /// Identity resolution errors
    #[error("Identity resolution error: {0}")]
    IdentityResolution(String),

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
            // Database and Io errors cannot be compared, so we use error message comparison
            (PdsError::Database(a), PdsError::Database(b)) => a.to_string() == b.to_string(),
            (PdsError::Io(a), PdsError::Io(b)) => a.to_string() == b.to_string(),
            _ => false,
        }
    }
}

/// XRPC error response format
#[derive(Debug, Serialize, Deserialize)]
pub struct XrpcErrorResponse {
    pub error: String,
    pub message: String,
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
}
