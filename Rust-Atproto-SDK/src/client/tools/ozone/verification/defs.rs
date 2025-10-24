//! Generated type definitions for tools.ozone.verification.defs

use serde::{Deserialize, Serialize};

/// Verification data for the associated subject.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationView {
    /// Verification data for the associated subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "subjectRepo")]
    pub subject_repo: Option<serde_json::Value>,
    /// Verification data for the associated subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "issuerProfile")]
    pub issuer_profile: Option<serde_json::Value>,
    /// Verification data for the associated subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "subjectProfile")]
    pub subject_profile: Option<serde_json::Value>,
    /// The user who revoked this verification.
    /// Verification data for the associated subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "revokedBy")]
    pub revoked_by: Option<crate::types::Did>,
    /// Verification data for the associated subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "issuerRepo")]
    pub issuer_repo: Option<serde_json::Value>,
    /// Handle of the subject the verification applies to at the moment of verifying, which might not be the same at the time of viewing. The verification is only valid if the current handle matches the one at the time of verifying.
    /// Verification data for the associated subject.
    pub handle: String,
    /// The user who issued this verification.
    /// Verification data for the associated subject.
    pub issuer: crate::types::Did,
    /// Timestamp when the verification was created.
    /// Verification data for the associated subject.
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// The subject of the verification.
    /// Verification data for the associated subject.
    pub subject: crate::types::Did,
    /// Describes the reason for revocation, also indicating that the verification is no longer valid.
    /// Verification data for the associated subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "revokeReason")]
    pub revoke_reason: Option<String>,
    /// The AT-URI of the verification record.
    /// Verification data for the associated subject.
    pub uri: crate::syntax::AtUri,
    /// Timestamp when the verification was revoked.
    /// Verification data for the associated subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "revokedAt")]
    pub revoked_at: Option<String>,
    /// Display name of the subject the verification applies to at the moment of verifying, which might not be the same at the time of viewing. The verification is only valid if the current displayName matches the one at the time of verifying.
    /// Verification data for the associated subject.
    #[serde(rename = "displayName")]
    pub display_name: String,
}


