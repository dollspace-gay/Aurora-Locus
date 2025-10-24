//! Moderation UI output
//!
//! This module provides the final UI output for how to display moderated content.

use super::decision::ModerationCause;

/// UI output for moderation - what to actually show in the interface
#[derive(Debug, Clone, Default)]
pub struct ModerationUI {
    /// Cannot be overridden by user action
    pub no_override: bool,

    /// Causes that should filter content from view entirely
    pub filters: Vec<ModerationCause>,

    /// Causes that should blur/hide content with reveal option
    pub blurs: Vec<ModerationCause>,

    /// Causes that should show an alert/warning
    pub alerts: Vec<ModerationCause>,

    /// Causes that should show informational badges
    pub informs: Vec<ModerationCause>,
}

impl ModerationUI {
    /// Create a new empty moderation UI
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a filter cause
    pub fn add_filter(&mut self, cause: ModerationCause) {
        self.filters.push(cause);
    }

    /// Add a blur cause
    pub fn add_blur(&mut self, cause: ModerationCause) {
        self.blurs.push(cause);
    }

    /// Add an alert cause
    pub fn add_alert(&mut self, cause: ModerationCause) {
        self.alerts.push(cause);
    }

    /// Add an inform cause
    pub fn add_inform(&mut self, cause: ModerationCause) {
        self.informs.push(cause);
    }

    /// Should this content be filtered from view?
    pub fn filter(&self) -> bool {
        !self.filters.is_empty()
    }

    /// Should this content be blurred?
    pub fn blur(&self) -> bool {
        !self.blurs.is_empty()
    }

    /// Should an alert/warning be shown?
    pub fn alert(&self) -> bool {
        !self.alerts.is_empty()
    }

    /// Should informational badges be shown?
    pub fn inform(&self) -> bool {
        !self.informs.is_empty()
    }

    /// Get all causes affecting this UI
    pub fn all_causes(&self) -> Vec<&ModerationCause> {
        let mut causes = Vec::new();
        causes.extend(self.filters.iter());
        causes.extend(self.blurs.iter());
        causes.extend(self.alerts.iter());
        causes.extend(self.informs.iter());
        causes
    }

    /// Get the highest priority cause
    pub fn primary_cause(&self) -> Option<&ModerationCause> {
        self.all_causes()
            .into_iter()
            .min_by_key(|c| c.priority())
    }

    /// Get a summary of what moderation actions are needed
    pub fn summary(&self) -> ModerationSummary {
        ModerationSummary {
            filter: self.filter(),
            blur: self.blur(),
            alert: self.alert(),
            inform: self.inform(),
            no_override: self.no_override,
            primary_cause_type: self.primary_cause().map(|c| c.cause_type()),
        }
    }
}

/// Summary of moderation actions
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModerationSummary {
    /// Filter from view
    pub filter: bool,

    /// Blur content
    pub blur: bool,

    /// Show alert
    pub alert: bool,

    /// Show info badge
    pub inform: bool,

    /// Cannot override
    pub no_override: bool,

    /// Type of primary cause
    pub primary_cause_type: Option<&'static str>,
}

impl ModerationSummary {
    /// Is any moderation active?
    pub fn is_moderated(&self) -> bool {
        self.filter || self.blur || self.alert || self.inform
    }

    /// What is the strongest action needed?
    pub fn strongest_action(&self) -> Option<&'static str> {
        if self.filter {
            Some("filter")
        } else if self.blur {
            Some("blur")
        } else if self.alert {
            Some("alert")
        } else if self.inform {
            Some("inform")
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::moderation::{ModerationCauseSource, PRIORITY_BLOCKING, PRIORITY_MED_SEVERITY};

    #[test]
    fn test_moderation_ui_new() {
        let ui = ModerationUI::new();
        assert!(!ui.filter());
        assert!(!ui.blur());
        assert!(!ui.alert());
        assert!(!ui.inform());
        assert!(!ui.no_override);
    }

    #[test]
    fn test_moderation_ui_blur() {
        let mut ui = ModerationUI::new();

        ui.add_blur(ModerationCause::Muted {
            source: ModerationCauseSource::User,
            priority: PRIORITY_MED_SEVERITY,
            downgraded: false,
        });

        assert!(!ui.filter());
        assert!(ui.blur());
        assert!(!ui.alert());
        assert!(!ui.inform());
    }

    #[test]
    fn test_moderation_ui_alert() {
        let mut ui = ModerationUI::new();

        ui.add_alert(ModerationCause::Blocking {
            source: ModerationCauseSource::User,
            priority: PRIORITY_BLOCKING,
            downgraded: false,
        });

        assert!(!ui.filter());
        assert!(!ui.blur());
        assert!(ui.alert());
        assert!(!ui.inform());
    }

    #[test]
    fn test_moderation_ui_multiple_causes() {
        let mut ui = ModerationUI::new();

        ui.add_blur(ModerationCause::Muted {
            source: ModerationCauseSource::User,
            priority: PRIORITY_MED_SEVERITY,
            downgraded: false,
        });

        ui.add_alert(ModerationCause::Blocking {
            source: ModerationCauseSource::User,
            priority: PRIORITY_BLOCKING,
            downgraded: false,
        });

        assert!(ui.blur());
        assert!(ui.alert());

        let summary = ui.summary();
        assert!(summary.is_moderated());
        assert_eq!(summary.strongest_action(), Some("blur"));
    }

    #[test]
    fn test_moderation_ui_primary_cause() {
        let mut ui = ModerationUI::new();

        // Lower priority number = higher priority
        ui.add_blur(ModerationCause::Muted {
            source: ModerationCauseSource::User,
            priority: PRIORITY_MED_SEVERITY, // 7
            downgraded: false,
        });

        ui.add_alert(ModerationCause::Blocking {
            source: ModerationCauseSource::User,
            priority: PRIORITY_BLOCKING, // 3
            downgraded: false,
        });

        let primary = ui.primary_cause();
        assert!(primary.is_some());
        assert_eq!(primary.unwrap().priority(), PRIORITY_BLOCKING);
        assert_eq!(primary.unwrap().cause_type(), "blocking");
    }

    #[test]
    fn test_moderation_summary() {
        let mut ui = ModerationUI::new();

        ui.add_blur(ModerationCause::Muted {
            source: ModerationCauseSource::User,
            priority: PRIORITY_MED_SEVERITY,
            downgraded: false,
        });

        let summary = ui.summary();
        assert!(summary.is_moderated());
        assert!(summary.blur);
        assert!(!summary.filter);
        assert!(!summary.alert);
        assert!(!summary.inform);
        assert_eq!(summary.strongest_action(), Some("blur"));
        assert_eq!(summary.primary_cause_type, Some("muted"));
    }
}
