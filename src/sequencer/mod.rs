/// Sequencer - Event log and firehose system
///
/// Provides globally ordered event stream for federation and synchronization.
/// All repository updates are recorded in a monotonically increasing sequence.

pub mod events;
pub mod sequencer;

pub use events::*;
pub use sequencer::{Sequencer, SequencerConfig};

use crate::error::PdsResult;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Event row from database
#[derive(Debug, Clone)]
pub struct SeqRow {
    pub seq: i64,
    pub did: String,
    pub event_type: String,
    pub event: Vec<u8>,  // CBOR-encoded
    pub invalidated: bool,
    pub sequenced_at: DateTime<Utc>,
}

/// Event type discriminator
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    Commit,
    Identity,
    Account,
}

impl EventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventType::Commit => "commit",
            EventType::Identity => "identity",
            EventType::Account => "account",
        }
    }
}

impl From<String> for EventType {
    fn from(s: String) -> Self {
        match s.as_str() {
            "commit" => EventType::Commit,
            "identity" => EventType::Identity,
            "account" => EventType::Account,
            _ => EventType::Commit, // Default
        }
    }
}

/// Unified event wrapper for the firehose
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "$type")]
pub enum SeqEvent {
    #[serde(rename = "#commit")]
    Commit {
        seq: i64,
        time: String,
        #[serde(flatten)]
        evt: CommitEvent,
    },
    #[serde(rename = "#identity")]
    Identity {
        seq: i64,
        time: String,
        #[serde(flatten)]
        evt: IdentityEvent,
    },
    #[serde(rename = "#account")]
    Account {
        seq: i64,
        time: String,
        #[serde(flatten)]
        evt: AccountEvent,
    },
}
