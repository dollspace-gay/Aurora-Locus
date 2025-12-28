/// Email sending functionality
pub mod notifications;
pub mod rate_limit;
pub mod templates;
pub mod tracking;

pub use notifications::NotificationEmail;
pub use rate_limit::EmailRateLimiter;
pub use templates::EmailTemplateManager;

use crate::{
    config::EmailConfig,
    error::{PdsError, PdsResult},
};
use lettre::{
    message::{header::ContentType, Message},
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Tokio1Executor,
};
use std::sync::Arc;

/// Email mailer service
#[derive(Clone)]
pub struct Mailer {
    config: Option<EmailConfig>,
    transport: Option<AsyncSmtpTransport<Tokio1Executor>>,
    #[allow(dead_code)] // Future mailer features
    template_manager: Arc<EmailTemplateManager>,
    rate_limiter: Arc<EmailRateLimiter>,
}

impl Mailer {
    /// Create a new mailer
    pub fn new(config: Option<EmailConfig>) -> PdsResult<Self> {
        let template_manager = Arc::new(EmailTemplateManager::new());
        let rate_limiter = Arc::new(EmailRateLimiter::default());

        let transport = if let Some(ref email_config) = config {
            // Parse SMTP URL (format: smtp://username:password@host:port)
            let smtp_url = &email_config.smtp_url;

            // For now, support simple smtp://user:pass@host:port format
            // In production, you'd want more robust URL parsing
            let transport = if smtp_url.starts_with("smtp://") {
                // Extract credentials and host from URL
                // This is a simplified implementation
                let without_scheme = smtp_url.trim_start_matches("smtp://");

                if let Some((creds_part, host_part)) = without_scheme.split_once('@') {
                    let (username, password) = if let Some((u, p)) = creds_part.split_once(':') {
                        (u.to_string(), p.to_string())
                    } else {
                        return Err(PdsError::Internal("Invalid SMTP URL format".to_string()));
                    };

                    let (host, _port_str) = if let Some((h, p)) = host_part.split_once(':') {
                        (h, p)
                    } else {
                        (host_part, "587") // Default SMTP submission port
                    };
                    // TODO: Parse and use _port_str instead of hardcoded port

                    let creds = Credentials::new(username, password);

                    AsyncSmtpTransport::<Tokio1Executor>::relay(host)
                        .map_err(|e| PdsError::Internal(format!("SMTP setup failed: {}", e)))?
                        .credentials(creds)
                        .build()
                } else {
                    return Err(PdsError::Internal("Invalid SMTP URL format".to_string()));
                }
            } else {
                return Err(PdsError::Internal("SMTP URL must start with smtp://".to_string()));
            };

            Some(transport)
        } else {
            None
        };

        Ok(Self {
            config,
            transport,
            template_manager,
            rate_limiter,
        })
    }

    /// Send an email verification message
    pub async fn send_verification_email(
        &self,
        to_email: &str,
        handle: &str,
        token: &str,
        base_url: &str,
    ) -> PdsResult<()> {
        if self.config.is_none() {
            tracing::warn!("Email not configured, skipping verification email to {}", to_email);
            return Ok(());
        }

        let config = self.config.as_ref().unwrap();
        let verification_url = format!("{}/verify-email?token={}", base_url, token);

        let body = format!(
            r#"
Hello {},

Thank you for creating an account on our AT Protocol Personal Data Server!

Please verify your email address by clicking the link below:

{}

This link will expire in 24 hours.

If you did not create this account, please ignore this email.

Best regards,
Aurora Locus PDS
"#,
            handle, verification_url
        );

        self.send_email(
            to_email,
            "Verify your email address",
            &body,
            &config.from_address,
        )
        .await
    }

    /// Send a password reset email
    pub async fn send_password_reset_email(
        &self,
        to_email: &str,
        handle: &str,
        token: &str,
        base_url: &str,
    ) -> PdsResult<()> {
        if self.config.is_none() {
            tracing::warn!("Email not configured, skipping password reset email to {}", to_email);
            return Ok(());
        }

        let config = self.config.as_ref().unwrap();
        let reset_url = format!("{}/reset-password?token={}", base_url, token);

        let body = format!(
            r#"
Hello {},

We received a request to reset the password for your account on our AT Protocol Personal Data Server.

To reset your password, click the link below:

{}

This link will expire in 1 hour.

If you did not request a password reset, please ignore this email. Your password will remain unchanged.

For security, this link can only be used once.

Best regards,
Aurora Locus PDS
"#,
            handle, reset_url
        );

        self.send_email(
            to_email,
            "Reset your password",
            &body,
            &config.from_address,
        )
        .await
    }

    /// Send an account deletion confirmation email
    ///
    /// This email contains a token that must be used to confirm account deletion.
    /// The token expires in 1 hour.
    pub async fn send_account_delete_email(
        &self,
        to_email: &str,
        handle: &str,
        token: &str,
    ) -> PdsResult<()> {
        if self.config.is_none() {
            tracing::warn!("Email not configured, skipping account delete email to {}", to_email);
            return Ok(());
        }

        let config = self.config.as_ref().unwrap();

        let body = format!(
            r#"
Hello {},

We received a request to permanently delete your account on our AT Protocol Personal Data Server.

To confirm this deletion, use the following confirmation code:

{}

This code will expire in 1 hour.

WARNING: Account deletion is permanent and cannot be undone. All your data, including:
- Your profile and posts
- Your followers and following lists
- Your preferences and settings
- All associated blobs and media

will be permanently removed.

If you did not request this deletion, please ignore this email and consider changing your password immediately for security.

Best regards,
Aurora Locus PDS
"#,
            handle, token
        );

        self.send_email(
            to_email,
            "Confirm account deletion",
            &body,
            &config.from_address,
        )
        .await
    }

    /// Send an email update confirmation email
    ///
    /// This email contains a token that must be used to confirm email address change.
    /// The token expires in 1 hour.
    pub async fn send_email_update_email(
        &self,
        to_email: &str,
        handle: &str,
        token: &str,
    ) -> PdsResult<()> {
        if self.config.is_none() {
            tracing::warn!("Email not configured, skipping email update email to {}", to_email);
            return Ok(());
        }

        let config = self.config.as_ref().unwrap();

        let body = format!(
            r#"
Hello {},

We received a request to change the email address associated with your account on our AT Protocol Personal Data Server.

To authorize this email change, use the following confirmation code:

{}

This code will expire in 1 hour.

If you did not request this change, please ignore this email and consider changing your password immediately for security.

Best regards,
Aurora Locus PDS
"#,
            handle, token
        );

        self.send_email(
            to_email,
            "Confirm email address change",
            &body,
            &config.from_address,
        )
        .await
    }

    /// Send a generic email
    async fn send_email(
        &self,
        to: &str,
        subject: &str,
        body: &str,
        from: &str,
    ) -> PdsResult<()> {
        if let Some(transport) = &self.transport {
            let email = Message::builder()
                .from(from.parse().map_err(|e| {
                    PdsError::Internal(format!("Invalid from address: {}", e))
                })?)
                .to(to.parse().map_err(|e| {
                    PdsError::Internal(format!("Invalid to address: {}", e))
                })?)
                .subject(subject)
                .header(ContentType::TEXT_PLAIN)
                .body(body.to_string())
                .map_err(|e| PdsError::Internal(format!("Failed to build email: {}", e)))?;

            transport
                .send(email)
                .await
                .map_err(|e| PdsError::Internal(format!("Failed to send email: {}", e)))?;

            tracing::info!("Sent email to {}: {}", to, subject);
            Ok(())
        } else {
            tracing::warn!("Email transport not configured, cannot send email");
            Ok(())
        }
    }

    /// Check if email is configured
    pub fn is_configured(&self) -> bool {
        self.config.is_some()
    }

    /// Send an admin email (for moderation notices, warnings, etc.)
    ///
    /// This is a public method for sending custom emails from admin actions.
    /// It bypasses templates and rate limiting for admin use.
    pub async fn send_admin_email(
        &self,
        to_email: &str,
        subject: &str,
        content: &str,
    ) -> PdsResult<()> {
        if self.config.is_none() {
            return Err(PdsError::Internal("Email not configured".to_string()));
        }

        let config = self.config.as_ref().unwrap();
        self.send_email(to_email, subject, content, &config.from_address).await
    }

    /// Send notification email using template system
    #[allow(dead_code)] // Public API for templated notifications
    pub async fn send_notification(
        &self,
        to_email: &str,
        notification: &NotificationEmail,
    ) -> PdsResult<()> {
        if self.config.is_none() {
            tracing::warn!("Email not configured, skipping notification to {}", to_email);
            return Ok(());
        }

        // Check rate limit
        self.rate_limiter.check_rate_limit(to_email).await?;

        // Render template
        let (subject, body) = notification.render(&self.template_manager)?;

        let config = self.config.as_ref().unwrap();

        // Send email
        self.send_email(to_email, &subject, &body, &config.from_address)
            .await?;

        // Record send for rate limiting
        self.rate_limiter.record_send(to_email).await;

        Ok(())
    }

    /// Get template manager
    #[allow(dead_code)] // Future template access method
    pub fn template_manager(&self) -> &EmailTemplateManager {
        &self.template_manager
    }

    /// Get rate limiter
    #[allow(dead_code)] // Future rate limiter access method
    pub fn rate_limiter(&self) -> &EmailRateLimiter {
        &self.rate_limiter
    }
}
