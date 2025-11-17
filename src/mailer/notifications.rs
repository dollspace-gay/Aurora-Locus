//! Email Notification System
//!
//! High-level interface for sending notification emails with templates.

use super::templates::{EmailTemplateManager, EmailTemplateType};
use crate::error::PdsResult;
use std::collections::HashMap;

/// Notification email builder
pub struct NotificationEmail {
    template_type: EmailTemplateType,
    variables: HashMap<String, String>,
}

impl NotificationEmail {
    /// Create moderation action notification
    pub fn moderation_action(
        handle: &str,
        action: &str,
        reason: &str,
        details: &str,
        service_name: &str,
        service_url: &str,
    ) -> Self {
        let mut vars = HashMap::new();
        vars.insert("handle".to_string(), handle.to_string());
        vars.insert("action".to_string(), action.to_string());
        vars.insert("reason".to_string(), reason.to_string());
        vars.insert("details".to_string(), details.to_string());
        vars.insert("date".to_string(), chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string());
        vars.insert("service_name".to_string(), service_name.to_string());
        vars.insert("service_url".to_string(), service_url.to_string());
        vars.insert("appeal_url".to_string(), format!("{}/appeal", service_url));
        vars.insert("guidelines_url".to_string(), format!("{}/guidelines", service_url));

        Self {
            template_type: EmailTemplateType::ModerationAction,
            variables: vars,
        }
    }

    /// Create appeal update notification
    pub fn appeal_update(
        handle: &str,
        appeal_id: i64,
        status: &str,
        decision: &str,
        details: &str,
        service_name: &str,
        service_url: &str,
    ) -> Self {
        let mut vars = HashMap::new();
        vars.insert("handle".to_string(), handle.to_string());
        vars.insert("appeal_id".to_string(), appeal_id.to_string());
        vars.insert("status".to_string(), status.to_string());
        vars.insert("decision".to_string(), decision.to_string());
        vars.insert("details".to_string(), details.to_string());
        vars.insert("service_name".to_string(), service_name.to_string());
        vars.insert("service_url".to_string(), service_url.to_string());
        vars.insert("appeal_policy_url".to_string(), format!("{}/appeal-policy", service_url));

        Self {
            template_type: EmailTemplateType::AppealUpdate,
            variables: vars,
        }
    }

    /// Create account suspended notification
    pub fn account_suspended(
        handle: &str,
        reason: &str,
        duration: &str,
        expires_at: &str,
        service_name: &str,
        service_url: &str,
    ) -> Self {
        let mut vars = HashMap::new();
        vars.insert("handle".to_string(), handle.to_string());
        vars.insert("reason".to_string(), reason.to_string());
        vars.insert("duration".to_string(), duration.to_string());
        vars.insert("expires_at".to_string(), expires_at.to_string());
        vars.insert("service_name".to_string(), service_name.to_string());
        vars.insert("service_url".to_string(), service_url.to_string());
        vars.insert("appeal_info".to_string(), format!("You can submit an appeal at {}/appeal", service_url));

        Self {
            template_type: EmailTemplateType::AccountSuspended,
            variables: vars,
        }
    }

    /// Create content takedown notification
    pub fn content_takedown(
        handle: &str,
        content_type: &str,
        content_uri: &str,
        reason: &str,
        details: &str,
        service_name: &str,
        service_url: &str,
    ) -> Self {
        let mut vars = HashMap::new();
        vars.insert("handle".to_string(), handle.to_string());
        vars.insert("content_type".to_string(), content_type.to_string());
        vars.insert("content_uri".to_string(), content_uri.to_string());
        vars.insert("reason".to_string(), reason.to_string());
        vars.insert("details".to_string(), details.to_string());
        vars.insert("date".to_string(), chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string());
        vars.insert("service_name".to_string(), service_name.to_string());
        vars.insert("service_url".to_string(), service_url.to_string());
        vars.insert("appeal_info".to_string(), format!("You can submit an appeal at {}/appeal", service_url));

        Self {
            template_type: EmailTemplateType::ContentTakedown,
            variables: vars,
        }
    }

    /// Create security alert notification
    pub fn security_alert(
        handle: &str,
        alert_message: &str,
        ip_address: &str,
        service_name: &str,
        service_url: &str,
    ) -> Self {
        let mut vars = HashMap::new();
        vars.insert("handle".to_string(), handle.to_string());
        vars.insert("alert_message".to_string(), alert_message.to_string());
        vars.insert("ip_address".to_string(), ip_address.to_string());
        vars.insert("date".to_string(), chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string());
        vars.insert("service_name".to_string(), service_name.to_string());
        vars.insert("service_url".to_string(), service_url.to_string());

        Self {
            template_type: EmailTemplateType::SecurityAlert,
            variables: vars,
        }
    }

    /// Create welcome email notification
    pub fn account_created(
        handle: &str,
        did: &str,
        service_name: &str,
        service_url: &str,
    ) -> Self {
        let mut vars = HashMap::new();
        vars.insert("handle".to_string(), handle.to_string());
        vars.insert("did".to_string(), did.to_string());
        vars.insert("service_name".to_string(), service_name.to_string());
        vars.insert("service_url".to_string(), service_url.to_string());
        vars.insert("help_url".to_string(), format!("{}/help", service_url));

        Self {
            template_type: EmailTemplateType::AccountCreated,
            variables: vars,
        }
    }

    /// Get template type
    pub fn template_type(&self) -> EmailTemplateType {
        self.template_type
    }

    /// Get variables
    pub fn variables(&self) -> &HashMap<String, String> {
        &self.variables
    }

    /// Render notification using template manager
    pub fn render(&self, template_manager: &EmailTemplateManager) -> PdsResult<(String, String)> {
        if let Some(rendered) = template_manager.render(self.template_type, &self.variables) {
            Ok((rendered.subject, rendered.body_text))
        } else {
            use crate::error::PdsError;
            Err(PdsError::Internal(format!(
                "Template not found: {:?}",
                self.template_type
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_moderation_action_notification() {
        let notification = NotificationEmail::moderation_action(
            "alice.bsky.social",
            "Takedown",
            "Spam content",
            "Multiple spam posts detected",
            "Aurora PDS",
            "https://example.com",
        );

        assert_eq!(notification.template_type(), EmailTemplateType::ModerationAction);
        assert_eq!(notification.variables().get("handle").unwrap(), "alice.bsky.social");
        assert_eq!(notification.variables().get("action").unwrap(), "Takedown");
    }

    #[test]
    fn test_appeal_update_notification() {
        let notification = NotificationEmail::appeal_update(
            "alice.bsky.social",
            123,
            "Approved",
            "Appeal granted",
            "After review, we determined this was an error",
            "Aurora PDS",
            "https://example.com",
        );

        assert_eq!(notification.template_type(), EmailTemplateType::AppealUpdate);
        assert_eq!(notification.variables().get("appeal_id").unwrap(), "123");
        assert_eq!(notification.variables().get("status").unwrap(), "Approved");
    }
}
