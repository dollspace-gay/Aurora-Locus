/// Moderation API Endpoints
/// Implements com.atproto.moderation.* endpoints for user-submitted reports
use crate::{admin::reports::ReportReason, auth::AuthContext, context::AppContext};
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use serde::{Deserialize, Serialize};

/// Build moderation API routes
pub fn routes() -> Router<AppContext> {
    Router::new().route(
        "/xrpc/com.atproto.moderation.createReport",
        post(create_report),
    )
}

// ============================================================================
// Request/Response Types
// ============================================================================

/// Subject reference - either a repo (account) or a record
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ReportSubject {
    /// Reference to an account (com.atproto.admin.defs#repoRef)
    Repo(RepoRef),
    /// Reference to a specific record (com.atproto.repo.strongRef)
    StrongRef(StrongRef),
}

/// Repo reference (account)
#[derive(Debug, Clone, Deserialize)]
pub struct RepoRef {
    #[serde(rename = "$type")]
    pub type_field: String,
    pub did: String,
}

/// Strong reference to a record
#[derive(Debug, Clone, Deserialize)]
pub struct StrongRef {
    #[serde(rename = "$type")]
    pub type_field: String,
    pub uri: String,
    pub cid: String,
}

/// Create report request
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateReportRequest {
    /// The reason type for the report (e.g., "com.atproto.moderation.defs#reasonSpam")
    pub reason_type: String,
    /// Additional context about the content and violation
    #[serde(default)]
    pub reason: Option<String>,
    /// The subject being reported (account or record)
    pub subject: ReportSubject,
}

/// Create report response
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateReportResponse {
    pub id: i64,
    pub reason_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub subject: serde_json::Value,
    pub reported_by: String,
    pub created_at: String,
}

// ============================================================================
// Endpoint Handlers
// ============================================================================

/// Create a moderation report
///
/// Allows authenticated users to report content or accounts for moderation review.
/// This is the standard ATProto endpoint for user-submitted reports.
async fn create_report(
    State(ctx): State<AppContext>,
    auth: AuthContext,
    Json(req): Json<CreateReportRequest>,
) -> Result<Json<CreateReportResponse>, (StatusCode, String)> {
    // Parse the reason type from ATProto format to our internal format
    let reason_type =
        parse_reason_type(&req.reason_type).map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    // Extract subject details
    let (subject_did, subject_uri, subject_cid) = match &req.subject {
        ReportSubject::Repo(repo_ref) => {
            // Validate type field
            if repo_ref.type_field != "com.atproto.admin.defs#repoRef" {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Invalid subject type for repo reference".to_string(),
                ));
            }
            (Some(repo_ref.did.clone()), None, None)
        }
        ReportSubject::StrongRef(strong_ref) => {
            // Validate type field
            if strong_ref.type_field != "com.atproto.repo.strongRef" {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Invalid subject type for strong reference".to_string(),
                ));
            }
            // Extract DID from URI (at://did:plc:xxx/collection/rkey)
            let did = extract_did_from_uri(&strong_ref.uri).ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    "Invalid record URI format".to_string(),
                )
            })?;
            (
                Some(did),
                Some(strong_ref.uri.clone()),
                Some(strong_ref.cid.clone()),
            )
        }
    };

    // Submit the report
    let report = ctx
        .report_manager
        .submit_report(
            subject_did.as_deref(),
            subject_uri.as_deref(),
            subject_cid.as_deref(),
            reason_type,
            req.reason.as_deref(),
            &auth.did,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Build response subject
    let subject_json = match &req.subject {
        ReportSubject::Repo(repo_ref) => serde_json::json!({
            "$type": "com.atproto.admin.defs#repoRef",
            "did": repo_ref.did
        }),
        ReportSubject::StrongRef(strong_ref) => serde_json::json!({
            "$type": "com.atproto.repo.strongRef",
            "uri": strong_ref.uri,
            "cid": strong_ref.cid
        }),
    };

    Ok(Json(CreateReportResponse {
        id: report.id,
        reason_type: req.reason_type,
        reason: req.reason,
        subject: subject_json,
        reported_by: auth.did,
        created_at: report.reported_at.to_rfc3339(),
    }))
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Parse ATProto reason type to internal ReportReason
fn parse_reason_type(reason_type: &str) -> Result<ReportReason, String> {
    // Handle full ATProto format (com.atproto.moderation.defs#reasonSpam)
    // and short format (spam, violation, etc.)
    let reason = if reason_type.starts_with("com.atproto.moderation.defs#reason") {
        reason_type
            .strip_prefix("com.atproto.moderation.defs#reason")
            .unwrap_or(reason_type)
            .to_lowercase()
    } else if reason_type.starts_with("tools.ozone.report.defs#reason") {
        // Map Ozone reason types to our internal types
        let ozone_reason = reason_type
            .strip_prefix("tools.ozone.report.defs#reason")
            .unwrap_or(reason_type);
        map_ozone_reason(ozone_reason)
    } else {
        reason_type.to_lowercase()
    };

    match reason.as_str() {
        "spam" => Ok(ReportReason::Spam),
        "violation" => Ok(ReportReason::Violation),
        "misleading" => Ok(ReportReason::Misleading),
        "sexual" => Ok(ReportReason::Sexual),
        "rude" => Ok(ReportReason::Rude),
        "other" | "appeal" => Ok(ReportReason::Other),
        _ => Err(format!("Invalid reason type: {}", reason_type)),
    }
}

/// Map Ozone-specific reason types to our internal types
fn map_ozone_reason(ozone_reason: &str) -> String {
    // Map detailed Ozone reasons to our simpler categories
    match ozone_reason.to_lowercase().as_str() {
        // Spam/Misleading category
        s if s.contains("spam") => "spam".to_string(),
        s if s.contains("misleading")
            || s.contains("impersonation")
            || s.contains("scam")
            || s.contains("bot") =>
        {
            "misleading".to_string()
        }

        // Sexual content category
        s if s.contains("sexual") => "sexual".to_string(),

        // Harassment/Rude category
        s if s.contains("harassment")
            || s.contains("hate")
            || s.contains("doxxing")
            || s.contains("troll") =>
        {
            "rude".to_string()
        }

        // Violation category (violence, child safety, rules)
        s if s.contains("violence")
            || s.contains("child")
            || s.contains("rule")
            || s.contains("selfharm") =>
        {
            "violation".to_string()
        }

        // Default to other
        _ => "other".to_string(),
    }
}

/// Extract DID from an AT URI (at://did:plc:xxx/collection/rkey)
fn extract_did_from_uri(uri: &str) -> Option<String> {
    if let Some(stripped) = uri.strip_prefix("at://") {
        let parts: Vec<&str> = stripped.split('/').collect();
        if !parts.is_empty() && parts[0].starts_with("did:") {
            return Some(parts[0].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_reason_type_full_format() {
        assert!(matches!(
            parse_reason_type("com.atproto.moderation.defs#reasonSpam"),
            Ok(ReportReason::Spam)
        ));
        assert!(matches!(
            parse_reason_type("com.atproto.moderation.defs#reasonViolation"),
            Ok(ReportReason::Violation)
        ));
        assert!(matches!(
            parse_reason_type("com.atproto.moderation.defs#reasonMisleading"),
            Ok(ReportReason::Misleading)
        ));
        assert!(matches!(
            parse_reason_type("com.atproto.moderation.defs#reasonSexual"),
            Ok(ReportReason::Sexual)
        ));
        assert!(matches!(
            parse_reason_type("com.atproto.moderation.defs#reasonRude"),
            Ok(ReportReason::Rude)
        ));
        assert!(matches!(
            parse_reason_type("com.atproto.moderation.defs#reasonOther"),
            Ok(ReportReason::Other)
        ));
        assert!(matches!(
            parse_reason_type("com.atproto.moderation.defs#reasonAppeal"),
            Ok(ReportReason::Other)
        ));
    }

    #[test]
    fn test_parse_reason_type_short_format() {
        assert!(matches!(parse_reason_type("spam"), Ok(ReportReason::Spam)));
        assert!(matches!(
            parse_reason_type("violation"),
            Ok(ReportReason::Violation)
        ));
        assert!(matches!(
            parse_reason_type("Misleading"),
            Ok(ReportReason::Misleading)
        ));
    }

    #[test]
    fn test_parse_reason_type_ozone() {
        // Ozone spam reasons
        assert!(matches!(
            parse_reason_type("tools.ozone.report.defs#reasonMisleadingSpam"),
            Ok(ReportReason::Spam)
        ));

        // Ozone harassment reasons
        assert!(matches!(
            parse_reason_type("tools.ozone.report.defs#reasonHarassmentTargeted"),
            Ok(ReportReason::Rude)
        ));

        // Ozone sexual reasons
        assert!(matches!(
            parse_reason_type("tools.ozone.report.defs#reasonSexualUnlabeled"),
            Ok(ReportReason::Sexual)
        ));
    }

    #[test]
    fn test_parse_reason_type_invalid() {
        assert!(parse_reason_type("invalid_reason").is_err());
    }

    #[test]
    fn test_extract_did_from_uri() {
        assert_eq!(
            extract_did_from_uri("at://did:plc:abc123/app.bsky.feed.post/xyz"),
            Some("did:plc:abc123".to_string())
        );
        assert_eq!(
            extract_did_from_uri("at://did:web:example.com/collection/key"),
            Some("did:web:example.com".to_string())
        );
        assert_eq!(extract_did_from_uri("invalid-uri"), None);
        assert_eq!(
            extract_did_from_uri("at://handle.bsky.social/col/key"),
            None
        );
    }
}
