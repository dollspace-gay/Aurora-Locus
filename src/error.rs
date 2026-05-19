/// Unified error types for Aurora Locus PDS
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

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
    /// 404 + `BlobNotFound` envelope. `#[allow(dead_code)]` while
    /// Arc 16b ships with zero production callers (§9.2.5.1); Arc
    /// 16c removes the annotation when STRICT is wired into the
    /// uploadBlob + record-write paths.
    #[error("Blob not found: CID {0}")]
    #[allow(dead_code)]
    BlobNotFound(String),

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
            // Arc 16b §9.2 / Step 0.3 / Step 3.6: typed error for
            // STRICT helper's "blob not present" path. Spec-compliant
            // envelope per Arc 14 v3.2 §7.6.5 pattern.
            PdsError::BlobNotFound(_) => {
                (StatusCode::NOT_FOUND, "BlobNotFound", self.to_string())
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
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError",
                self.to_string(),
            ),
        };

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
