//! Generated type definitions for com.atproto.identity.defs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityInfo {
    /// The complete DID document for the identity.
    #[serde(rename = "didDoc")]
    pub did_doc: serde_json::Value,
    pub did: crate::types::Did,
    /// The validated handle of the account; or 'handle.invalid' if the handle did not bi-directionally match the DID document.
    pub handle: String,
}


