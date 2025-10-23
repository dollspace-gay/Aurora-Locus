/// Email sending functionality
use crate::{
    config::EmailConfig,
    error::{PdsError, PdsResult},
};
use lettre::{
    message::{header::ContentType, Message},
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Tokio1Executor,
};

/// Email mailer service
#[derive(Clone)]
pub struct Mailer {
    config: Option<EmailConfig>,
    transport: Option<AsyncSmtpTransport<Tokio1Executor>>,
}

impl Mailer {
    /// Create a new mailer
    pub fn new(config: Option<EmailConfig>) -> PdsResult<Self> {
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

                    let (host, port_str) = if let Some((h, p)) = host_part.split_once(':') {
                        (h, p)
                    } else {
                        (host_part, "587") // Default SMTP submission port
                    };

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

        Ok(Self { config, transport })
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
}
