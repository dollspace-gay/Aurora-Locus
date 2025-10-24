//! Moderation decision logic
//!
//! This module implements the core logic for determining how content should be moderated.

use super::labels::*;
use super::types::*;
use super::ui::*;

/// A single reason why content is being moderated
#[derive(Debug, Clone)]
pub enum ModerationCause {
    /// User is blocking this actor
    Blocking {
        source: ModerationCauseSource,
        priority: ModerationPriority,
        downgraded: bool,
    },

    /// User is blocked by this actor
    BlockedBy {
        source: ModerationCauseSource,
        priority: ModerationPriority,
        downgraded: bool,
    },

    /// Blocking for other reasons
    BlockOther {
        source: ModerationCauseSource,
        priority: ModerationPriority,
        downgraded: bool,
    },

    /// Label-based moderation
    Label {
        source: ModerationCauseSource,
        label_val: String,
        label_def: InterpretedLabelValueDefinition,
        target: LabelTarget,
        setting: LabelPreference,
        behavior: ModerationBehavior,
        no_override: bool,
        priority: ModerationPriority,
        downgraded: bool,
    },

    /// User muted this actor
    Muted {
        source: ModerationCauseSource,
        priority: ModerationPriority,
        downgraded: bool,
    },

    /// Content matches muted words
    MuteWord {
        source: ModerationCauseSource,
        priority: ModerationPriority,
        downgraded: bool,
        matches: Vec<String>,
    },

    /// User hid this content
    Hidden {
        source: ModerationCauseSource,
        priority: ModerationPriority,
        downgraded: bool,
    },
}

impl ModerationCause {
    /// Get the priority of this cause
    pub fn priority(&self) -> ModerationPriority {
        match self {
            Self::Blocking { priority, .. }
            | Self::BlockedBy { priority, .. }
            | Self::BlockOther { priority, .. }
            | Self::Label { priority, .. }
            | Self::Muted { priority, .. }
            | Self::MuteWord { priority, .. }
            | Self::Hidden { priority, .. } => *priority,
        }
    }

    /// Check if this cause is downgraded
    pub fn is_downgraded(&self) -> bool {
        match self {
            Self::Blocking { downgraded, .. }
            | Self::BlockedBy { downgraded, .. }
            | Self::BlockOther { downgraded, .. }
            | Self::Label { downgraded, .. }
            | Self::Muted { downgraded, .. }
            | Self::MuteWord { downgraded, .. }
            | Self::Hidden { downgraded, .. } => *downgraded,
        }
    }

    /// Get the type of this cause as a string
    pub fn cause_type(&self) -> &'static str {
        match self {
            Self::Blocking { .. } => "blocking",
            Self::BlockedBy { .. } => "blocked-by",
            Self::BlockOther { .. } => "block-other",
            Self::Label { .. } => "label",
            Self::Muted { .. } => "muted",
            Self::MuteWord { .. } => "mute-word",
            Self::Hidden { .. } => "hidden",
        }
    }
}

/// Aggregated moderation decision for a piece of content
#[derive(Debug, Clone)]
pub struct ModerationDecision {
    /// DID of the subject being moderated
    pub did: String,

    /// Is this content from the current user?
    pub is_me: bool,

    /// All reasons why this content is being moderated
    pub causes: Vec<ModerationCause>,
}

impl ModerationDecision {
    /// Create a new moderation decision
    pub fn new(did: String, is_me: bool) -> Self {
        Self {
            did,
            is_me,
            causes: Vec::new(),
        }
    }

    /// Add a moderation cause
    pub fn add_cause(&mut self, cause: ModerationCause) {
        self.causes.push(cause);
    }

    /// Check if content is blocked
    pub fn blocked(&self) -> bool {
        self.causes
            .iter()
            .any(|c| matches!(c, ModerationCause::Blocking { .. }))
    }

    /// Check if user is blocked by the content author
    pub fn blocked_by(&self) -> bool {
        self.causes
            .iter()
            .any(|c| matches!(c, ModerationCause::BlockedBy { .. }))
    }

    /// Check if content is muted
    pub fn muted(&self) -> bool {
        self.causes
            .iter()
            .any(|c| matches!(c, ModerationCause::Muted { .. }))
    }

    /// Get the blocking cause if any
    pub fn block_cause(&self) -> Option<&ModerationCause> {
        self.causes
            .iter()
            .find(|c| matches!(c, ModerationCause::Blocking { .. }))
    }

    /// Get the mute cause if any
    pub fn mute_cause(&self) -> Option<&ModerationCause> {
        self.causes
            .iter()
            .find(|c| matches!(c, ModerationCause::Muted { .. }))
    }

    /// Get all label causes
    pub fn label_causes(&self) -> Vec<&ModerationCause> {
        self.causes
            .iter()
            .filter(|c| matches!(c, ModerationCause::Label { .. }))
            .collect()
    }

    /// Generate UI output for a specific context
    pub fn ui(&self, context: &str) -> ModerationUI {
        let mut ui = ModerationUI::new();

        // Sort causes by priority (lowest number = highest priority)
        let mut sorted_causes = self.causes.clone();
        sorted_causes.sort_by_key(|c| c.priority());

        for cause in &sorted_causes {
            match cause {
                ModerationCause::Blocking { .. } => {
                    // Apply block behavior based on context
                    apply_behavior_to_ui(&mut ui, &BLOCK_BEHAVIOR, context, cause);
                }
                ModerationCause::BlockedBy { .. } => {
                    apply_behavior_to_ui(&mut ui, &BLOCK_BEHAVIOR, context, cause);
                }
                ModerationCause::BlockOther { .. } => {
                    apply_behavior_to_ui(&mut ui, &BLOCK_BEHAVIOR, context, cause);
                }
                ModerationCause::Label {
                    behavior,
                    no_override,
                    ..
                } => {
                    apply_behavior_to_ui(&mut ui, behavior, context, cause);
                    if *no_override {
                        ui.no_override = true;
                    }
                }
                ModerationCause::Muted { .. } => {
                    apply_behavior_to_ui(&mut ui, &MUTE_BEHAVIOR, context, cause);
                }
                ModerationCause::MuteWord { .. } => {
                    apply_behavior_to_ui(&mut ui, &MUTEWORD_BEHAVIOR, context, cause);
                }
                ModerationCause::Hidden { .. } => {
                    apply_behavior_to_ui(&mut ui, &HIDE_BEHAVIOR, context, cause);
                }
            }
        }

        ui
    }
}

/// Apply a behavior to the UI for a specific context
fn apply_behavior_to_ui(
    ui: &mut ModerationUI,
    behavior: &ModerationBehavior,
    context: &str,
    cause: &ModerationCause,
) {
    let action = match context {
        "profileList" => behavior.profile_list,
        "profileView" => behavior.profile_view,
        "avatar" => behavior.avatar,
        "banner" => behavior.banner,
        "displayName" => behavior.display_name,
        "contentList" => behavior.content_list,
        "contentView" => behavior.content_view,
        "contentMedia" => behavior.content_media,
        _ => None,
    };

    if let Some(action) = action {
        match action {
            ModerationAction::Blur => ui.add_blur(cause.clone()),
            ModerationAction::Alert => ui.add_alert(cause.clone()),
            ModerationAction::Inform => ui.add_inform(cause.clone()),
        }
    }
}

/// Apply a label to create a moderation cause
pub fn apply_label(
    label_val: &str,
    target: LabelTarget,
    prefs: &ModerationPrefs,
    labeler_did: Option<&str>,
) -> Option<ModerationCause> {
    // Get the label definition
    let label_def = if let Some(known) = KnownLabelValue::parse(label_val) {
        get_label_definition(known)
    } else {
        // Unknown/custom label - skip for now
        // In full implementation, would look up from labeler definitions
        return None;
    };

    // Check if adult content is required and enabled
    if label_def.flags.contains(&LabelFlag::Adult) && !prefs.adult_content_enabled {
        return None;
    }

    // Get user's preference for this label
    let setting = if label_def.configurable {
        // Check labeler-specific preference first
        if let Some(labeler_did) = labeler_did {
            prefs
                .labelers
                .iter()
                .find(|l| l.did == labeler_did)
                .and_then(|l| l.labels.get(label_val).cloned())
                .or_else(|| prefs.labels.get(label_val).cloned())
                .unwrap_or_else(|| label_def.default_setting.clone())
        } else {
            prefs
                .labels
                .get(label_val)
                .cloned()
                .unwrap_or_else(|| label_def.default_setting.clone())
        }
    } else {
        // Not configurable - use default
        label_def.default_setting.clone()
    };

    // If user set to ignore, skip
    if matches!(setting, LabelPreference::Ignore) {
        return None;
    }

    // Get behavior for this target
    let behavior = match target {
        LabelTarget::Account => label_def.behaviors.account.clone(),
        LabelTarget::Profile => label_def.behaviors.profile.clone(),
        LabelTarget::Content => label_def.behaviors.content.clone(),
    }
    .unwrap_or_default();

    // Determine priority based on label characteristics
    let no_override = label_def.flags.contains(&LabelFlag::NoOverride);
    let priority = if no_override {
        PRIORITY_FORCED
    } else if label_def.flags.contains(&LabelFlag::Adult) {
        PRIORITY_ADULT
    } else if matches!(label_def.blurs, LabelBlurs::Content) {
        PRIORITY_HIGH_SEVERITY
    } else if matches!(setting, LabelPreference::Warn) {
        PRIORITY_MED_SEVERITY
    } else {
        PRIORITY_LOW_SEVERITY
    };

    let source = if let Some(did) = labeler_did {
        ModerationCauseSource::Labeler { did: did.to_string() }
    } else {
        ModerationCauseSource::User
    };

    Some(ModerationCause::Label {
        source,
        label_val: label_val.to_string(),
        label_def,
        target,
        setting,
        behavior,
        no_override,
        priority,
        downgraded: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_moderation_decision_new() {
        let decision = ModerationDecision::new("did:plc:test".to_string(), false);
        assert_eq!(decision.did, "did:plc:test");
        assert!(!decision.is_me);
        assert!(decision.causes.is_empty());
    }

    #[test]
    fn test_apply_label_porn_hide() {
        let mut prefs = ModerationPrefs::default();
        prefs.adult_content_enabled = true;
        prefs.labels.insert("porn".to_string(), LabelPreference::Hide);

        let cause = apply_label("porn", LabelTarget::Content, &prefs, None);
        assert!(cause.is_some());

        if let Some(ModerationCause::Label { label_val, setting, priority, .. }) = cause {
            assert_eq!(label_val, "porn");
            assert_eq!(setting, LabelPreference::Hide);
            assert_eq!(priority, PRIORITY_ADULT);
        } else {
            panic!("Expected Label cause");
        }
    }

    #[test]
    fn test_apply_label_adult_content_disabled() {
        let prefs = ModerationPrefs::default(); // adult_content_enabled = false

        let cause = apply_label("porn", LabelTarget::Content, &prefs, None);
        assert!(cause.is_none()); // Should be filtered out
    }

    #[test]
    fn test_apply_label_ignore_preference() {
        let mut prefs = ModerationPrefs::default();
        prefs.labels.insert("nudity".to_string(), LabelPreference::Ignore);

        let cause = apply_label("nudity", LabelTarget::Content, &prefs, None);
        assert!(cause.is_none()); // Should be ignored
    }

    #[test]
    fn test_apply_label_forced_hide() {
        let prefs = ModerationPrefs::default();

        let cause = apply_label("!hide", LabelTarget::Content, &prefs, None);
        assert!(cause.is_some());

        if let Some(ModerationCause::Label { no_override, priority, .. }) = cause {
            assert!(no_override);
            assert_eq!(priority, PRIORITY_FORCED);
        } else {
            panic!("Expected Label cause");
        }
    }

    #[test]
    fn test_moderation_decision_blocked() {
        let mut decision = ModerationDecision::new("did:plc:test".to_string(), false);

        decision.add_cause(ModerationCause::Blocking {
            source: ModerationCauseSource::User,
            priority: PRIORITY_BLOCKING,
            downgraded: false,
        });

        assert!(decision.blocked());
        assert!(!decision.muted());
    }

    #[test]
    fn test_moderation_ui_context() {
        let mut decision = ModerationDecision::new("did:plc:test".to_string(), false);

        let mut prefs = ModerationPrefs::default();
        prefs.adult_content_enabled = true;
        prefs.labels.insert("sexual".to_string(), LabelPreference::Warn);

        if let Some(cause) = apply_label("sexual", LabelTarget::Content, &prefs, None) {
            decision.add_cause(cause);
        }

        let list_ui = decision.ui("contentList");
        let view_ui = decision.ui("contentView");

        // Sexual content should have media blurred
        let media_ui = decision.ui("contentMedia");
        assert!(media_ui.blur());
    }
}
