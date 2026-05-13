//! Shared Aurora admin-extension types (chainlink #100 / Phase 3.3).
//!
//! Mirrors the `tools.aurora.admin.defs` namespace. Types here are
//! used across multiple Phase 3 sub-phases:
//!
//! - `Subject` and `SubjectType` (decision B in design doc §4.1)
//! - `PaginatedResponse` and `PaginationParams` (decision E in §4.3)
//! - `AuroraAdminError` (decision F in §4.4)
//!
//! `ModEvent` (decision A in §4.2) is added by Phase 3.5 (#102)
//! since it's tied to the emitEvent endpoint rather than the
//! moderator-tier reads.

use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Polymorphic moderation subject. Mirrors the three-variant
/// shape from `com.atproto.admin.defs` (#repoRef, strongRef,
/// #repoBlobRef) with `$type`-discriminated wire format for
/// ATProto compatibility.
///
/// Per design doc §4.1: same shape used by every Phase 3 endpoint
/// that takes or returns a subject.
///
/// # Wire-format contract
///
/// Per `docs/V03_DESIGN.md` §6.3.1 (and `docs/AURORA_DESIGN.md`
/// §4.1.1): variant stability is committed. New variants are
/// additive only; existing variants do not change shape across
/// releases. The three variants currently committed:
///
/// - `Repo` → `{"$type":"com.atproto.admin.defs#repoRef","did":...}`
/// - `Record` → `{"$type":"com.atproto.repo.strongRef","cid":...,"uri":...}`
/// - `Blob` → `{"$type":"com.atproto.admin.defs#repoBlobRef","cid":...,"did":...,"record_uri"?:...}`
///
/// Snapshot tests in this module's `#[cfg(test)] mod tests` pin
/// each variant's exact wire shape; the cross-type byte-equality
/// guard at `src/api/admin.rs::tests::subject_blob_and_subject_union_repoblobref_serialize_byte_equal`
/// pins agreement with `SubjectUnion` (the parsing dual on the
/// updateSubjectStatus surface). Either guards a regression
/// individually; together they make a silent wire-shape change
/// impossible.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "$type")]
pub enum Subject {
    #[serde(rename = "com.atproto.admin.defs#repoRef")]
    Repo {
        did: String,
    },
    /// Record subject — strong reference to a single record.
    ///
    /// The `cid` field's interpretation depends on the surface
    /// that produced the value:
    ///
    /// - **Single-subject paths** (e.g., `emitEvent{TakedownRecord}`,
    ///   `emitEvent{ApplyLabel}` on a Record subject, and the
    ///   getAuditTrail wire shape): `cid` is the strong-reference
    ///   CID and identifies a specific record version. Semantics
    ///   are CID-level.
    /// - **`batchTakedownRecords` cascade entries** (per Arc 4
    ///   §8.4.3): `cid` is an empty string by deliberate
    ///   convention, signaling URI-level takedown semantics. The
    ///   URI is the identifying field; the takedown covers all
    ///   versions of the record at that URI. See
    ///   [`crate::api::aurora_admin::BatchRecordsInput`].
    ///
    /// External consumers reading `cascade_subjects` from the
    /// audit chain MUST treat empty-CID `Record` entries as
    /// URI-level references, not as missing data. The empty-CID
    /// convention is pinned by
    /// `batch_takedown_records_produces_uri_level_cascade_with_empty_cids`
    /// in `src/api/aurora_admin.rs`.
    #[serde(rename = "com.atproto.repo.strongRef")]
    Record {
        uri: String,
        cid: String,
    },
    #[serde(rename = "com.atproto.admin.defs#repoBlobRef")]
    Blob {
        did: String,
        cid: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        record_uri: Option<String>,
    },
}

impl Subject {
    /// Convenience accessor — the DID this subject references, or
    /// the repo DID for a Record subject. Used by handlers that
    /// need to fetch repo-level metadata regardless of subject type.
    pub fn primary_did(&self) -> Option<&str> {
        match self {
            Subject::Repo { did } | Subject::Blob { did, .. } => Some(did),
            // Record's URI carries did as the authority component
            // (at://did/...); extracting it here is the natural place.
            Subject::Record { uri, .. } => uri
                .strip_prefix("at://")
                .and_then(|rest| rest.split('/').next()),
        }
    }

    /// Construct a Subject from the moderation_event/audit_chain_entry
    /// flat-column representation (subject_did + subject_uri +
    /// subject_cid). Returns `None` if no subject info is present
    /// (i.e., event has no subject, like a server-level event).
    ///
    /// Disambiguation rules (per CR-1 / chainlink #121):
    /// - `(Some, Some, Some)` → `Blob` with `record_uri` populated.
    /// - `(Some, None, Some)` → `Blob` with `record_uri = None`.
    ///   Covers two cases: (a) callers who legitimately don't know
    ///   the originating record, and (b) legacy chain rows written
    ///   before the producer preserved `record_uri` (pre-CR-1).
    /// - `(None, Some, Some)` → `Record`. The absence of `subject_did`
    ///   is what distinguishes Record from a Blob with `record_uri`;
    ///   Record's URI carries the DID as the authority component.
    /// - `(Some, None, None)` → `Repo`.
    /// - Any other shape → `None` (no subject, or invalid combination).
    pub fn from_columns(
        subject_did: Option<&str>,
        subject_uri: Option<&str>,
        subject_cid: Option<&str>,
    ) -> Option<Self> {
        match (subject_did, subject_uri, subject_cid) {
            (Some(did), Some(uri), Some(cid)) => Some(Subject::Blob {
                did: did.to_string(),
                cid: cid.to_string(),
                record_uri: Some(uri.to_string()),
            }),
            (Some(did), None, Some(cid)) => Some(Subject::Blob {
                did: did.to_string(),
                cid: cid.to_string(),
                record_uri: None,
            }),
            (None, Some(uri), Some(cid)) => Some(Subject::Record {
                uri: uri.to_string(),
                cid: cid.to_string(),
            }),
            (Some(did), None, None) => Some(Subject::Repo {
                did: did.to_string(),
            }),
            _ => None,
        }
    }
}

/// Filter-parameter form of [`Subject`]. Used by query endpoints
/// where the caller wants to narrow by subject category without
/// providing a full subject identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubjectType {
    Account,
    Record,
    Blob,
}

/// Standard paginated response wrapper. Reused across every Phase 3
/// list endpoint per design doc §4.3.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    /// Present only when more pages remain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Standard pagination parameters. `limit` defaults to 50, capped at
/// 100 to bound per-request work.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PaginationParams {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_u32_lenient")]
    pub limit: Option<u32>,
}

/// Accept either a JSON number or a string-encoded integer for an
/// `Option<u32>` query parameter. This is needed because axum's
/// `Query<T>` extractor delegates to `serde_urlencoded`, whose
/// string-to-number coercion happens at the format level and is
/// bypassed when a struct uses `#[serde(flatten)]` (the inner struct
/// is fed through serde's internal `Content` buffer, which keeps the
/// raw string form). Without this helper, `?limit=25` on a flattened
/// `PaginationParams` fails with "invalid type: string \"25\",
/// expected u32".
///
/// Behavior:
/// - missing key → `None`
/// - JSON `null` / unit → `None`
/// - integer (any signed/unsigned form that fits) → `Some(n)`
/// - string parseable as u32 → `Some(n)`
/// - anything else → deserialization error with the original value
pub fn deserialize_optional_u32_lenient<'de, D>(d: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{Error, Visitor};
    use std::fmt;

    struct LenientU32;

    impl<'de> Visitor<'de> for LenientU32 {
        type Value = Option<u32>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an optional u32 (number or string-encoded integer)")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D2>(self, d: D2) -> Result<Self::Value, D2::Error>
        where
            D2: serde::Deserializer<'de>,
        {
            d.deserialize_any(LenientU32)
        }

        fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
            v.parse::<u32>()
                .map(Some)
                .map_err(|e| E::custom(format!("invalid u32 \"{}\": {}", v, e)))
        }

        fn visit_string<E: Error>(self, v: String) -> Result<Self::Value, E> {
            self.visit_str(&v)
        }

        fn visit_u8<E>(self, v: u8) -> Result<Self::Value, E> {
            Ok(Some(v as u32))
        }

        fn visit_u16<E>(self, v: u16) -> Result<Self::Value, E> {
            Ok(Some(v as u32))
        }

        fn visit_u32<E>(self, v: u32) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_u64<E: Error>(self, v: u64) -> Result<Self::Value, E> {
            u32::try_from(v)
                .map(Some)
                .map_err(|_| E::custom(format!("u32 overflow: {}", v)))
        }

        fn visit_i32<E: Error>(self, v: i32) -> Result<Self::Value, E> {
            u32::try_from(v)
                .map(Some)
                .map_err(|_| E::custom(format!("u32 negative or overflow: {}", v)))
        }

        fn visit_i64<E: Error>(self, v: i64) -> Result<Self::Value, E> {
            u32::try_from(v)
                .map(Some)
                .map_err(|_| E::custom(format!("u32 negative or overflow: {}", v)))
        }
    }

    d.deserialize_option(LenientU32)
}

impl PaginationParams {
    pub const DEFAULT_LIMIT: u32 = 50;
    pub const MAX_LIMIT: u32 = 100;

    /// Resolved limit, applying default + cap.
    pub fn effective_limit(&self) -> u32 {
        self.limit
            .unwrap_or(Self::DEFAULT_LIMIT)
            .clamp(1, Self::MAX_LIMIT)
    }

    /// Decode the opaque cursor into its internal representation.
    /// Returns `None` if no cursor was provided; returns `Err` if
    /// the cursor was provided but malformed (caller should map to
    /// `AuroraAdminError::OutdatedCursor`).
    pub fn decode_cursor(&self) -> Result<Option<CursorPosition>, &'static str> {
        let Some(s) = &self.cursor else { return Ok(None) };
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(s.as_bytes())
            .map_err(|_| "invalid base64")?;
        let pos: CursorPosition =
            serde_json::from_slice(&raw).map_err(|_| "invalid JSON")?;
        Ok(Some(pos))
    }
}

/// Internal cursor representation. Composite (timestamp + id) so
/// pages don't drift when multiple items share a `created_at`
/// (common during bulk operations).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorPosition {
    pub after_created: DateTime<Utc>,
    pub after_id: i64,
}

impl CursorPosition {
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).expect("CursorPosition serialize");
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    }
}

/// Aurora admin-extension error vocabulary (decision F). Wire
/// format: ATProto error envelope `{"error": "<CodeName>",
/// "message": "<optional>"}`.
///
/// Per-endpoint error sets can extend with endpoint-specific
/// variants but should reuse the shared vocabulary where it fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // not all variants used by 3.3; future sub-phases consume the rest
pub enum AuroraAdminError {
    SubjectNotFound,
    InvalidEvent,
    PermissionDenied,
    OutdatedCursor,
    UnknownEventVariant,
    AppealNotFound,
    BatchValidationError,
}

impl AuroraAdminError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::SubjectNotFound => "SubjectNotFound",
            Self::InvalidEvent => "InvalidEvent",
            Self::PermissionDenied => "PermissionDenied",
            Self::OutdatedCursor => "OutdatedCursor",
            Self::UnknownEventVariant => "UnknownEventVariant",
            Self::AppealNotFound => "AppealNotFound",
            Self::BatchValidationError => "BatchValidationError",
        }
    }

    pub fn http_status(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        match self {
            Self::SubjectNotFound | Self::AppealNotFound => StatusCode::NOT_FOUND,
            Self::PermissionDenied => StatusCode::FORBIDDEN,
            Self::OutdatedCursor
            | Self::InvalidEvent
            | Self::UnknownEventVariant
            | Self::BatchValidationError => StatusCode::BAD_REQUEST,
        }
    }
}

/// Convert to ATProto error envelope tuple for axum response.
impl From<AuroraAdminError> for (axum::http::StatusCode, axum::Json<serde_json::Value>) {
    fn from(e: AuroraAdminError) -> Self {
        (
            e.http_status(),
            axum::Json(serde_json::json!({
                "error": e.code(),
                "message": null,
            })),
        )
    }
}

// Arc 2 Step 1 (§6.4.1) — canonical-JSON helper for snapshot
// tests. Lives at top-level (not inside `mod tests`) because
// `#[path]` resolution from inside a nested inline module
// produces a virtual path that the filesystem can't traverse on
// Linux (the `mod tests/` segment doesn't physically exist).
// Top-level `#[path]` resolves relative to the directory holding
// this .rs file (`src/admin/`), which is real on disk.
#[cfg(test)]
#[path = "../../tests/common/canonical_json.rs"]
mod canonical_json_helper;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_repo_serializes_with_type_discriminator() {
        let s = Subject::Repo {
            did: "did:plc:abc".to_string(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"$type\":\"com.atproto.admin.defs#repoRef\""));
        assert!(json.contains("\"did\":\"did:plc:abc\""));
    }

    #[test]
    fn subject_record_round_trips() {
        let s = Subject::Record {
            uri: "at://did:plc:author/app.bsky.feed.post/abc".to_string(),
            cid: "bafkreitest".to_string(),
        };
        let json = serde_json::to_string(&s).unwrap();
        let parsed: Subject = serde_json::from_str(&json).unwrap();
        assert_eq!(s, parsed);
    }

    #[test]
    fn subject_primary_did_extracts_from_record_uri() {
        let s = Subject::Record {
            uri: "at://did:plc:writer/app.bsky.feed.post/abc".to_string(),
            cid: "bafkreitest".to_string(),
        };
        assert_eq!(s.primary_did(), Some("did:plc:writer"));
    }

    #[test]
    fn subject_from_columns_dispatches_correctly() {
        // Repo: did only
        assert_eq!(
            Subject::from_columns(Some("did:plc:a"), None, None),
            Some(Subject::Repo {
                did: "did:plc:a".to_string()
            })
        );
        // Record: uri + cid (no did column)
        assert_eq!(
            Subject::from_columns(None, Some("at://did:plc:b/c/d"), Some("baf")),
            Some(Subject::Record {
                uri: "at://did:plc:b/c/d".to_string(),
                cid: "baf".to_string(),
            })
        );
        // Blob with record_uri: did + uri + cid all populated
        assert_eq!(
            Subject::from_columns(Some("did:plc:c"), Some("at://x"), Some("baf")),
            Some(Subject::Blob {
                did: "did:plc:c".to_string(),
                cid: "baf".to_string(),
                record_uri: Some("at://x".to_string()),
            })
        );
        // Blob without record_uri (and legacy pre-CR-1 chain rows):
        // did + cid populated, uri NULL → still a Blob, record_uri = None.
        // Without this arm the (Some, None, Some) shape would fall
        // through to the catch-all and return None, losing the subject
        // identity for any chain row written before record_uri was
        // preserved through the producer (chainlink #121).
        assert_eq!(
            Subject::from_columns(Some("did:plc:legacy"), None, Some("bafkreilegacy")),
            Some(Subject::Blob {
                did: "did:plc:legacy".to_string(),
                cid: "bafkreilegacy".to_string(),
                record_uri: None,
            })
        );
        // No subject info at all → None
        assert_eq!(Subject::from_columns(None, None, None), None);
    }

    #[test]
    fn pagination_effective_limit_applies_default_and_cap() {
        assert_eq!(
            PaginationParams::default().effective_limit(),
            PaginationParams::DEFAULT_LIMIT
        );
        assert_eq!(
            PaginationParams {
                limit: Some(200),
                ..Default::default()
            }
            .effective_limit(),
            PaginationParams::MAX_LIMIT
        );
        assert_eq!(
            PaginationParams {
                limit: Some(0),
                ..Default::default()
            }
            .effective_limit(),
            1
        );
        assert_eq!(
            PaginationParams {
                limit: Some(25),
                ..Default::default()
            }
            .effective_limit(),
            25
        );
    }

    #[test]
    fn cursor_round_trip() {
        let pos = CursorPosition {
            after_created: chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            after_id: 42,
        };
        let encoded = pos.encode();
        let params = PaginationParams {
            cursor: Some(encoded),
            limit: None,
        };
        let decoded = params.decode_cursor().unwrap().unwrap();
        assert_eq!(decoded.after_id, 42);
    }

    #[test]
    fn cursor_decode_rejects_garbage() {
        let params = PaginationParams {
            cursor: Some("not-base64-!@#".to_string()),
            limit: None,
        };
        assert!(params.decode_cursor().is_err());
    }

    #[test]
    fn aurora_admin_error_codes_are_stable() {
        assert_eq!(AuroraAdminError::SubjectNotFound.code(), "SubjectNotFound");
        assert_eq!(AuroraAdminError::OutdatedCursor.code(), "OutdatedCursor");
    }

    #[test]
    fn aurora_admin_error_http_status_mapping() {
        use axum::http::StatusCode;
        assert_eq!(
            AuroraAdminError::SubjectNotFound.http_status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            AuroraAdminError::PermissionDenied.http_status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            AuroraAdminError::OutdatedCursor.http_status(),
            StatusCode::BAD_REQUEST
        );
    }

    // ---------------------------------------------------------------
    // Lenient u32 deserialization for query-string limits.
    //
    // Reproduces the original symptom and verifies the helper covers
    // both bare-struct and `#[serde(flatten)]` cases — the latter is
    // where axum's default Query extractor breaks because flatten
    // sends every value through serde's internal Content buffer as a
    // string.
    // ---------------------------------------------------------------

    #[derive(Debug, Deserialize)]
    struct Outer {
        #[serde(flatten)]
        pagination: PaginationParams,
    }

    #[test]
    fn pagination_limit_accepts_string_form_via_query_string() {
        // The original bug: `?limit=25` over a flattened pagination
        // struct fails before this helper because the inner u32 sees a
        // borrowed "25" string. With the lenient helper this round-
        // trips correctly.
        let parsed: Outer = serde_urlencoded::from_str("limit=25").expect("limit=25 parses");
        assert_eq!(parsed.pagination.limit, Some(25));
    }

    #[test]
    fn pagination_limit_accepts_explicit_zero_string() {
        let parsed: Outer = serde_urlencoded::from_str("limit=0").expect("limit=0 parses");
        assert_eq!(parsed.pagination.limit, Some(0));
    }

    #[test]
    fn pagination_limit_missing_yields_none() {
        let parsed: Outer = serde_urlencoded::from_str("").expect("empty parses");
        assert_eq!(parsed.pagination.limit, None);
        assert_eq!(parsed.pagination.cursor, None);
    }

    #[test]
    fn pagination_limit_rejects_non_integer_string() {
        let result: Result<Outer, _> = serde_urlencoded::from_str("limit=abc");
        assert!(result.is_err(), "non-integer limit must error");
    }

    #[test]
    fn pagination_limit_rejects_negative_string() {
        let result: Result<Outer, _> = serde_urlencoded::from_str("limit=-5");
        assert!(result.is_err(), "negative limit must error");
    }

    #[test]
    fn pagination_limit_accepts_native_json_number() {
        // Direct deserialization (not over a query string) — the
        // helper must still accept native u32/u64 values so it does
        // not regress the JSON-body case.
        let parsed: PaginationParams =
            serde_json::from_str(r#"{"limit": 25}"#).expect("native int parses");
        assert_eq!(parsed.limit, Some(25));
    }

    #[test]
    fn pagination_limit_accepts_json_string_form() {
        let parsed: PaginationParams =
            serde_json::from_str(r#"{"limit": "25"}"#).expect("string int parses");
        assert_eq!(parsed.limit, Some(25));
    }

    #[test]
    fn pagination_limit_rejects_json_overflow() {
        let too_big = (u32::MAX as u64) + 1;
        let json = format!(r#"{{"limit": {}}}"#, too_big);
        let result: Result<PaginationParams, _> = serde_json::from_str(&json);
        assert!(result.is_err(), "u32 overflow must error");
    }

    #[test]
    fn pagination_limit_with_cursor_round_trips_via_query_string() {
        // The three impacted endpoints (queryEvents, getAuditTrail,
        // listAppeals) all flatten PaginationParams alongside their
        // own filter fields. Ensure the cursor + limit combination
        // works from a real query string.
        let parsed: Outer =
            serde_urlencoded::from_str("limit=25&cursor=abc").expect("combined parses");
        assert_eq!(parsed.pagination.limit, Some(25));
        assert_eq!(parsed.pagination.cursor.as_deref(), Some("abc"));
    }

    // ====================================================================
    // Arc 2 Step 1 (§6.4.1) — Subject vocabulary contract snapshots.
    //
    // Each variant gets a full canonical-JSON snapshot pinning the
    // exact wire shape. The strings here are the contract — changing
    // any of them breaks shipped clients. New variants are additive
    // only per §6.3.1's variant-stability commitment.
    //
    // Cross-type byte-equality with `SubjectUnion` (the parsing dual
    // on updateSubjectStatus) is pinned separately at
    // `src/api/admin.rs::tests::subject_blob_and_subject_union_repoblobref_serialize_byte_equal`
    // — that test must live in `admin.rs` because `SubjectUnion` is
    // private to that module. Here we pin the absolute wire shape;
    // there we pin agreement.
    // ====================================================================

    use super::canonical_json_helper::canonical_json;

    #[test]
    fn subject_repo_wire_format_snapshot() {
        let subject = Subject::Repo {
            did: "did:plc:test1234567890abcdef".to_string(),
        };
        assert_eq!(
            canonical_json(&subject),
            r#"{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:test1234567890abcdef"}"#,
        );
    }

    #[test]
    fn subject_record_wire_format_snapshot() {
        let subject = Subject::Record {
            uri: "at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc".to_string(),
            cid: "bafyreidemorecord456".to_string(),
        };
        assert_eq!(
            canonical_json(&subject),
            r#"{"$type":"com.atproto.repo.strongRef","cid":"bafyreidemorecord456","uri":"at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc"}"#,
        );
    }

    #[test]
    fn subject_blob_wire_format_snapshot() {
        let subject = Subject::Blob {
            did: "did:plc:test1234567890abcdef".to_string(),
            cid: "bafyreidemoblob456".to_string(),
            record_uri: Some(
                "at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc".to_string(),
            ),
        };
        assert_eq!(
            canonical_json(&subject),
            r#"{"$type":"com.atproto.admin.defs#repoBlobRef","cid":"bafyreidemoblob456","did":"did:plc:test1234567890abcdef","record_uri":"at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc"}"#,
        );
    }

    #[test]
    fn subject_blob_wire_format_snapshot_record_uri_omitted() {
        let subject = Subject::Blob {
            did: "did:plc:test1234567890abcdef".to_string(),
            cid: "bafyreidemoblob456".to_string(),
            record_uri: None,
        };
        // skip_serializing_if drops `record_uri` entirely; the wire
        // shape collapses to {$type, cid, did}.
        assert_eq!(
            canonical_json(&subject),
            r#"{"$type":"com.atproto.admin.defs#repoBlobRef","cid":"bafyreidemoblob456","did":"did:plc:test1234567890abcdef"}"#,
        );
    }
}
