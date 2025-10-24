//! Generated type definitions for com.atproto.repo.defs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitMeta {
    pub cid: String,
    pub rev: String,
}


