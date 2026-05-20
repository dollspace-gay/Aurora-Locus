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
