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

/// Subject vocabulary for `com.atproto.moderation.createReport`.
///
/// # Wire-format contract
///
/// Wire format is **untagged** — there is no top-level `$type`
/// discriminator on the enum itself; variants disambiguate
/// structurally via the inner struct's `$type` field. Two
/// variants:
///
/// - `Repo` → `{"$type":"com.atproto.admin.defs#repoRef","did":...}`
/// - `StrongRef` → `{"$type":"com.atproto.repo.strongRef","cid":...,"uri":...}`
///
/// Per `docs/V03_DESIGN.md` §6.3.1: variant stability is committed.
/// New variants are additive only; existing variants do not change
/// shape across releases.
///
/// This is a **separate contract surface** from the canonical
/// Aurora Subject (`crate::admin::defs::Subject`); the two
/// surfaces are intentionally distinct shapes (createReport is
/// untagged-at-the-enum-level, the Aurora surface is internally
/// tagged via serde's `tag = "$type"`). Cross-surface byte
/// comparison is not a meaningful test — the distinct shapes are
/// the contract. Snapshot tests in this module's
/// `#[cfg(test)] mod tests` pin each variant's exact wire shape.
///
/// `Serialize` is derived alongside `Deserialize` so canonical-JSON
/// snapshot tests can emit the wire shape; nothing in production
/// serialises this type (the surface is parse-only on
/// `createReport`), but the derive is harmless and keeps the
/// snapshot test shape symmetric with `Subject`'s.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReportSubject {
    /// Reference to an account (com.atproto.admin.defs#repoRef)
    Repo(RepoRef),
    /// Reference to a specific record (com.atproto.repo.strongRef)
    StrongRef(StrongRef),
}

/// Repo reference (account)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoRef {
    #[serde(rename = "$type")]
    pub type_field: String,
    pub did: String,
}

/// Strong reference to a record
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    // §5.5.4 Phase A: apply the configured default action (full tier
    // only). Best-effort — the report is already persisted, so a
    // default-application failure is logged, not surfaced as a 500.
    if let Err(e) = crate::api::moderation_defaults::apply_report_default(&ctx, &report).await {
        tracing::warn!(
            error = %e,
            report_id = report.id,
            "moderation default-action consumer failed on createReport intake"
        );
    }
    // §5.5.4 Phase B: route the new item to a reviewer (Pipeline A §4),
    // after the §2 default action. Best-effort, full tier only.
    if let Err(e) = crate::api::reviewer_assignment::assign_reviewer_on_intake(&ctx, &report).await {
        tracing::warn!(
            error = %e,
            report_id = report.id,
            "reviewer-assignment consumer failed on createReport intake"
        );
    }
    // §5.5.4 Phase C: Pipeline A report-count auto-label rules. Best-effort.
    if let Err(e) = crate::api::auto_label_rules::evaluate_pipeline_a(&ctx, &report).await {
        tracing::warn!(
            error = %e,
            report_id = report.id,
            "auto-label Pipeline A failed on createReport intake"
        );
    }
    // §5.5.4 Phase D: Pipeline A escalation rules. Best-effort.
    if let Err(e) = crate::api::escalation_rules::evaluate_pipeline_a(&ctx, &report).await {
        tracing::warn!(
            error = %e,
            report_id = report.id,
            "escalation Pipeline A failed on createReport intake"
        );
    }

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

/// Canonical ATProto reason-type prefix. Inputs starting with this
/// prefix go through exact-match handling against our internal
/// ReportReason set. Any other NSID-shaped reason input
/// (`<namespace>#reason<Suffix>`) is routed through the extended
/// vocabulary below.
const ATPROTO_REASON_PREFIX: &str = "com.atproto.moderation.defs#reason";

/// One rule in the extended report-reason vocabulary. Substring
/// matches on the lowercased suffix produced by stripping the
/// `#reason` boundary; the first matching rule supplies the
/// internal-category target string.
struct ExtendedReasonRule {
    /// Substrings to look for (any-match) in the lowercased suffix.
    needles: &'static [&'static str],
    /// Internal-category target. Must be one of the values
    /// `parse_reason_type`'s final match arm accepts: spam,
    /// misleading, sexual, rude, violation, other.
    target: &'static str,
}

/// Extended report-reason vocabulary, expressed as data. External
/// moderation systems define their own NSIDs with reason vocabularies
/// that don't line up one-to-one with the canonical com.atproto
/// reason set; this table smooths registered suffixes into our
/// internal categories. The code in `map_extended_reason` does not
/// privilege any specific external system — it iterates this table
/// linearly and the table is the entire input. Adding a new
/// vocabulary is a data change, not a code change.
const EXTENDED_REASON_VOCABULARY: &[ExtendedReasonRule] = &[
    ExtendedReasonRule { needles: &["spam"], target: "spam" },
    ExtendedReasonRule {
        needles: &["misleading", "impersonation", "scam", "bot"],
        target: "misleading",
    },
    ExtendedReasonRule { needles: &["sexual"], target: "sexual" },
    ExtendedReasonRule {
        needles: &["harassment", "hate", "doxxing", "troll"],
        target: "rude",
    },
    ExtendedReasonRule {
        needles: &["violence", "child", "rule", "selfharm"],
        target: "violation",
    },
];

/// Parse ATProto reason type to internal ReportReason. Three input
/// shapes are accepted:
///
/// 1. Canonical: `com.atproto.moderation.defs#reasonSpam` etc. The
///    suffix after the prefix is lowercased and matched exactly.
/// 2. Extended-vocabulary: any other `<namespace>#reason<Suffix>`
///    NSID-shape. The suffix runs through substring-keyed
///    classification per `EXTENDED_REASON_VOCABULARY`.
/// 3. Short form: bare strings like "spam", "violation".
fn parse_reason_type(reason_type: &str) -> Result<ReportReason, String> {
    let reason = if let Some(suffix) = reason_type.strip_prefix(ATPROTO_REASON_PREFIX) {
        suffix.to_lowercase()
    } else if let Some(suffix) = strip_extended_reason_prefix(reason_type) {
        map_extended_reason(suffix)
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

/// Detect a non-canonical reason-type NSID and return whatever
/// follows the `#reason` boundary. Returns None for the canonical
/// com.atproto prefix (handled by the caller separately) and for
/// inputs that aren't NSID-shaped at all.
fn strip_extended_reason_prefix(s: &str) -> Option<&str> {
    if s.starts_with(ATPROTO_REASON_PREFIX) {
        return None;
    }
    let (_, suffix) = s.split_once("#reason")?;
    if suffix.is_empty() {
        None
    } else {
        Some(suffix)
    }
}

/// Map an extended-vocabulary suffix to an internal category by
/// substring matching against `EXTENDED_REASON_VOCABULARY`. Returns
/// "other" when no rule matches, mirroring the wide catch-all
/// behavior of the canonical "Other" reason.
fn map_extended_reason(suffix: &str) -> String {
    let lower = suffix.to_lowercase();
    for rule in EXTENDED_REASON_VOCABULARY {
        if rule.needles.iter().any(|needle| lower.contains(needle)) {
            return rule.target.to_string();
        }
    }
    "other".to_string()
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

// Arc 2 Step 1 (§6.4.1) — canonical-JSON helper for snapshot
// tests. See the matching declaration in `src/admin/defs.rs` for
// the rationale on top-level placement vs nested-mod placement.
#[cfg(test)]
#[path = "../../tests/common/canonical_json.rs"]
mod canonical_json_helper;

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
    fn test_parse_reason_type_extended_vocabulary() {
        // Any `<namespace>#reason<Suffix>` NSID-shape that isn't the
        // canonical com.atproto prefix flows through the extended
        // vocabulary's substring matcher. Test fixtures use generic
        // namespace placeholders rather than naming any specific
        // external system — the contract is "extended NSIDs get
        // substring-classified," not "this one external system gets
        // privileged handling."

        // Spam-family substrings → Spam.
        assert!(matches!(
            parse_reason_type("external.test.report.defs#reasonMisleadingSpam"),
            Ok(ReportReason::Spam)
        ));

        // Harassment-family substrings → Rude.
        assert!(matches!(
            parse_reason_type("external.test.report.defs#reasonHarassmentTargeted"),
            Ok(ReportReason::Rude)
        ));

        // Sexual-family substrings → Sexual.
        assert!(matches!(
            parse_reason_type("external.test.report.defs#reasonSexualUnlabeled"),
            Ok(ReportReason::Sexual)
        ));

        // Violation-family substrings → Violation.
        assert!(matches!(
            parse_reason_type("external.test.report.defs#reasonRuleViolation"),
            Ok(ReportReason::Violation)
        ));

        // No matching needle → Other (the catch-all rule).
        assert!(matches!(
            parse_reason_type("external.test.report.defs#reasonUnknownThing"),
            Ok(ReportReason::Other)
        ));

        // The canonical com.atproto prefix is NOT routed through the
        // extended path — it goes through exact-match. Confirm the
        // strip_extended_reason_prefix helper short-circuits.
        assert!(matches!(
            parse_reason_type("com.atproto.moderation.defs#reasonSpam"),
            Ok(ReportReason::Spam)
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

    // ====================================================================
    // Arc 2 Step 1 (§6.4.1) — ReportSubject vocabulary contract.
    //
    // The createReport surface is the OTHER Subject contract — a
    // separate, intentionally-distinct shape from the canonical
    // Aurora Subject (crate::admin::defs::Subject). It's untagged
    // at the enum level: variants disambiguate via the inner
    // struct's $type field, not via serde's tag = "$type". Each
    // variant gets a full canonical-JSON snapshot here pinning the
    // exact wire shape per §6.3.1's variant-stability commitment.
    // ====================================================================

    use super::canonical_json_helper::canonical_json;

    #[test]
    fn report_subject_repo_wire_format_snapshot() {
        let subject = ReportSubject::Repo(RepoRef {
            type_field: "com.atproto.admin.defs#repoRef".to_string(),
            did: "did:plc:test1234567890abcdef".to_string(),
        });
        // Untagged at the enum level: serializer emits the inner
        // RepoRef directly; the only $type marker is the inner
        // struct's `type_field`.
        assert_eq!(
            canonical_json(&subject),
            r#"{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:test1234567890abcdef"}"#,
        );
    }

    #[test]
    fn report_subject_strong_ref_wire_format_snapshot() {
        let subject = ReportSubject::StrongRef(StrongRef {
            type_field: "com.atproto.repo.strongRef".to_string(),
            uri: "at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc".to_string(),
            cid: "bafyreidemorecord456".to_string(),
        });
        assert_eq!(
            canonical_json(&subject),
            r#"{"$type":"com.atproto.repo.strongRef","cid":"bafyreidemorecord456","uri":"at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc"}"#,
        );
    }
}
