//! Generated type definitions for com.atproto.repo.strongRef

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Main {
    pub uri: crate::syntax::AtUri,
    pub cid: String,
}


