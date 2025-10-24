//! Generated type definitions for app.bsky.actor.defs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelersPref {
    pub labelers: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedViewPref {
    /// Hide quote posts in the feed.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hideQuotePosts")]
    pub hide_quote_posts: Option<bool>,
    /// Hide replies in the feed if they are not by followed users.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hideRepliesByUnfollowed")]
    pub hide_replies_by_unfollowed: Option<bool>,
    /// Hide replies in the feed.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hideReplies")]
    pub hide_replies: Option<bool>,
    /// Hide replies in the feed if they do not have this number of likes.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hideRepliesByLikeCount")]
    pub hide_replies_by_like_count: Option<i64>,
    /// The URI of the feed, or an identifier which describes the feed.
    pub feed: String,
    /// Hide reposts in the feed.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hideReposts")]
    pub hide_reposts: Option<bool>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileViewBasic {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub associated: Option<serde_json::Value>,
    pub handle: String,
    pub did: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
}


/// Default post interaction settings for the account. These values should be applied as default values when creating new posts. These refs should mirror the threadgate and postgate records exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostInteractionSettingsPref {
    /// Matches threadgate record. List of rules defining who can reply to this users posts. If value is an empty array, no one can reply. If value is undefined, anyone can reply.
    /// Default post interaction settings for the account. These values should be applied as default values when creating new posts. These refs should mirror the threadgate and postgate records exactly.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "threadgateAllowRules")]
    pub threadgate_allow_rules: Option<serde_json::Value>,
    /// Matches postgate record. List of rules defining who can embed this users posts. If value is an empty array or is undefined, no particular rules apply and anyone can embed.
    /// Default post interaction settings for the account. These values should be applied as default values when creating new posts. These refs should mirror the threadgate and postgate records exactly.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "postgateEmbeddingRules")]
    pub postgate_embedding_rules: Option<serde_json::Value>,
}


/// A grab bag of state that's specific to the bsky.app program. Third-party apps shouldn't use this.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BskyAppStatePref {
    /// An array of tokens which identify nudges (modals, popups, tours, highlight dots) that should be shown to the user.
    /// A grab bag of state that's specific to the bsky.app program. Third-party apps shouldn't use this.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "queuedNudges")]
    pub queued_nudges: Option<serde_json::Value>,
    /// Storage for NUXs the user has encountered.
    /// A grab bag of state that's specific to the bsky.app program. Third-party apps shouldn't use this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nuxs: Option<serde_json::Value>,
    /// A grab bag of state that's specific to the bsky.app program. Third-party apps shouldn't use this.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "activeProgressGuide")]
    pub active_progress_guide: Option<serde_json::Value>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelerPrefItem {
    pub did: crate::types::Did,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutedWordsPref {
    /// A list of words the account owner has muted.
    pub items: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileViewDetailed {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "followersCount")]
    pub followers_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "followsCount")]
    pub follows_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "joinedViaStarterPack")]
    pub joined_via_starter_pack: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "indexedAt")]
    pub indexed_at: Option<String>,
    pub did: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub associated: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "postsCount")]
    pub posts_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "pinnedPost")]
    pub pinned_post: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<serde_json::Value>,
    pub handle: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedFeedsPref {
    pub saved: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "timelineIndex")]
    pub timeline_index: Option<i64>,
    pub pinned: serde_json::Value,
}


/// Preferences for how verified accounts appear in the app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationPrefs {
    /// Hide the blue check badges for verified accounts and trusted verifiers.
    /// Preferences for how verified accounts appear in the app.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "hideBadges")]
    pub hide_badges: Option<bool>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileAssociatedChat {
    #[serde(rename = "allowIncoming")]
    pub allow_incoming: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadViewPref {
    /// Sorting mode for threads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    /// Show followed users at the top of all replies.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "prioritizeFollowedUsers")]
    pub prioritize_followed_users: Option<bool>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalDetailsPref {
    /// The birth date of account owner.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "birthDate")]
    pub birth_date: Option<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedFeed {
    pub id: String,
    pub value: String,
    pub pinned: bool,
    #[serde(rename = "type")]
    pub r#type: String,
}


/// Metadata about the requesting account's relationship with the subject account. Only has meaningful content for authed requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewerState {
    /// Metadata about the requesting account's relationship with the subject account. Only has meaningful content for authed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    /// Metadata about the requesting account's relationship with the subject account. Only has meaningful content for authed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "mutedByList")]
    pub muted_by_list: Option<serde_json::Value>,
    /// Metadata about the requesting account's relationship with the subject account. Only has meaningful content for authed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "blockingByList")]
    pub blocking_by_list: Option<serde_json::Value>,
    /// Metadata about the requesting account's relationship with the subject account. Only has meaningful content for authed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "blockedBy")]
    pub blocked_by: Option<bool>,
    /// Metadata about the requesting account's relationship with the subject account. Only has meaningful content for authed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub following: Option<crate::syntax::AtUri>,
    /// This property is present only in selected cases, as an optimization.
    /// Metadata about the requesting account's relationship with the subject account. Only has meaningful content for authed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "knownFollowers")]
    pub known_followers: Option<serde_json::Value>,
    /// Metadata about the requesting account's relationship with the subject account. Only has meaningful content for authed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocking: Option<crate::syntax::AtUri>,
    /// Metadata about the requesting account's relationship with the subject account. Only has meaningful content for authed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "followedBy")]
    pub followed_by: Option<crate::syntax::AtUri>,
    /// This property is present only in selected cases, as an optimization.
    /// Metadata about the requesting account's relationship with the subject account. Only has meaningful content for authed requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "activitySubscription")]
    pub activity_subscription: Option<serde_json::Value>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedFeedsPrefV2 {
    pub items: serde_json::Value,
}


pub type MutedWordTarget = String;

/// If set, an active progress guide. Once completed, can be set to undefined. Should have unspecced fields tracking progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BskyAppProgressGuide {
    /// If set, an active progress guide. Once completed, can be set to undefined. Should have unspecced fields tracking progress.
    pub guide: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdultContentPref {
    pub enabled: bool,
}


/// A new user experiences (NUX) storage object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nux {
    /// Arbitrary data for the NUX. The structure is defined by the NUX itself. Limited to 300 characters.
    /// A new user experiences (NUX) storage object
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// The date and time at which the NUX will expire and should be considered completed.
    /// A new user experiences (NUX) storage object
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<String>,
    /// A new user experiences (NUX) storage object
    pub id: String,
    /// A new user experiences (NUX) storage object
    pub completed: bool,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileAssociated {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedgens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "starterPacks")]
    pub starter_packs: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labeler: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lists: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "activitySubscription")]
    pub activity_subscription: Option<serde_json::Value>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusView {
    /// True if the status is not expired, false if it is expired. Only present if expiration was set.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isActive")]
    pub is_active: Option<bool>,
    pub record: serde_json::Value,
    /// The date when this status will expire. The application might choose to no longer return the status after expiration.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<String>,
    /// The status for the account.
    pub status: String,
    /// An optional embed associated with the status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed: Option<serde_json::Value>,
}


/// A word that the account owner has muted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutedWord {
    /// The muted word itself.
    /// A word that the account owner has muted.
    pub value: String,
    /// A word that the account owner has muted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The intended targets of the muted word.
    /// A word that the account owner has muted.
    pub targets: serde_json::Value,
    /// Groups of users to apply the muted word to. If undefined, applies to all users.
    /// A word that the account owner has muted.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "actorTarget")]
    pub actor_target: Option<String>,
    /// The date and time at which the muted word will expire and no longer be applied.
    /// A word that the account owner has muted.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileAssociatedActivitySubscription {
    #[serde(rename = "allowSubscriptions")]
    pub allow_subscriptions: String,
}


/// Represents the verification information about the user this object is attached to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationState {
    /// The user's status as a trusted verifier.
    /// Represents the verification information about the user this object is attached to.
    #[serde(rename = "trustedVerifierStatus")]
    pub trusted_verifier_status: String,
    /// The user's status as a verified account.
    /// Represents the verification information about the user this object is attached to.
    #[serde(rename = "verifiedStatus")]
    pub verified_status: String,
    /// All verifications issued by trusted verifiers on behalf of this user. Verifications by untrusted verifiers are not included.
    /// Represents the verification information about the user this object is attached to.
    pub verifications: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterestsPref {
    /// A list of tags which describe the account owner's interests gathered during onboarding.
    pub tags: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<serde_json::Value>,
    pub did: crate::types::Did,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer: Option<serde_json::Value>,
    pub handle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub associated: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "indexedAt")]
    pub indexed_at: Option<String>,
}


/// The subject's followers whom you also follow
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownFollowers {
    /// The subject's followers whom you also follow
    pub count: i64,
    /// The subject's followers whom you also follow
    pub followers: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentLabelPref {
    pub label: String,
    pub visibility: String,
    /// Which labeler does this preference apply to? If undefined, applies globally.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "labelerDid")]
    pub labeler_did: Option<crate::types::Did>,
}


/// An individual verification for an associated subject.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationView {
    /// The user who issued this verification.
    /// An individual verification for an associated subject.
    pub issuer: crate::types::Did,
    /// The AT-URI of the verification record.
    /// An individual verification for an associated subject.
    pub uri: crate::syntax::AtUri,
    /// True if the verification passes validation, otherwise false.
    /// An individual verification for an associated subject.
    #[serde(rename = "isValid")]
    pub is_valid: bool,
    /// Timestamp when the verification was created.
    /// An individual verification for an associated subject.
    #[serde(rename = "createdAt")]
    pub created_at: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HiddenPostsPref {
    /// A list of URIs of posts the account owner has hidden.
    pub items: serde_json::Value,
}


