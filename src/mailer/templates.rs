// Allow dead_code - email templates for future use
#![allow(dead_code)]

//! Email Template System
//!
//! Provides a flexible template system for all email types.
//! Templates support variable substitution and HTML/plaintext variants.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Email template type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmailTemplateType {
    /// Email verification
    EmailVerification,
    /// Password reset
    PasswordReset,
    /// Account created welcome email
    AccountCreated,
    /// Account moderation action
    ModerationAction,
    /// Appeal status update
    AppealUpdate,
    /// Account suspension notice
    AccountSuspended,
    /// Account deleted notice
    AccountDeleted,
    /// Content takedown notice
    ContentTakedown,
    /// Security alert
    SecurityAlert,
}

impl EmailTemplateType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EmailTemplateType::EmailVerification => "email_verification",
            EmailTemplateType::PasswordReset => "password_reset",
            EmailTemplateType::AccountCreated => "account_created",
            EmailTemplateType::ModerationAction => "moderation_action",
            EmailTemplateType::AppealUpdate => "appeal_update",
            EmailTemplateType::AccountSuspended => "account_suspended",
            EmailTemplateType::AccountDeleted => "account_deleted",
            EmailTemplateType::ContentTakedown => "content_takedown",
            EmailTemplateType::SecurityAlert => "security_alert",
        }
    }
}

/// Email template with subject and body
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailTemplate {
    pub subject: String,
    pub body_text: String,
    pub body_html: Option<String>,
}

impl EmailTemplate {
    /// Create a new template
    pub fn new(subject: String, body_text: String, body_html: Option<String>) -> Self {
        Self {
            subject,
            body_text,
            body_html,
        }
    }

    /// Render template with variable substitution
    pub fn render(&self, variables: &HashMap<String, String>) -> RenderedEmail {
        let subject = self.substitute(&self.subject, variables);
        let body_text = self.substitute(&self.body_text, variables);
        let body_html = self
            .body_html
            .as_ref()
            .map(|html| self.substitute(html, variables));

        RenderedEmail {
            subject,
            body_text,
            body_html,
        }
    }

    /// Substitute variables in template string
    fn substitute(&self, template: &str, variables: &HashMap<String, String>) -> String {
        let mut result = template.to_string();
        for (key, value) in variables {
            let placeholder = format!("{{{{{}}}}}", key);
            result = result.replace(&placeholder, value);
        }
        result
    }
}

/// Rendered email ready to send
#[derive(Debug, Clone)]
pub struct RenderedEmail {
    pub subject: String,
    pub body_text: String,
    pub body_html: Option<String>,
}

/// Email template manager
pub struct EmailTemplateManager {
    templates: HashMap<String, EmailTemplate>,
}

impl EmailTemplateManager {
    /// Create new template manager with default templates
    pub fn new() -> Self {
        let mut templates = HashMap::new();

        // Email verification template
        templates.insert(
            EmailTemplateType::EmailVerification.as_str().to_string(),
            EmailTemplate::new(
                "Verify your email address".to_string(),
                r#"Hello {{handle}},

Thank you for creating an account on {{service_name}}!

Please verify your email address by clicking the link below:

{{verification_url}}

This link will expire in {{expiry_hours}} hours.

If you did not create this account, please ignore this email.

Best regards,
{{service_name}}

---
You are receiving this email because you created an account at {{service_url}}.
"#
                .to_string(),
                None,
            ),
        );

        // Password reset template
        templates.insert(
            EmailTemplateType::PasswordReset.as_str().to_string(),
            EmailTemplate::new(
                "Reset your password".to_string(),
                r#"Hello {{handle}},

We received a request to reset the password for your account on {{service_name}}.

To reset your password, click the link below:

{{reset_url}}

This link will expire in {{expiry_hours}} hour(s).

If you did not request a password reset, please ignore this email. Your password will remain unchanged.

For security, this link can only be used once.

Best regards,
{{service_name}}

---
You are receiving this email because a password reset was requested for {{email}}.
"#
                .to_string(),
                None,
            ),
        );

        // Account created welcome email
        templates.insert(
            EmailTemplateType::AccountCreated.as_str().to_string(),
            EmailTemplate::new(
                "Welcome to {{service_name}}!".to_string(),
                r#"Hello {{handle}},

Welcome to {{service_name}}!

Your account has been successfully created and verified.

Your handle: {{handle}}
Your DID: {{did}}

You can now use your account with any AT Protocol compatible client.

Getting started:
- Download the Bluesky app or use a compatible client
- Sign in with your handle: {{handle}}
- Start posting, following, and exploring the network!

If you have any questions, please visit our help center at {{help_url}}.

Best regards,
{{service_name}}

---
You are receiving this email because you created an account at {{service_url}}.
"#
                .to_string(),
                None,
            ),
        );

        // Moderation action template
        templates.insert(
            EmailTemplateType::ModerationAction.as_str().to_string(),
            EmailTemplate::new(
                "Moderation action taken on your account".to_string(),
                r#"Hello {{handle}},

A moderation action has been taken on your account.

Action: {{action}}
Reason: {{reason}}
Date: {{date}}

{{details}}

If you believe this action was taken in error, you can submit an appeal at:
{{appeal_url}}

For more information about our community guidelines, please visit:
{{guidelines_url}}

Best regards,
{{service_name}} Moderation Team

---
You are receiving this email because moderation action was taken on your account at {{service_url}}.
"#
                .to_string(),
                None,
            ),
        );

        // Appeal update template
        templates.insert(
            EmailTemplateType::AppealUpdate.as_str().to_string(),
            EmailTemplate::new(
                "Update on your appeal".to_string(),
                r#"Hello {{handle}},

There is an update on your appeal.

Appeal ID: {{appeal_id}}
Status: {{status}}
Decision: {{decision}}

{{details}}

If you have questions about this decision, please review our appeal policy at:
{{appeal_policy_url}}

Best regards,
{{service_name}} Moderation Team

---
You are receiving this email regarding your appeal at {{service_url}}.
"#
                .to_string(),
                None,
            ),
        );

        // Account suspended template
        templates.insert(
            EmailTemplateType::AccountSuspended.as_str().to_string(),
            EmailTemplate::new(
                "Your account has been suspended".to_string(),
                r#"Hello {{handle}},

Your account has been temporarily suspended.

Reason: {{reason}}
Suspension period: {{duration}}
Expires: {{expires_at}}

During this suspension, you will not be able to:
- Post new content
- Interact with other users
- Access certain features

{{appeal_info}}

Best regards,
{{service_name}} Moderation Team

---
You are receiving this email because your account at {{service_url}} has been suspended.
"#
                .to_string(),
                None,
            ),
        );

        // Content takedown template
        templates.insert(
            EmailTemplateType::ContentTakedown.as_str().to_string(),
            EmailTemplate::new(
                "Content removed from your account".to_string(),
                r#"Hello {{handle}},

Content from your account has been removed.

Content type: {{content_type}}
Content URI: {{content_uri}}
Reason: {{reason}}
Date: {{date}}

{{details}}

{{appeal_info}}

Best regards,
{{service_name}} Moderation Team

---
You are receiving this email because content was removed from your account at {{service_url}}.
"#
                .to_string(),
                None,
            ),
        );

        // Security alert template
        templates.insert(
            EmailTemplateType::SecurityAlert.as_str().to_string(),
            EmailTemplate::new(
                "Security alert for your account".to_string(),
                r#"Hello {{handle}},

We detected important activity on your account:

{{alert_message}}

Date: {{date}}
IP Address: {{ip_address}}

If this was you, you can safely ignore this email.

If you did not perform this action, please:
1. Reset your password immediately
2. Review your account security settings
3. Contact support if you need assistance

Security tips:
- Use a strong, unique password
- Enable two-factor authentication if available
- Be cautious of phishing attempts

Best regards,
{{service_name}} Security Team

---
You are receiving this security alert for your account at {{service_url}}.
"#
                .to_string(),
                None,
            ),
        );

        Self { templates }
    }

    /// Get template by type
    pub fn get(&self, template_type: EmailTemplateType) -> Option<&EmailTemplate> {
        self.templates.get(template_type.as_str())
    }

    /// Render template with variables
    pub fn render(
        &self,
        template_type: EmailTemplateType,
        variables: &HashMap<String, String>,
    ) -> Option<RenderedEmail> {
        self.get(template_type).map(|tmpl| tmpl.render(variables))
    }
}

impl Default for EmailTemplateManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_substitution() {
        let template = EmailTemplate::new(
            "Hello {{name}}".to_string(),
            "Welcome {{name}} to {{service}}!".to_string(),
            None,
        );

        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "Alice".to_string());
        vars.insert("service".to_string(), "Aurora PDS".to_string());

        let rendered = template.render(&vars);
        assert_eq!(rendered.subject, "Hello Alice");
        assert_eq!(rendered.body_text, "Welcome Alice to Aurora PDS!");
    }

    #[test]
    fn test_template_manager() {
        let manager = EmailTemplateManager::new();

        let mut vars = HashMap::new();
        vars.insert("handle".to_string(), "alice.bsky.social".to_string());
        vars.insert("verification_url".to_string(), "https://example.com/verify?token=abc123".to_string());
        vars.insert("service_name".to_string(), "Aurora PDS".to_string());
        vars.insert("expiry_hours".to_string(), "24".to_string());
        vars.insert("service_url".to_string(), "https://example.com".to_string());

        let rendered = manager.render(EmailTemplateType::EmailVerification, &vars);
        assert!(rendered.is_some());

        let email = rendered.unwrap();
        assert!(email.subject.contains("Verify"));
        assert!(email.body_text.contains("alice.bsky.social"));
        assert!(email.body_text.contains("https://example.com/verify?token=abc123"));
    }
}
