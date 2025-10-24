//! Generated code for app.bsky.graph.verification
//!
//! Record declaring a verification relationship between two accounts. Verifications are only considered valid by an app if issued by an account the app considers trusted.

use serde::{Deserialize, Serialize};

/// Record declaring a verification relationship between two accounts. Verifications are only considered valid by an app if issued by an account the app considers trusted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verification {
    /// Handle of the subject the verification applies to at the moment of verifying, which might not be the same at the time of viewing. The verification is only valid if the current handle matches the one at the time of verifying.
    pub handle: String,
    /// Date of when the verification was created.
    #[serde(rename = "createdAt")]
    pub created_at: String,
    /// DID of the subject the verification applies to.
    pub subject: crate::types::Did,
    /// Display name of the subject the verification applies to at the moment of verifying, which might not be the same at the time of viewing. The verification is only valid if the current displayName matches the one at the time of verifying.
    #[serde(rename = "displayName")]
    pub display_name: String,
}

