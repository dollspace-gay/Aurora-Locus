//! Moderation subject helpers
//!
//! This module provides convenience functions for applying moderation decisions
//! to different types of subjects (notifications, feed generators, user lists).
//!
//! These helpers follow the same pattern as the TypeScript SDK's subject helpers,
//! combining account, profile, and content-level moderation decisions.

use super::decision::{ModerationCause, ModerationDecision};
use super::labels::get_label_definition;
use super::types::*;
use crate::client::app::bsky::{
    actor::defs::ProfileViewBasic,
    feed::defs::GeneratorView,
    graph::defs::ListView,
};
use crate::client::com::atproto::label::defs::Label;
use serde_json::Value;

/// Decide moderation for a notification
///
/// Evaluates moderation decisions for a notification based on the author's
/// account, profile, and any labels attached to the notification.
///
/// # Arguments
///
/// * `author` - The author's profile (ProfileViewBasic)
/// * `labels` - Optional labels attached to the notification (as JSON array)
/// * `prefs` - User's moderation preferences
/// * `current_user_did` - Optional DID of the current user
///
/// # Returns
///
/// A `ModerationDecision` containing all applicable moderation causes
pub fn decide_notification(
    author: &ProfileViewBasic,
    labels: Option<&Value>,
    prefs: &ModerationPrefs,
    current_user_did: Option<&str>,
) -> ModerationDecision {
    let author_did = author.did.as_str().to_string();
    let is_me = current_user_did == Some(author_did.as_str());

    let mut decision = ModerationDecision::new(author_did, is_me);

    // Add labels from the notification if present
    if let Some(Value::Array(labels_array)) = labels {
        for label_val in labels_array {
            if let Some(val) = label_val.get("val").and_then(|v| v.as_str()) {
                let src = label_val.get("src").and_then(|v| v.as_str()).unwrap_or("");
                add_label_to_decision(
                    &mut decision,
                    val.to_string(),
                    src,
                    LabelTarget::Content,
                    prefs,
                );
            }
        }
    }

    // Merge decisions from account and profile evaluation
    merge_account_decision(&mut decision, author, prefs);
    merge_profile_decision(&mut decision, author, prefs);

    decision
}

/// Decide moderation for a feed generator
///
/// Evaluates moderation decisions for a feed generator based on the creator's
/// account, profile, and any labels attached to the feed.
///
/// # Arguments
///
/// * `feed_gen` - The feed generator view to evaluate
/// * `prefs` - User's moderation preferences
/// * `current_user_did` - Optional DID of the current user
///
/// # Returns
///
/// A `ModerationDecision` containing all applicable moderation causes
pub fn decide_feed_generator(
    feed_gen: &GeneratorView,
    prefs: &ModerationPrefs,
    current_user_did: Option<&str>,
) -> ModerationDecision {
    let creator_did = feed_gen.creator.did.as_str().to_string();
    let is_me = current_user_did == Some(creator_did.as_str());

    let mut decision = ModerationDecision::new(creator_did, is_me);

    // Add labels from the feed generator if present
    if let Some(Value::Array(labels_array)) = &feed_gen.labels {
        for label_val in labels_array {
            if let Some(val) = label_val.get("val").and_then(|v| v.as_str()) {
                let src = label_val.get("src").and_then(|v| v.as_str()).unwrap_or("");
                add_label_to_decision(
                    &mut decision,
                    val.to_string(),
                    src,
                    LabelTarget::Content,
                    prefs,
                );
            }
        }
    }

    // Merge decisions from account and profile evaluation
    merge_account_decision(&mut decision, &feed_gen.creator, prefs);
    merge_profile_decision(&mut decision, &feed_gen.creator, prefs);

    decision
}

/// Decide moderation for a user list
///
/// Evaluates moderation decisions for a user list based on the creator's
/// account, profile, and any labels attached to the list.
///
/// # Arguments
///
/// * `user_list` - The list view to evaluate
/// * `prefs` - User's moderation preferences
/// * `current_user_did` - Optional DID of the current user
///
/// # Returns
///
/// A `ModerationDecision` containing all applicable moderation causes
pub fn decide_user_list(
    user_list: &ListView,
    prefs: &ModerationPrefs,
    current_user_did: Option<&str>,
) -> ModerationDecision {
    // Get creator DID from the list
    let creator_did = if let Some(Value::Object(creator_obj)) = &user_list.creator {
        creator_obj
            .get("did")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    } else {
        // Fallback: extract from URI
        // Format: at://did:plc:xyz/app.bsky.graph.list/abc
        user_list
            .uri
            .as_str()
            .split('/')
            .nth(2)
            .unwrap_or("")
            .to_string()
    };

    let is_me = current_user_did == Some(creator_did.as_str());
    let mut decision = ModerationDecision::new(creator_did, is_me);

    // Add labels from the list if present
    if let Some(Value::Array(labels_array)) = &user_list.labels {
        for label_val in labels_array {
            if let Some(val) = label_val.get("val").and_then(|v| v.as_str()) {
                let src = label_val.get("src").and_then(|v| v.as_str()).unwrap_or("");
                add_label_to_decision(
                    &mut decision,
                    val.to_string(),
                    src,
                    LabelTarget::Content,
                    prefs,
                );
            }
        }
    }

    // Merge decisions from creator's account and profile if available
    if let Some(Value::Object(creator_obj)) = &user_list.creator {
        // Try to construct a ProfileViewBasic from the JSON
        if let Ok(creator_profile) = serde_json::from_value::<ProfileViewBasic>(user_list.creator.clone().unwrap_or(Value::Null)) {
            merge_account_decision(&mut decision, &creator_profile, prefs);
            merge_profile_decision(&mut decision, &creator_profile, prefs);
        }
    }

    decision
}

/// Helper: Add label-based moderation cause to a decision
fn add_label_to_decision(
    decision: &mut ModerationDecision,
    label_val: String,
    labeler_did: &str,
    target: LabelTarget,
    prefs: &ModerationPrefs,
) {
    // Look up label definition
    let label_def = get_label_definition(&label_val);

    // Determine user preference for this label
    let setting = prefs
        .labels
        .get(&label_val)
        .cloned()
        .unwrap_or(LabelPreference::Warn);

    // Get behavior and priority from label definition
    let behavior = label_def.behaviors.clone();
    let priority = label_def
        .default_setting
        .as_ref()
        .map(|s| match s.as_str() {
            "hide" => PRIORITY_HIGH_SEVERITY,
            "warn" => PRIORITY_MED_SEVERITY,
            _ => PRIORITY_LOW_SEVERITY,
        })
        .unwrap_or(PRIORITY_LOW_SEVERITY);

    decision.add_cause(ModerationCause::Label {
        source: ModerationCauseSource::Labeler {
            did: labeler_did.to_string(),
        },
        label_val,
        label_def,
        target,
        setting,
        behavior,
        no_override: false,
        priority,
        downgraded: false,
    });
}

/// Helper: Merge account-level moderation decisions
fn merge_account_decision(
    decision: &mut ModerationDecision,
    profile: &ProfileViewBasic,
    prefs: &ModerationPrefs,
) {
    // Check viewer state for blocking/muting
    if let Some(Value::Object(viewer)) = &profile.viewer {
        // Check blocking
        if let Some(blocking_val) = viewer.get("blocking") {
            if let Some(blocking_str) = blocking_val.as_str() {
                if !blocking_str.is_empty() {
                    decision.add_cause(ModerationCause::Blocking {
                        source: ModerationCauseSource::User,
                        priority: PRIORITY_BLOCKING,
                        downgraded: false,
                    });
                }
            }
        }

        // Check blocked_by
        if let Some(Value::Bool(true)) = viewer.get("blockedBy") {
            decision.add_cause(ModerationCause::BlockedBy {
                source: ModerationCauseSource::User,
                priority: PRIORITY_BLOCKED_BY,
                downgraded: false,
            });
        }

        // Check muted
        if let Some(Value::Bool(true)) = viewer.get("muted") {
            decision.add_cause(ModerationCause::Muted {
                source: ModerationCauseSource::User,
                priority: PRIORITY_MEDIUM,
                downgraded: false,
            });
        }
    }

    // Add account-level labels
    if let Some(Value::Array(labels_array)) = &profile.labels {
        for label_val in labels_array {
            if let Some(val) = label_val.get("val").and_then(|v| v.as_str()) {
                let src = label_val.get("src").and_then(|v| v.as_str()).unwrap_or("");
                add_label_to_decision(
                    decision,
                    val.to_string(),
                    src,
                    LabelTarget::Account,
                    prefs,
                );
            }
        }
    }
}

/// Helper: Merge profile-level moderation decisions
fn merge_profile_decision(
    decision: &mut ModerationDecision,
    profile: &ProfileViewBasic,
    prefs: &ModerationPrefs,
) {
    // Add profile-level labels
    if let Some(Value::Array(labels_array)) = &profile.labels {
        for label_val in labels_array {
            if let Some(val) = label_val.get("val").and_then(|v| v.as_str()) {
                let src = label_val.get("src").and_then(|v| v.as_str()).unwrap_or("");
                add_label_to_decision(
                    decision,
                    val.to_string(),
                    src,
                    LabelTarget::Profile,
                    prefs,
                );
            }
        }
    }
}
