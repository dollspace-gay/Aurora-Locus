//! User Preferences Management
//!
//! This module handles all user preferences for Bluesky, including:
//! - Feed view preferences (home, timeline filters)
//! - Thread view preferences (sorting, prioritization)
//! - Moderation preferences (adult content, labels, labelers)
//! - Saved feeds management (V1 and V2 with migration)
//! - Muted words
//! - Hidden posts
//! - App state (nudges, NUX, progress guides)
//! - Personal details (birth date)
//! - Interests
//! - Post interaction settings
//! - Verification preferences

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

impl Default for BskyFeedViewPreference {
    fn default() -> Self {
        Self {
            hide_replies: false,
            hide_replies_by_unfollowed: true,
            hide_replies_by_like_count: 0,
            hide_reposts: false,
            hide_quote_posts: false,
        }
    }
}

impl Default for BskyThreadViewPreference {
    fn default() -> Self {
        Self {
            sort: "hotness".to_string(),
            prioritize_followed_users: true,
        }
    }
}

/// Feed view preferences for timeline filtering
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BskyFeedViewPreference {
    /// Hide replies in feed
    pub hide_replies: bool,

    /// Hide replies from users you don't follow
    pub hide_replies_by_unfollowed: bool,

    /// Hide replies with like count below this threshold
    pub hide_replies_by_like_count: i64,

    /// Hide reposts in feed
    pub hide_reposts: bool,

    /// Hide quote posts in feed
    pub hide_quote_posts: bool,
}

/// Thread view preferences for post threading
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BskyThreadViewPreference {
    /// Thread sorting: "hotness", "oldest", "newest", "most-likes", "random"
    pub sort: String,

    /// Prioritize followed users in threads
    pub prioritize_followed_users: bool,
}

/// Interests preference
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct BskyInterestsPreference {
    /// Interest tags
    pub tags: Vec<String>,
}

/// Moderation preferences labeler
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModerationPrefsLabeler {
    /// Labeler DID
    pub did: String,

    /// Label preferences for this labeler
    pub labels: HashMap<String, String>,
}

/// Moderation preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModerationPrefs {
    /// Adult content enabled
    pub adult_content_enabled: bool,

    /// Label preferences (label name -> preference)
    pub labels: HashMap<String, String>,

    /// Configured labelers
    pub labelers: Vec<ModerationPrefsLabeler>,

    /// Muted words
    pub muted_words: Vec<crate::client::app::bsky::actor::defs::MutedWord>,

    /// Hidden post URIs
    pub hidden_posts: Vec<String>,
}

/// App state (Bluesky-specific)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct BskyAppState {
    /// Queued nudges
    pub queued_nudges: Vec<String>,

    /// Active progress guide
    pub active_progress_guide: Option<serde_json::Value>, // BskyAppProgressGuide

    /// NUX (New User Experience) items
    pub nuxs: Vec<serde_json::Value>, // Nux[]
}

/// Deprecated V1 feeds preference
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct LegacyFeedsPreference {
    /// Saved feed URIs
    pub saved: Option<Vec<String>>,

    /// Pinned feed URIs
    pub pinned: Option<Vec<String>>,
}

/// Complete Bluesky preferences structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BskyPreferences {
    /// Deprecated V1 feeds (use saved_feeds instead)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feeds: Option<LegacyFeedsPreference>,

    /// V2 saved feeds
    pub saved_feeds: Vec<crate::client::app::bsky::actor::defs::SavedFeed>,

    /// Feed view preferences per feed
    pub feed_view_prefs: HashMap<String, BskyFeedViewPreference>,

    /// Thread view preferences
    pub thread_view_prefs: BskyThreadViewPreference,

    /// Moderation preferences
    pub moderation_prefs: ModerationPrefs,

    /// Birth date
    pub birth_date: Option<chrono::DateTime<chrono::Utc>>,

    /// Interests
    pub interests: BskyInterestsPreference,

    /// App state
    pub bsky_app_state: BskyAppState,

    /// Post interaction settings
    pub post_interaction_settings: serde_json::Value, // PostInteractionSettingsPref

    /// Verification preferences
    pub verification_prefs: serde_json::Value, // VerificationPrefs
}

impl Default for BskyPreferences {
    fn default() -> Self {
        Self {
            feeds: Some(LegacyFeedsPreference::default()),
            saved_feeds: Vec::new(),
            feed_view_prefs: {
                let mut map = HashMap::new();
                map.insert("home".to_string(), BskyFeedViewPreference::default());
                map
            },
            thread_view_prefs: BskyThreadViewPreference::default(),
            moderation_prefs: ModerationPrefs {
                adult_content_enabled: false,
                labels: HashMap::new(),
                labelers: Vec::new(),
                muted_words: Vec::new(),
                hidden_posts: Vec::new(),
            },
            birth_date: None,
            interests: BskyInterestsPreference::default(),
            bsky_app_state: BskyAppState::default(),
            post_interaction_settings: serde_json::json!({}),
            verification_prefs: serde_json::json!({
                "hideBadges": false
            }),
        }
    }
}

/// Label preference value
pub type LabelPreference = String; // "ignore" | "warn" | "hide"

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feed_view_pref_defaults() {
        let defaults = BskyFeedViewPreference::default();
        assert_eq!(defaults.hide_replies, false);
        assert_eq!(defaults.hide_replies_by_unfollowed, true);
        assert_eq!(defaults.hide_replies_by_like_count, 0);
        assert_eq!(defaults.hide_reposts, false);
        assert_eq!(defaults.hide_quote_posts, false);
    }

    #[test]
    fn test_thread_view_pref_defaults() {
        let defaults = BskyThreadViewPreference::default();
        assert_eq!(defaults.sort, "hotness");
        assert_eq!(defaults.prioritize_followed_users, true);
    }

    #[test]
    fn test_bsky_preferences_default() {
        let prefs = BskyPreferences::default();
        assert!(prefs.saved_feeds.is_empty());
        assert_eq!(prefs.moderation_prefs.adult_content_enabled, false);
        assert!(prefs.moderation_prefs.labelers.is_empty());
        assert!(prefs.feed_view_prefs.contains_key("home"));
    }

    #[test]
    fn test_serialization() {
        let prefs = BskyPreferences::default();
        let json = serde_json::to_string(&prefs).unwrap();
        let deserialized: BskyPreferences = serde_json::from_str(&json).unwrap();
        assert_eq!(prefs.moderation_prefs.adult_content_enabled, deserialized.moderation_prefs.adult_content_enabled);
    }
}
