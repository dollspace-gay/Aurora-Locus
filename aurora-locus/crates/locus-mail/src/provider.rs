//! Email settings as people enter them: an email provider and its
//! credentials, another SMTP server's details, or a whole SMTP URL, turned
//! into the SMTP URL the mailer stores. The setup wizard, the console and
//! `aurora-locus setup` share these, and the forms are built from
//! [`form_fields`].

/// An email provider whose SMTP details are known, so people only enter
/// credentials.
#[derive(Debug)]
pub struct Preset {
    /// The form value (`provider=<id>`), and the prefix of its fields.
    pub id: &'static str,
    pub name: &'static str,
    host: Host,
    login: Login,
    /// The usual port and whether it's STARTTLS (else TLS from the start).
    port: (u16, bool),
    /// The alternate port, for servers whose host blocks the usual ones.
    alt_port: (u16, bool),
}

#[derive(Debug)]
enum Host {
    Fixed(&'static str),
    /// Mailgun: `us` or `eu`.
    MailgunRegion,
    /// Amazon SES: `email-smtp.<region>.amazonaws.com`.
    SesRegion,
}

#[derive(Debug)]
enum Login {
    /// A fixed username and a key as the password.
    Key {
        user: &'static str,
        label: &'static str,
    },
    /// One token as both username and password.
    Token { label: &'static str },
    /// A username and password from the provider.
    UserPass {
        user_label: &'static str,
        pass_label: &'static str,
    },
}

/// The providers offered, in the order shown.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "resend",
        name: "Resend",
        host: Host::Fixed("smtp.resend.com"),
        login: Login::Key {
            user: "resend",
            label: "API key",
        },
        port: (465, false),
        alt_port: (2465, false),
    },
    Preset {
        id: "sendgrid",
        name: "SendGrid",
        host: Host::Fixed("smtp.sendgrid.net"),
        login: Login::Key {
            user: "apikey",
            label: "API key",
        },
        port: (465, false),
        alt_port: (2525, true),
    },
    Preset {
        id: "mailgun",
        name: "Mailgun",
        host: Host::MailgunRegion,
        login: Login::UserPass {
            user_label: "SMTP username",
            pass_label: "SMTP password",
        },
        port: (465, false),
        alt_port: (2525, true),
    },
    Preset {
        id: "postmark",
        name: "Postmark",
        host: Host::Fixed("smtp.postmarkapp.com"),
        login: Login::Token {
            label: "Server API token",
        },
        port: (587, true),
        alt_port: (2525, true),
    },
    Preset {
        id: "ses",
        name: "Amazon SES",
        host: Host::SesRegion,
        login: Login::UserPass {
            user_label: "SMTP username",
            pass_label: "SMTP password",
        },
        port: (465, false),
        alt_port: (2465, false),
    },
    Preset {
        id: "brevo",
        name: "Brevo",
        host: Host::Fixed("smtp-relay.brevo.com"),
        login: Login::UserPass {
            user_label: "SMTP login",
            pass_label: "SMTP key",
        },
        port: (465, false),
        alt_port: (2525, true),
    },
];

/// The two choices besides the presets.
pub const OTHER: &str = "other";
pub const URL: &str = "url";

/// The provider choices for the form's select: (value, label).
#[must_use]
pub fn provider_options() -> Vec<(&'static str, &'static str)> {
    PRESETS
        .iter()
        .map(|p| (p.id, p.name))
        .chain([
            (OTHER, "Other (an SMTP server's details)"),
            (URL, "An SMTP URL"),
        ])
        .collect()
}

/// A field of the email form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormField {
    pub name: String,
    pub label: String,
    /// text, password, number, select or checkbox.
    pub kind: &'static str,
    pub value: String,
    /// For a select: (value, label).
    pub options: Vec<(&'static str, &'static str)>,
    pub hint: String,
    /// The provider choices it belongs to, space-separated (the page shows
    /// it only for those).
    pub show_for: String,
}

fn field(name: String, label: &str, kind: &'static str, show_for: &str) -> FormField {
    FormField {
        name,
        label: label.to_owned(),
        kind,
        value: String::new(),
        options: Vec::new(),
        hint: String::new(),
        show_for: show_for.to_owned(),
    }
}

/// The email form's fields after the provider select: each preset's
/// credentials (`<id>_key`, `<id>_username`, ...), the alternate-port box,
/// another server's details, and the URL.
#[must_use]
pub fn form_fields() -> Vec<FormField> {
    let mut out = Vec::new();
    for p in PRESETS {
        match p.host {
            Host::Fixed(_) => {}
            Host::MailgunRegion => out.push(FormField {
                options: vec![
                    ("us", "US (smtp.mailgun.org)"),
                    ("eu", "EU (smtp.eu.mailgun.org)"),
                ],
                ..field(format!("{}_region", p.id), "Region", "select", p.id)
            }),
            Host::SesRegion => out.push(FormField {
                hint: "The AWS region your SES account sends from, like us-east-1.".into(),
                ..field(format!("{}_region", p.id), "Region", "text", p.id)
            }),
        }
        match p.login {
            Login::Key { label, .. } => {
                out.push(field(format!("{}_key", p.id), label, "password", p.id));
            }
            Login::Token { label } => {
                out.push(field(format!("{}_token", p.id), label, "password", p.id));
            }
            Login::UserPass {
                user_label,
                pass_label,
            } => {
                out.push(field(
                    format!("{}_username", p.id),
                    user_label,
                    "text",
                    p.id,
                ));
                out.push(field(
                    format!("{}_password", p.id),
                    pass_label,
                    "password",
                    p.id,
                ));
            }
        }
    }
    let presets: Vec<&str> = PRESETS.iter().map(|p| p.id).collect();
    out.push(FormField {
        hint: "Some hosting providers block outgoing mail on ports 25, 465 and 587; this uses the provider's alternate port.".into(),
        ..field(
            "alt_port".into(),
            "My server's host blocks the usual mail ports",
            "checkbox",
            &presets.join(" "),
        )
    });
    out.push(FormField {
        hint: "Like smtp.example.com.".into(),
        ..field("host".into(), "Host", "text", OTHER)
    });
    out.push(FormField {
        value: "465".into(),
        hint: "If your server's host blocks the usual mail ports (25, 465, 587), use your email provider's alternate port.".into(),
        ..field("port".into(), "Port", "number", OTHER)
    });
    out.push(FormField {
        options: vec![
            ("tls", "TLS from the start (usually 465)"),
            ("starttls", "STARTTLS (usually 587)"),
        ],
        ..field("security".into(), "Security", "select", OTHER)
    });
    out.push(field("username".into(), "Username", "text", OTHER));
    out.push(field("password".into(), "Password", "password", OTHER));
    out.push(FormField {
        hint: "smtps://user:password@smtp.example.com, as PDS_EMAIL_SMTP_URL takes it.".into(),
        ..field("smtp_url".into(), "SMTP URL", "password", URL)
    });
    out
}

/// What the email form asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmtpChoice {
    /// A known provider with its fields' values (by field name).
    Preset {
        preset: &'static str,
        region: String,
        username: String,
        password: String,
        alt_port: bool,
    },
    /// Another SMTP server's details. `starttls`: plain connection upgraded
    /// with STARTTLS; otherwise TLS from the start.
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
    /// Reads the form's fields (see [`form_fields`]); `get` returns "" for a
    /// missing field. A form without `provider` (only `smtp_url`) is a URL.
    pub fn from_form(get: impl Fn(&str) -> String) -> Self {
        let provider = get("provider");
        if let Some(p) = PRESETS.iter().find(|p| p.id == provider) {
            let g = |f: &str| get(&format!("{}_{f}", p.id));
            let (username, password) = match p.login {
                Login::Key { user, .. } => {
                    let key = g("key").trim().to_owned();
                    (
                        if key.is_empty() {
                            String::new()
                        } else {
                            user.to_owned()
                        },
                        key,
                    )
                }
                Login::Token { .. } => {
                    let t = g("token").trim().to_owned();
                    (t.clone(), t)
                }
                Login::UserPass { .. } => (g("username").trim().to_owned(), g("password")),
            };
            return Self::Preset {
                preset: p.id,
                region: g("region").trim().to_ascii_lowercase(),
                username,
                password,
                alt_port: !get("alt_port").is_empty(),
            };
        }
        match provider.as_str() {
            OTHER => Self::Server {
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
            Self::Preset {
                username, password, ..
            } => username.is_empty() && password.is_empty(),
            Self::Server { host, .. } => host.is_empty(),
            Self::Url(u) => u.is_empty(),
        }
    }

    /// The SMTP URL, or what's wrong with the entry.
    pub fn smtp_url(&self) -> Result<String, String> {
        match self {
            Self::Preset {
                preset,
                region,
                username,
                password,
                alt_port,
            } => {
                let p = PRESETS
                    .iter()
                    .find(|p| p.id == *preset)
                    .ok_or("Pick an email provider.")?;
                let host = match p.host {
                    Host::Fixed(h) => h.to_owned(),
                    Host::MailgunRegion => match region.as_str() {
                        "eu" => "smtp.eu.mailgun.org".to_owned(),
                        _ => "smtp.mailgun.org".to_owned(),
                    },
                    Host::SesRegion => {
                        if region.is_empty()
                            || !region
                                .chars()
                                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                        {
                            return Err(
                                "Enter the AWS region your SES account sends from, like us-east-1."
                                    .into(),
                            );
                        }
                        format!("email-smtp.{region}.amazonaws.com")
                    }
                };
                match p.login {
                    Login::Key { label, .. } | Login::Token { label } if password.is_empty() => {
                        return Err(format!("Enter the {} {label}.", p.name));
                    }
                    Login::UserPass {
                        user_label,
                        pass_label,
                    } if username.is_empty() || password.is_empty() => {
                        return Err(format!(
                            "Enter the {} {} and {}.",
                            p.name, user_label, pass_label
                        ));
                    }
                    _ => {}
                }
                let (port, starttls) = if *alt_port { p.alt_port } else { p.port };
                build(
                    if starttls { "smtp" } else { "smtps" },
                    &host,
                    port,
                    username,
                    password,
                )
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
                build(
                    if *starttls { "smtp" } else { "smtps" },
                    host,
                    port,
                    username,
                    password,
                )
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

/// The mail ports some hosting providers block for outgoing connections.
const USUAL_PORTS: [u16; 3] = [25, 465, 587];

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
         Tick \"My server's host blocks the usual mail ports\" to use your email provider's alternate port, or enter that port yourself.",
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

    fn url(pairs: &[(&str, &str)]) -> String {
        form(pairs).smtp_url().unwrap()
    }

    #[test]
    fn each_provider_needs_only_its_credentials() {
        assert_eq!(
            url(&[("provider", "resend"), ("resend_key", " re_abc123 ")]),
            "smtps://resend:re_abc123@smtp.resend.com:465"
        );
        assert_eq!(
            url(&[
                ("provider", "resend"),
                ("resend_key", "re_abc123"),
                ("alt_port", "1")
            ]),
            "smtps://resend:re_abc123@smtp.resend.com:2465"
        );
        assert_eq!(
            url(&[
                ("provider", "sendgrid"),
                ("sendgrid_key", "SG.k"),
                ("alt_port", "1")
            ]),
            "smtp://apikey:SG.k@smtp.sendgrid.net:2525"
        );
        assert_eq!(
            url(&[
                ("provider", "mailgun"),
                ("mailgun_region", "eu"),
                ("mailgun_username", "postmaster@mg.example.com"),
                ("mailgun_password", "pw"),
            ]),
            "smtps://postmaster%40mg.example.com:pw@smtp.eu.mailgun.org:465"
        );
        assert_eq!(
            url(&[("provider", "postmark"), ("postmark_token", "tok-1")]),
            "smtp://tok-1:tok-1@smtp.postmarkapp.com:587"
        );
        assert_eq!(
            url(&[
                ("provider", "ses"),
                ("ses_region", "EU-West-1"),
                ("ses_username", "AKIA1"),
                ("ses_password", "secret/+="),
                ("alt_port", "1"),
            ]),
            "smtps://AKIA1:secret%2F+%3D@email-smtp.eu-west-1.amazonaws.com:2465"
        );
        assert_eq!(
            url(&[
                ("provider", "brevo"),
                ("brevo_username", "me@example.com"),
                ("brevo_password", "xsmtpsib-1"),
            ]),
            "smtps://me%40example.com:xsmtpsib-1@smtp-relay.brevo.com:465"
        );
        // Another provider's fields don't count.
        let wrong = form(&[("provider", "resend"), ("sendgrid_key", "SG.k")]);
        assert!(wrong.is_empty());
        assert_eq!(wrong.smtp_url().unwrap_err(), "Enter the Resend API key.");
        assert!(
            form(&[
                ("provider", "ses"),
                ("ses_username", "a"),
                ("ses_password", "b")
            ])
            .smtp_url()
            .is_err()
        );
        assert!(
            form(&[("provider", "brevo"), ("brevo_username", "a")])
                .smtp_url()
                .is_err()
        );
    }

    #[test]
    fn every_preset_has_its_fields_and_an_alternate_port() {
        let fields = form_fields();
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        for p in PRESETS {
            assert!(
                names.iter().any(|n| n.starts_with(&format!("{}_", p.id))),
                "{}",
                p.id
            );
            assert!(!USUAL_PORTS.contains(&p.alt_port.0), "{}", p.id);
            assert!(
                fields
                    .iter()
                    .any(|f| f.name == "alt_port" && f.show_for.split(' ').any(|s| s == p.id)),
                "{}",
                p.id
            );
        }
        // Every field is shown for some choice the select offers.
        let choices: Vec<&str> = provider_options().iter().map(|(v, _)| *v).collect();
        for f in &fields {
            assert!(
                f.show_for.split(' ').all(|s| choices.contains(&s)),
                "{}",
                f.name
            );
        }
    }

    #[test]
    fn server_details_become_an_encoded_url() {
        let url = url(&[
            ("provider", "other"),
            ("host", "smtp.example.com"),
            ("port", "2587"),
            ("security", "starttls"),
            ("username", "me@example.com"),
            ("password", "p@ss:w/rd"),
        ]);
        assert!(
            url.starts_with("smtp://me%40example.com:p%40ss%3Aw%2Frd@smtp.example.com:2587"),
            "{url}"
        );
        // The mailer reads it back as entered.
        assert!(crate::transport(&url).is_ok());
        let u = url::Url::parse(&url).unwrap();
        assert_eq!(crate::percent_decode(u.password().unwrap()), "p@ss:w/rd");
        assert_eq!(crate::percent_decode(u.username()), "me@example.com");
        assert_eq!(
            form(&[
                ("provider", "other"),
                ("host", "mail.example.net"),
                ("port", "465")
            ])
            .smtp_url()
            .unwrap(),
            "smtps://mail.example.net:465"
        );
        for (host, port) in [
            ("smtp://x.com", "25"),
            ("x.com:25", "25"),
            ("x.com", "0"),
            ("x.com", "abc"),
            ("x.com", "70000"),
        ] {
            assert!(
                form(&[("provider", "other"), ("host", host), ("port", port)])
                    .smtp_url()
                    .is_err(),
                "{host} {port}"
            );
        }
        assert!(
            form(&[
                ("provider", "other"),
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
            url(&[("smtp_url", "smtps://u:p@smtp.example.com")]),
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
