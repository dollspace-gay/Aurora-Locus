//! The mailer and email templates.
//!
//! SMTP URLs (which carry credentials) live in the encrypted key store.
//! A sender counts as configured once its URL passed a connect-and-auth
//! check (or a test email arrived); otherwise features that need email
//! report it unavailable instead of pretending to send.

pub mod provider;
pub mod templates;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lettre::message::{Mailbox, MultiPart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use locus_core::CoreResult;
use locus_core::keys::MasterKey;
use locus_store::Store;
use locus_store::email::{Sender, SenderStatus};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub use templates::{Email, Template};

const SMTP_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Error)]
pub enum MailError {
    #[error("Email is not configured on this server")]
    NotConfigured,
    #[error("invalid address: {0}")]
    Address(String),
    #[error("sending failed: {0}")]
    Send(String),
}

/// The key-store name of a sender's SMTP URL.
#[must_use]
pub fn smtp_secret_name(sender: Sender) -> &'static str {
    match sender {
        Sender::Main => "email/smtp_url",
        Sender::Moderation => "email/moderation_smtp_url",
    }
}

fn url_hash(url: &str) -> String {
    hex::encode(Sha256::digest(url.as_bytes()))
}

/// A message the capturing mailer recorded (tests and dry runs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub sender: Sender,
    pub from: String,
    pub to: String,
    pub email: Email,
}

#[derive(Clone)]
enum Mode {
    Smtp,
    Capture(Arc<Mutex<Vec<Sent>>>),
}

#[derive(Clone)]
pub struct Mailer {
    store: Store,
    key: Arc<MasterKey>,
    mode: Mode,
}

impl Mailer {
    #[must_use]
    pub fn new(store: Store, key: Arc<MasterKey>) -> Self {
        Self {
            store,
            key,
            mode: Mode::Smtp,
        }
    }

    /// A mailer that records messages instead of sending them, and counts
    /// as configured. Returns the record.
    #[must_use]
    pub fn capturing(store: Store, key: Arc<MasterKey>) -> (Self, Arc<Mutex<Vec<Sent>>>) {
        let sent = Arc::default();
        (
            Self {
                store,
                key,
                mode: Mode::Capture(Arc::clone(&sent)),
            },
            sent,
        )
    }

    async fn url(&self, sender: Sender) -> CoreResult<Option<String>> {
        Ok(self
            .store
            .get_secret(smtp_secret_name(sender), &self.key)
            .await?
            .and_then(|b| String::from_utf8(b.to_vec()).ok())
            .filter(|u| !u.is_empty()))
    }

    /// The sender's status, if it has an SMTP URL that was checked.
    pub async fn status(&self, sender: Sender) -> CoreResult<Option<SenderStatus>> {
        if matches!(self.mode, Mode::Capture(_)) {
            return Ok(Some(SenderStatus::Verified));
        }
        let Some(url) = self.url(sender).await? else {
            return Ok(None);
        };
        Ok(self
            .store
            .sender_state(sender)
            .await?
            .filter(|s| s.url_hash == url_hash(&url))
            .map(|s| s.status))
    }

    /// Whether `sender` can send (the moderation sender falls back to the
    /// main one).
    pub async fn configured(&self, sender: Sender) -> CoreResult<bool> {
        Ok(self.effective(sender).await?.is_some())
    }

    async fn effective(&self, sender: Sender) -> CoreResult<Option<Sender>> {
        for s in [sender, Sender::Main] {
            if self.status(s).await?.is_some_and(SenderStatus::configured) {
                return Ok(Some(s));
            }
            if s == Sender::Main {
                break;
            }
        }
        Ok(None)
    }

    /// Stores a sender's SMTP URL and checks it (connect and auth).
    pub async fn set_url(&self, sender: Sender, url: &str) -> CoreResult<SenderStatus> {
        let mut tx = self.store.begin().await?;
        tx.put_secret(smtp_secret_name(sender), url.as_bytes(), &self.key)
            .await?;
        tx.commit().await?;
        self.check(sender).await
    }

    /// Connect-and-auth check of a sender's stored URL, recorded as its
    /// status. A URL that already delivered a test email stays verified.
    pub async fn check(&self, sender: Sender) -> CoreResult<SenderStatus> {
        let Some(url) = self.url(sender).await? else {
            return Ok(SenderStatus::Failed);
        };
        let hash = url_hash(&url);
        let result = match transport(&url) {
            Ok(t) => t
                .test_connection()
                .await
                .map_err(|e| e.to_string())
                .and_then(|ok| {
                    if ok {
                        Ok(())
                    } else {
                        Err("the server refused the connection".to_owned())
                    }
                }),
            Err(e) => Err(e),
        };
        let status = match &result {
            Ok(()) => {
                let prior = self.store.sender_state(sender).await?;
                if prior.is_some_and(|p| p.url_hash == hash && p.status == SenderStatus::Verified) {
                    SenderStatus::Verified
                } else {
                    SenderStatus::Unverified
                }
            }
            Err(_) => SenderStatus::Failed,
        };
        self.store
            .set_sender_state(sender, status, &hash, result.err().as_deref())
            .await?;
        Ok(status)
    }

    /// After a failed check: the blocked-port hint, when the sender's last
    /// failure was a connection on one of the usual mail ports.
    pub async fn blocked_port_hint(&self, sender: Sender) -> Option<&'static str> {
        let url = self.url(sender).await.ok()??;
        let error = self.store.sender_state(sender).await.ok()??.last_error?;
        provider::blocked_port_hint(&url, &error)
    }

    /// Records that a test email sent with the current URL arrived.
    pub async fn mark_verified(&self, sender: Sender) -> CoreResult<()> {
        if let Some(url) = self.url(sender).await? {
            self.store
                .set_sender_state(sender, SenderStatus::Verified, &url_hash(&url), None)
                .await?;
        }
        Ok(())
    }

    /// Imports `PDS_EMAIL_SMTP_URL` and `PDS_MODERATION_EMAIL_SMTP_URL` when
    /// they differ from what's stored, then checks every stored URL.
    pub async fn import_env_and_check(&self, env: &HashMap<String, String>) -> CoreResult<()> {
        for (sender, var) in [
            (Sender::Main, "PDS_EMAIL_SMTP_URL"),
            (Sender::Moderation, "PDS_MODERATION_EMAIL_SMTP_URL"),
        ] {
            if let Some(v) = env.get(var).map(|v| v.trim()).filter(|v| !v.is_empty())
                && self.url(sender).await?.as_deref() != Some(v)
            {
                let mut tx = self.store.begin().await?;
                tx.put_secret(smtp_secret_name(sender), v.as_bytes(), &self.key)
                    .await?;
                tx.commit().await?;
                tracing::info!(sender = sender.as_str(), "SMTP URL applied from {var}");
            }
            if self.url(sender).await?.is_some() {
                match self.check(sender).await? {
                    SenderStatus::Failed => tracing::warn!(
                        sender = sender.as_str(),
                        "SMTP check failed; email features are unavailable until it passes"
                    ),
                    s => tracing::info!(
                        sender = sender.as_str(),
                        status = s.as_str(),
                        "SMTP checked"
                    ),
                }
            }
        }
        Ok(())
    }

    /// Sends `email` to `to` from `from`.
    pub async fn send(
        &self,
        sender: Sender,
        from: &str,
        to: &str,
        email: Email,
    ) -> Result<(), MailError> {
        if let Mode::Capture(sent) = &self.mode {
            sent.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(Sent {
                    sender,
                    from: from.to_owned(),
                    to: to.to_owned(),
                    email,
                });
            return Ok(());
        }
        let store_err = |e: locus_core::CoreError| MailError::Send(e.to_string());
        let sender = self
            .effective(sender)
            .await
            .map_err(store_err)?
            .ok_or(MailError::NotConfigured)?;
        let url = self
            .url(sender)
            .await
            .map_err(store_err)?
            .ok_or(MailError::NotConfigured)?;
        let message = Message::builder()
            .from(
                from.parse::<Mailbox>()
                    .map_err(|e| MailError::Address(format!("from {from:?}: {e}")))?,
            )
            .to(to
                .parse::<Mailbox>()
                .map_err(|e| MailError::Address(format!("to {to:?}: {e}")))?)
            .subject(email.subject)
            .multipart(MultiPart::alternative_plain_html(email.text, email.html))
            .map_err(|e| MailError::Send(e.to_string()))?;
        let result = match transport(&url) {
            Ok(t) => t.send(message).await.map(|_| ()).map_err(|e| e.to_string()),
            Err(e) => Err(e),
        };
        if let Err(e) = &result {
            tracing::warn!(sender = sender.as_str(), "email send failed: {e}");
            let _ = self.store.count_send_failure(sender, e).await;
        }
        result.map_err(MailError::Send)
    }
}

/// An SMTP transport from a reference-style URL: `smtps://` is TLS from the
/// start (port 465 by default); `smtp://` upgrades with STARTTLS when the
/// server offers it (port 587 by default), as the reference's mailer does.
/// Credentials come from the URL's user info.
fn transport(url: &str) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
    let u = url::Url::parse(url).map_err(|e| format!("invalid SMTP URL: {e}"))?;
    let host = u.host_str().ok_or("SMTP URL has no host")?.to_owned();
    let tls = TlsParameters::new(host.clone()).map_err(|e| e.to_string())?;
    let (port, tls) = match u.scheme() {
        "smtps" => (u.port().unwrap_or(465), Tls::Wrapper(tls)),
        "smtp" => (u.port().unwrap_or(587), Tls::Opportunistic(tls)),
        other => return Err(format!("unsupported SMTP URL scheme {other:?}")),
    };
    let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&host)
        .port(port)
        .tls(tls)
        .timeout(Some(SMTP_TIMEOUT));
    if !u.username().is_empty() {
        builder = builder.credentials(Credentials::new(
            percent_decode(u.username()),
            percent_decode(u.password().unwrap_or("")),
        ));
    }
    Ok(builder.build())
}

/// `%XX` decoding for URL user info (`+` stays a plus).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn smtp_urls_parse_as_the_reference_reads_them() {
        assert!(transport("smtps://user:p%40ss@smtp.example.net").is_ok());
        assert!(transport("smtp://smtp.example.net:2525").is_ok());
        assert!(transport("http://smtp.example.net").is_err());
        assert!(transport("not a url").is_err());
        assert_eq!(percent_decode("p%40ss+word%2"), "p@ss+word%2");
    }
}
