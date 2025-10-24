//! Label definitions and interpretation
//!
//! This module defines all known ATProto labels and their behaviors.

use super::types::*;
use std::collections::HashMap;

/// Known label values in the ATProto system
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KnownLabelValue {
    /// Force hide content (cannot be overridden)
    Hide,
    /// Force warn on content
    Warn,
    /// Hide from unauthenticated users
    NoUnauthenticated,
    /// Pornographic content
    Porn,
    /// Sexual content
    Sexual,
    /// Nudity
    Nudity,
    /// Graphic media (violence/gore)
    GraphicMedia,
    /// Gore (deprecated alias for GraphicMedia)
    Gore,
}

impl KnownLabelValue {
    /// Get the string identifier for this label
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hide => "!hide",
            Self::Warn => "!warn",
            Self::NoUnauthenticated => "!no-unauthenticated",
            Self::Porn => "porn",
            Self::Sexual => "sexual",
            Self::Nudity => "nudity",
            Self::GraphicMedia => "graphic-media",
            Self::Gore => "gore",
        }
    }

    /// Parse from string identifier
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "!hide" => Some(Self::Hide),
            "!warn" => Some(Self::Warn),
            "!no-unauthenticated" => Some(Self::NoUnauthenticated),
            "porn" => Some(Self::Porn),
            "sexual" => Some(Self::Sexual),
            "nudity" => Some(Self::Nudity),
            "graphic-media" => Some(Self::GraphicMedia),
            "gore" => Some(Self::Gore),
            _ => None,
        }
    }

    /// Get all known label values
    pub fn all() -> &'static [KnownLabelValue] {
        &[
            Self::Hide,
            Self::Warn,
            Self::NoUnauthenticated,
            Self::Porn,
            Self::Sexual,
            Self::Nudity,
            Self::GraphicMedia,
            Self::Gore,
        ]
    }
}

/// Interpreted label value definition with all metadata
#[derive(Debug, Clone)]
pub struct InterpretedLabelValueDefinition {
    /// The label identifier (e.g., "porn", "!hide")
    pub identifier: String,

    /// Can the user configure this label's behavior?
    pub configurable: bool,

    /// Default preference if user hasn't set one
    pub default_setting: LabelPreference,

    /// Flags that modify label behavior
    pub flags: Vec<LabelFlag>,

    /// Severity level of this label
    pub severity: LabelSeverity,

    /// What to blur when this label is applied
    pub blurs: LabelBlurs,

    /// Behaviors for different label targets
    pub behaviors: LabelBehaviors,

    /// DID of the labeler who defined this (None for global labels)
    pub defined_by: Option<String>,
}

/// Behaviors for different targets (account, profile, content)
#[derive(Debug, Clone, Default)]
pub struct LabelBehaviors {
    pub account: Option<ModerationBehavior>,
    pub profile: Option<ModerationBehavior>,
    pub content: Option<ModerationBehavior>,
}

/// Default label settings for known labels
pub fn default_label_settings() -> HashMap<String, LabelPreference> {
    let mut settings = HashMap::new();
    settings.insert("porn".to_string(), LabelPreference::Hide);
    settings.insert("sexual".to_string(), LabelPreference::Warn);
    settings.insert("nudity".to_string(), LabelPreference::Ignore);
    settings.insert("graphic-media".to_string(), LabelPreference::Warn);
    settings
}

/// Get the definition for a known label
pub fn get_label_definition(label: KnownLabelValue) -> InterpretedLabelValueDefinition {
    match label {
        KnownLabelValue::Hide => InterpretedLabelValueDefinition {
            identifier: "!hide".to_string(),
            configurable: false,
            default_setting: LabelPreference::Hide,
            flags: vec![LabelFlag::NoOverride, LabelFlag::NoSelf],
            severity: LabelSeverity::Alert,
            blurs: LabelBlurs::Content,
            behaviors: LabelBehaviors {
                account: Some(ModerationBehavior {
                    profile_list: Some(ModerationAction::Blur),
                    profile_view: Some(ModerationAction::Blur),
                    avatar: Some(ModerationAction::Blur),
                    banner: Some(ModerationAction::Blur),
                    display_name: Some(ModerationAction::Blur),
                    content_list: Some(ModerationAction::Blur),
                    content_view: Some(ModerationAction::Blur),
                    content_media: None,
                }),
                profile: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: Some(ModerationAction::Blur),
                    banner: Some(ModerationAction::Blur),
                    display_name: Some(ModerationAction::Blur),
                    content_list: None,
                    content_view: None,
                    content_media: None,
                }),
                content: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: None,
                    banner: None,
                    display_name: None,
                    content_list: Some(ModerationAction::Blur),
                    content_view: Some(ModerationAction::Blur),
                    content_media: None,
                }),
            },
            defined_by: None,
        },

        KnownLabelValue::Warn => InterpretedLabelValueDefinition {
            identifier: "!warn".to_string(),
            configurable: false,
            default_setting: LabelPreference::Warn,
            flags: vec![LabelFlag::NoSelf],
            severity: LabelSeverity::None,
            blurs: LabelBlurs::Content,
            behaviors: LabelBehaviors {
                account: Some(ModerationBehavior {
                    profile_list: Some(ModerationAction::Blur),
                    profile_view: Some(ModerationAction::Blur),
                    avatar: Some(ModerationAction::Blur),
                    banner: Some(ModerationAction::Blur),
                    display_name: None,
                    content_list: Some(ModerationAction::Blur),
                    content_view: Some(ModerationAction::Blur),
                    content_media: None,
                }),
                profile: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: Some(ModerationAction::Blur),
                    banner: Some(ModerationAction::Blur),
                    display_name: Some(ModerationAction::Blur),
                    content_list: None,
                    content_view: None,
                    content_media: None,
                }),
                content: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: None,
                    banner: None,
                    display_name: None,
                    content_list: Some(ModerationAction::Blur),
                    content_view: Some(ModerationAction::Blur),
                    content_media: None,
                }),
            },
            defined_by: None,
        },

        KnownLabelValue::NoUnauthenticated => InterpretedLabelValueDefinition {
            identifier: "!no-unauthenticated".to_string(),
            configurable: false,
            default_setting: LabelPreference::Hide,
            flags: vec![LabelFlag::NoOverride, LabelFlag::Unauthed],
            severity: LabelSeverity::None,
            blurs: LabelBlurs::Content,
            behaviors: LabelBehaviors {
                account: Some(ModerationBehavior {
                    profile_list: Some(ModerationAction::Blur),
                    profile_view: Some(ModerationAction::Blur),
                    avatar: Some(ModerationAction::Blur),
                    banner: Some(ModerationAction::Blur),
                    display_name: Some(ModerationAction::Blur),
                    content_list: Some(ModerationAction::Blur),
                    content_view: Some(ModerationAction::Blur),
                    content_media: None,
                }),
                profile: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: Some(ModerationAction::Blur),
                    banner: Some(ModerationAction::Blur),
                    display_name: Some(ModerationAction::Blur),
                    content_list: None,
                    content_view: None,
                    content_media: None,
                }),
                content: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: None,
                    banner: None,
                    display_name: None,
                    content_list: Some(ModerationAction::Blur),
                    content_view: Some(ModerationAction::Blur),
                    content_media: None,
                }),
            },
            defined_by: None,
        },

        KnownLabelValue::Porn => InterpretedLabelValueDefinition {
            identifier: "porn".to_string(),
            configurable: true,
            default_setting: LabelPreference::Hide,
            flags: vec![LabelFlag::Adult],
            severity: LabelSeverity::None,
            blurs: LabelBlurs::Media,
            behaviors: LabelBehaviors {
                account: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: Some(ModerationAction::Blur),
                    banner: Some(ModerationAction::Blur),
                    display_name: None,
                    content_list: None,
                    content_view: None,
                    content_media: None,
                }),
                profile: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: Some(ModerationAction::Blur),
                    banner: Some(ModerationAction::Blur),
                    display_name: None,
                    content_list: None,
                    content_view: None,
                    content_media: None,
                }),
                content: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: None,
                    banner: None,
                    display_name: None,
                    content_list: None,
                    content_view: None,
                    content_media: Some(ModerationAction::Blur),
                }),
            },
            defined_by: None,
        },

        KnownLabelValue::Sexual => InterpretedLabelValueDefinition {
            identifier: "sexual".to_string(),
            configurable: true,
            default_setting: LabelPreference::Warn,
            flags: vec![LabelFlag::Adult],
            severity: LabelSeverity::None,
            blurs: LabelBlurs::Media,
            behaviors: LabelBehaviors {
                account: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: Some(ModerationAction::Blur),
                    banner: Some(ModerationAction::Blur),
                    display_name: None,
                    content_list: None,
                    content_view: None,
                    content_media: None,
                }),
                profile: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: Some(ModerationAction::Blur),
                    banner: Some(ModerationAction::Blur),
                    display_name: None,
                    content_list: None,
                    content_view: None,
                    content_media: None,
                }),
                content: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: None,
                    banner: None,
                    display_name: None,
                    content_list: None,
                    content_view: None,
                    content_media: Some(ModerationAction::Blur),
                }),
            },
            defined_by: None,
        },

        KnownLabelValue::Nudity => InterpretedLabelValueDefinition {
            identifier: "nudity".to_string(),
            configurable: true,
            default_setting: LabelPreference::Ignore,
            flags: vec![],
            severity: LabelSeverity::None,
            blurs: LabelBlurs::Media,
            behaviors: LabelBehaviors {
                account: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: Some(ModerationAction::Blur),
                    banner: Some(ModerationAction::Blur),
                    display_name: None,
                    content_list: None,
                    content_view: None,
                    content_media: None,
                }),
                profile: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: Some(ModerationAction::Blur),
                    banner: Some(ModerationAction::Blur),
                    display_name: None,
                    content_list: None,
                    content_view: None,
                    content_media: None,
                }),
                content: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: None,
                    banner: None,
                    display_name: None,
                    content_list: None,
                    content_view: None,
                    content_media: Some(ModerationAction::Blur),
                }),
            },
            defined_by: None,
        },

        KnownLabelValue::GraphicMedia | KnownLabelValue::Gore => InterpretedLabelValueDefinition {
            identifier: if matches!(label, KnownLabelValue::GraphicMedia) {
                "graphic-media"
            } else {
                "gore"
            }
            .to_string(),
            configurable: true,
            default_setting: LabelPreference::Warn,
            flags: vec![LabelFlag::Adult],
            severity: LabelSeverity::None,
            blurs: LabelBlurs::Media,
            behaviors: LabelBehaviors {
                account: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: Some(ModerationAction::Blur),
                    banner: Some(ModerationAction::Blur),
                    display_name: None,
                    content_list: None,
                    content_view: None,
                    content_media: None,
                }),
                profile: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: Some(ModerationAction::Blur),
                    banner: Some(ModerationAction::Blur),
                    display_name: None,
                    content_list: None,
                    content_view: None,
                    content_media: None,
                }),
                content: Some(ModerationBehavior {
                    profile_list: None,
                    profile_view: None,
                    avatar: None,
                    banner: None,
                    display_name: None,
                    content_list: None,
                    content_view: None,
                    content_media: Some(ModerationAction::Blur),
                }),
            },
            defined_by: None,
        },
    }
}

/// Get all label definitions as a map
pub fn get_all_label_definitions() -> HashMap<String, InterpretedLabelValueDefinition> {
    KnownLabelValue::all()
        .iter()
        .map(|&label| {
            let def = get_label_definition(label);
            (def.identifier.clone(), def)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_label_parsing() {
        assert_eq!(
            KnownLabelValue::from_str("!hide"),
            Some(KnownLabelValue::Hide)
        );
        assert_eq!(
            KnownLabelValue::from_str("porn"),
            Some(KnownLabelValue::Porn)
        );
        assert_eq!(
            KnownLabelValue::from_str("sexual"),
            Some(KnownLabelValue::Sexual)
        );
        assert_eq!(KnownLabelValue::from_str("invalid"), None);
    }

    #[test]
    fn test_label_definition_hide() {
        let def = get_label_definition(KnownLabelValue::Hide);
        assert_eq!(def.identifier, "!hide");
        assert!(!def.configurable);
        assert_eq!(def.default_setting, LabelPreference::Hide);
        assert!(def.flags.contains(&LabelFlag::NoOverride));
    }

    #[test]
    fn test_label_definition_porn() {
        let def = get_label_definition(KnownLabelValue::Porn);
        assert_eq!(def.identifier, "porn");
        assert!(def.configurable);
        assert_eq!(def.default_setting, LabelPreference::Hide);
        assert!(def.flags.contains(&LabelFlag::Adult));
    }

    #[test]
    fn test_default_label_settings() {
        let settings = default_label_settings();
        assert_eq!(settings.get("porn"), Some(&LabelPreference::Hide));
        assert_eq!(settings.get("sexual"), Some(&LabelPreference::Warn));
        assert_eq!(settings.get("nudity"), Some(&LabelPreference::Ignore));
    }

    #[test]
    fn test_all_labels_have_definitions() {
        let all_defs = get_all_label_definitions();
        for &label in KnownLabelValue::all() {
            let id = label.as_str();
            assert!(
                all_defs.contains_key(id),
                "Missing definition for {}",
                id
            );
        }
    }
}
