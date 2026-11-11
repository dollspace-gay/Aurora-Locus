//! Email settings as people enter them: an email provider's API key, or an
//! SMTP server's details, turned into the SMTP URL the mailer stores. The
//! setup wizard, the console and `aurora-locus setup` share these.

/// Resend's SMTP server: the username is always `resend`, the password the
/// API key.
pub const RESEND_HOST: &str = "smtp.resend.com";
/// Resend's TLS port, and the alternate for hosts that block the usual ones.
pub const RESEND_PORT: u16 = 465;
pub const RESEND_ALT_PORT: u16 = 2465;

/// The mail ports some hosting providers block for outgoing connections.
const USUAL_PORTS: [u16; 3] = [25, 465, 587];

/// What the email form asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmtpChoice {
    /// Resend, by API key; `alt_port` for hosts that block the usual ports.
    Resend { api_key: String, alt_port: bool },
    /// An SMTP server's details. `starttls`: plain connection upgraded with
    /// STARTTLS; otherwise TLS from the start.
    Server {
        host: String,
        port: String,
        starttls: bool,
        username: String,
        password: String,
    },
    /// A full SMTP URL, as `PDS_EMAIL_SMTP_URL` takes it.
    Url(String),
}

impl SmtpChoice {
    /// Reads the form's fields: `provider` (`resend`, `server` or `url`),
    /// then `api_key` and `alt_port`; `host`, `port`, `security`
    /// (`starttls` or `tls`), `username` and `password`; or `smtp_url`.
    /// `get` returns "" for a missing field. A form with only `smtp_url`
    /// (no `provider`) is a URL.
    pub fn from_form(get: impl Fn(&str) -> String) -> Self {
        match get("provider").as_str() {
            "resend" => Self::Resend {
                api_key: get("api_key").trim().to_owned(),
                alt_port: !get("alt_port").is_empty(),
            },
            "server" => Self::Server {
                host: get("host").trim().to_owned(),
                port: get("port").trim().to_owned(),
                starttls: get("security") == "starttls",
                username: get("username").trim().to_owned(),
                password: get("password"),
            },
            _ => Self::Url(get("smtp_url").trim().to_owned()),
        }
    }

    /// Whether nothing was entered (a form left empty keeps what's saved).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Resend { api_key, .. } => api_key.is_empty(),
            Self::Server { host, .. } => host.is_empty(),
            Self::Url(u) => u.is_empty(),
        }
    }

    /// The SMTP URL, or what's wrong with the entry.
    pub fn smtp_url(&self) -> Result<String, String> {
        match self {
            Self::Resend { api_key, alt_port } => {
                if api_key.is_empty() {
                    return Err("Enter the Resend API key.".into());
                }
                let port = if *alt_port {
                    RESEND_ALT_PORT
                } else {
                    RESEND_PORT
                };
                build("smtps", RESEND_HOST, port, "resend", api_key)
            }
            Self::Server {
                host,
                port,
                starttls,
                username,
                password,
            } => {
                if host.is_empty() {
                    return Err("Enter the SMTP server's host name.".into());
                }
                if host.contains("://") || host.contains(['/', ' ', '@', ':']) {
                    return Err(
                        "The host is a name like smtp.example.com, without a scheme, port or path."
                            .into(),
                    );
                }
                let port: u16 = match port.parse() {
                    Ok(p) if p > 0 => p,
                    _ => return Err("The port is a number from 1 to 65535.".into()),
                };
                if username.is_empty() && !password.is_empty() {
                    return Err("A password needs a username.".into());
                }
                let scheme = if *starttls { "smtp" } else { "smtps" };
                build(scheme, host, port, username, password)
            }
            Self::Url(u) => {
                if u.starts_with("smtp://") || u.starts_with("smtps://") {
                    Ok(u.clone())
                } else {
                    Err("An SMTP URL starts with smtp:// or smtps://.".into())
                }
            }
        }
    }
}

fn build(scheme: &str, host: &str, port: u16, user: &str, pass: &str) -> Result<String, String> {
    let mut u = url::Url::parse(&format!("{scheme}://{host}:{port}"))
        .map_err(|_| format!("{host} isn't a host name."))?;
    if !user.is_empty() {
        // set_username/set_password percent-encode, which the mailer decodes.
        u.set_username(user)
            .map_err(|()| "That username can't be used.".to_owned())?;
        u.set_password(Some(pass))
            .map_err(|()| "That password can't be used.".to_owned())?;
    }
    Ok(u.to_string())
}

/// When a check failed to connect on one of the usual mail ports, a hint
/// that the server's host may block it. `error` is the failure the mailer
/// recorded.
#[must_use]
pub fn blocked_port_hint(url: &str, error: &str) -> Option<&'static str> {
    let u = url::Url::parse(url).ok()?;
    let port = u
        .port()
        .unwrap_or(if u.scheme() == "smtps" { 465 } else { 587 });
    let e = error.to_ascii_lowercase();
    let unreachable = ["timed out", "timeout", "refused", "unreachable"]
        .iter()
        .any(|w| e.contains(w));
    (USUAL_PORTS.contains(&port) && unreachable).then_some(
        "Some hosting providers block outgoing mail on ports 25, 465 and 587. \
         Try your email provider's alternate port (for Resend, tick \"My server's host blocks the usual mail ports\").",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn form(pairs: &[(&str, &str)]) -> SmtpChoice {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        SmtpChoice::from_form(|k| m.get(k).cloned().unwrap_or_default())
    }

    #[test]
    fn resend_needs_only_the_key() {
        assert_eq!(
            form(&[("provider", "resend"), ("api_key", " re_abc123 ")])
                .smtp_url()
                .unwrap(),
            "smtps://resend:re_abc123@smtp.resend.com:465"
        );
        assert_eq!(
            form(&[
                ("provider", "resend"),
                ("api_key", "re_abc123"),
                ("alt_port", "1")
            ])
            .smtp_url()
            .unwrap(),
            "smtps://resend:re_abc123@smtp.resend.com:2465"
        );
        let empty = form(&[("provider", "resend")]);
        assert!(empty.is_empty());
        assert!(empty.smtp_url().is_err());
    }

    #[test]
    fn server_details_become_an_encoded_url() {
        let url = form(&[
            ("provider", "server"),
            ("host", "smtp.example.com"),
            ("port", "2587"),
            ("security", "starttls"),
            ("username", "me@example.com"),
            ("password", "p@ss:w/rd"),
        ])
        .smtp_url()
        .unwrap();
        assert!(
            url.starts_with("smtp://me%40example.com:p%40ss%3Aw%2Frd@smtp.example.com:2587"),
            "{url}"
        );
        // The mailer reads it back as entered.
        assert!(crate::transport(&url).is_ok());
        let u = url::Url::parse(&url).unwrap();
        assert_eq!(crate::percent_decode(u.password().unwrap()), "p@ss:w/rd");
        assert_eq!(crate::percent_decode(u.username()), "me@example.com");

        let tls = form(&[
            ("provider", "server"),
            ("host", "mail.example.net"),
            ("port", "465"),
        ]);
        assert_eq!(tls.smtp_url().unwrap(), "smtps://mail.example.net:465");
        for (host, port) in [
            ("smtp://x.com", "25"),
            ("x.com:25", "25"),
            ("x.com", "0"),
            ("x.com", "abc"),
            ("x.com", "70000"),
        ] {
            assert!(
                form(&[("provider", "server"), ("host", host), ("port", port)])
                    .smtp_url()
                    .is_err(),
                "{host} {port}"
            );
        }
        assert!(
            form(&[
                ("provider", "server"),
                ("host", "x.com"),
                ("port", "25"),
                ("password", "p")
            ])
            .smtp_url()
            .is_err()
        );
    }

    #[test]
    fn a_bare_url_still_works() {
        assert_eq!(
            form(&[("smtp_url", "smtps://u:p@smtp.example.com")])
                .smtp_url()
                .unwrap(),
            "smtps://u:p@smtp.example.com"
        );
        assert!(
            form(&[("provider", "url"), ("smtp_url", "http://x")])
                .smtp_url()
                .is_err()
        );
    }

    #[test]
    fn the_hint_is_for_connection_failures_on_usual_ports() {
        let timed_out = "Connection error: connection timed out";
        assert!(blocked_port_hint("smtps://u:p@smtp.resend.com", timed_out).is_some());
        assert!(
            blocked_port_hint(
                "smtp://u:p@smtp.example.com:587",
                "Connection refused (os error 111)"
            )
            .is_some()
        );
        assert!(blocked_port_hint("smtp://h:25", timed_out).is_some());
        // An alternate port, or a sign-in failure: no hint.
        assert!(blocked_port_hint("smtps://u:p@smtp.resend.com:2465", timed_out).is_none());
        assert!(
            blocked_port_hint(
                "smtps://u:p@smtp.resend.com",
                "permanent error (535): authentication failed"
            )
            .is_none()
        );
        // The hint names no hosting provider.
        let hint = blocked_port_hint("smtps://h", timed_out).unwrap();
        assert!(!hint.to_ascii_lowercase().contains("ovh"));
    }
}
