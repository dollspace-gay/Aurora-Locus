//! Core types for the moderation system

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Label preference - how the user wants to handle a specific label
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LabelPreference {
    /// Don't apply this label's moderation
    Ignore,
    /// Show a warning before displaying content
    Warn,
    /// Hide the content completely
    Hide,
}

impl std::str::FromStr for LabelPreference {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ignore" => Ok(Self::Ignore),
            "warn" => Ok(Self::Warn),
            "hide" => Ok(Self::Hide),
            _ => Err(format!("Invalid label preference: {}", s)),
        }
    }
}

impl LabelPreference {
    /// Convert to string
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ignore => "ignore",
            Self::Warn => "warn",
            Self::Hide => "hide",
        }
    }
}

/// What the label is applied to
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LabelTarget {
    /// Applied to the user's account
    Account,
    /// Applied to the user's profile
    Profile,
    /// Applied to specific content (post, image, etc.)
    Content,
}

/// Flags that modify label behavior
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LabelFlag {
    /// Label cannot be overridden by user preferences
    NoOverride,
    /// Label requires adult content to be enabled
    Adult,
    /// Label applies to unauthenticated users
    Unauthed,
    /// Label cannot be self-applied
    NoSelf,
}

/// Severity level of a label
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LabelSeverity {
    /// No severity (informational)
    None,
    /// Informational badge
    Inform,
    /// Alert/warning shown to user
    Alert,
}

/// What content to blur for a label
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LabelBlurs {
    /// Don't blur anything
    None,
    /// Blur the entire content
    Content,
    /// Blur only media (images/videos)
    Media,
}

/// UI behavior for a specific context
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModerationBehavior {
    /// Behavior when shown in profile lists
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_list: Option<ModerationAction>,

    /// Behavior when viewing full profile
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_view: Option<ModerationAction>,

    /// Behavior for avatar images
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<ModerationAction>,

    /// Behavior for banner images
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banner: Option<ModerationAction>,

    /// Behavior for display names
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<ModerationAction>,

    /// Behavior when shown in content lists (feeds)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_list: Option<ModerationAction>,

    /// Behavior when viewing full content
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_view: Option<ModerationAction>,

    /// Behavior for media content
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_media: Option<ModerationAction>,
}

/// What action to take in the UI
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModerationAction {
    /// Show blurred/hidden with reveal option
    Blur,
    /// Show alert/warning badge
    Alert,
    /// Show informational badge
    Inform,
}

/// Pre-defined behavior for blocking
pub const BLOCK_BEHAVIOR: ModerationBehavior = ModerationBehavior {
    profile_list: Some(ModerationAction::Blur),
    profile_view: Some(ModerationAction::Alert),
    avatar: Some(ModerationAction::Blur),
    banner: Some(ModerationAction::Blur),
    display_name: None,
    content_list: Some(ModerationAction::Blur),
    content_view: Some(ModerationAction::Blur),
    content_media: None,
};

/// Pre-defined behavior for muting
pub const MUTE_BEHAVIOR: ModerationBehavior = ModerationBehavior {
    profile_list: Some(ModerationAction::Inform),
    profile_view: Some(ModerationAction::Alert),
    avatar: None,
    banner: None,
    display_name: None,
    content_list: Some(ModerationAction::Blur),
    content_view: Some(ModerationAction::Inform),
    content_media: None,
};

/// Pre-defined behavior for muted words
pub const MUTEWORD_BEHAVIOR: ModerationBehavior = ModerationBehavior {
    profile_list: None,
    profile_view: None,
    avatar: None,
    banner: None,
    display_name: None,
    content_list: Some(ModerationAction::Blur),
    content_view: Some(ModerationAction::Blur),
    content_media: None,
};

/// Pre-defined behavior for hidden content
pub const HIDE_BEHAVIOR: ModerationBehavior = ModerationBehavior {
    profile_list: None,
    profile_view: None,
    avatar: None,
    banner: None,
    display_name: None,
    content_list: Some(ModerationAction::Blur),
    content_view: Some(ModerationAction::Blur),
    content_media: None,
};

/// Moderation preferences for a user
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModerationPrefs {
    /// Whether adult content is enabled
    pub adult_content_enabled: bool,

    /// Global label preferences (label value -> preference)
    pub labels: HashMap<String, LabelPreference>,

    /// Per-labeler preferences
    pub labelers: Vec<ModerationPrefsLabeler>,

    /// Muted words/phrases
    pub muted_words: Vec<MutedWord>,

    /// Hidden post URIs
    pub hidden_posts: Vec<String>,
}


/// Per-labeler preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModerationPrefsLabeler {
    /// DID of the labeler
    pub did: String,

    /// Label preferences specific to this labeler (overrides global)
    pub labels: HashMap<String, LabelPreference>,
}

/// A muted word or phrase
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutedWord {
    /// The word or phrase to mute
    pub value: String,

    /// Targets where this mute applies
    pub targets: Vec<MutedWordTarget>,

    /// Actor filters (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_target: Option<MutedWordActorTarget>,

    /// Expiration date (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// Where a muted word applies
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MutedWordTarget {
    /// Apply to post content
    Content,
    /// Apply to tags
    Tag,
}

/// Actor-based mute word filtering
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MutedWordActorTarget {
    /// Exclude actors the user is following
    ExcludeFollowing,
    /// Only include specific actors
    All,
}

/// Source of a moderation action
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ModerationCauseSource {
    /// User-initiated action (blocking, muting, etc.)
    User,

    /// From a moderation list
    List {
        /// The list that caused this moderation
        list: serde_json::Value, // AppBskyGraphDefs.ListViewBasic
    },

    /// From a labeler service
    Labeler {
        /// DID of the labeler
        did: String,
    },
}

/// Priority levels for moderation causes (1 = highest)
pub type ModerationPriority = u8;

/// Constant priorities
pub const PRIORITY_FORCED: ModerationPriority = 1;
pub const PRIORITY_ADULT: ModerationPriority = 2;
pub const PRIORITY_BLOCKING: ModerationPriority = 3;
pub const PRIORITY_BLOCKED_BY: ModerationPriority = 4;
pub const PRIORITY_HIGH_SEVERITY: ModerationPriority = 5;
pub const PRIORITY_MEDIUM: ModerationPriority = 6;
pub const PRIORITY_MED_SEVERITY: ModerationPriority = 7;
pub const PRIORITY_LOW_SEVERITY: ModerationPriority = 8;
