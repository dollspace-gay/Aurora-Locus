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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "$type")]
pub enum Subject {
    #[serde(rename = "com.atproto.admin.defs#repoRef")]
    Repo {
        did: String,
    },
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

    /// Construct a Subject from the moderation_event/admin_audit_log
    /// flat-column representation (subject_did + subject_uri +
    /// subject_cid). Returns `None` if no subject info is present
    /// (i.e., event has no subject, like a server-level event).
    pub fn from_columns(
        subject_did: Option<&str>,
        subject_uri: Option<&str>,
        subject_cid: Option<&str>,
    ) -> Option<Self> {
        match (subject_did, subject_uri, subject_cid) {
            (Some(did), Some(_uri), Some(cid)) => Some(Subject::Blob {
                did: did.to_string(),
                cid: cid.to_string(),
                record_uri: subject_uri.map(String::from),
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
    #[serde(default)]
    pub limit: Option<u32>,
}

impl PaginationParams {
    pub const DEFAULT_LIMIT: u32 = 50;
    pub const MAX_LIMIT: u32 = 100;

    /// Resolved limit, applying default + cap.
    pub fn effective_limit(&self) -> u32 {
        self.limit
            .unwrap_or(Self::DEFAULT_LIMIT)
            .min(Self::MAX_LIMIT)
            .max(1)
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
        // Blob: did + cid (uri optional, used as record_uri context)
        assert_eq!(
            Subject::from_columns(Some("did:plc:c"), Some("at://x"), Some("baf")),
            Some(Subject::Blob {
                did: "did:plc:c".to_string(),
                cid: "baf".to_string(),
                record_uri: Some("at://x".to_string()),
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
}
