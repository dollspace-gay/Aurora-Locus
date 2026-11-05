//! The built-in email templates: a subject, a text body and an HTML body
//! each, with `{{name}}` placeholders. The service name and the token are
//! filled in; HTML bodies get escaped values.

/// The emails locus sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    ConfirmEmail,
    UpdateEmail,
    ResetPassword,
    DeleteAccount,
    PlcOperation,
    SignInCode,
}

/// A rendered email.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    pub subject: String,
    pub text: String,
    pub html: String,
}

struct Parts {
    subject: &'static str,
    intro: &'static str,
    outro: &'static str,
}

impl Template {
    /// Subjects as the reference's.
    fn parts(self) -> Parts {
        match self {
            Self::ConfirmEmail => Parts {
                subject: "Email Confirmation",
                intro: "Use this code to confirm your email address on {{service}}:",
                outro: "If you didn't ask for this, you can ignore this email.",
            },
            Self::UpdateEmail => Parts {
                subject: "Email Update Requested",
                intro: "Use this code to change the email address of your {{service}} account:",
                outro: "If you didn't ask to change your email address, you can ignore this email.",
            },
            Self::ResetPassword => Parts {
                subject: "Password Reset Requested",
                intro: "Use this code to reset the password of {{handle}} on {{service}}:",
                outro: "If you didn't ask to reset your password, you can ignore this email; your password hasn't changed.",
            },
            Self::DeleteAccount => Parts {
                subject: "Account Deletion Requested",
                intro: "Use this code to delete your {{service}} account. Deleting it can't be undone:",
                outro: "If you didn't ask to delete your account, change your password now.",
            },
            Self::PlcOperation => Parts {
                subject: "PLC Update Operation Requested",
                intro: "Use this code to sign an update to your account's identity (for example, to move it to another server) on {{service}}:",
                outro: "If you didn't ask for this, don't share the code with anyone, and change your password.",
            },
            Self::SignInCode => Parts {
                subject: "Sign-in Confirmation",
                intro: "Use this code to finish signing in as {{handle}} on {{service}}:",
                outro: "If this wasn't you, change your password now.",
            },
        }
    }

    /// Renders the template. `vars` holds `(name, value)` pairs; `service`,
    /// `token` and `handle` are the ones the built-in templates use.
    #[must_use]
    pub fn render(self, vars: &[(&str, &str)]) -> Email {
        let p = self.parts();
        let fill = |s: &str, escape: bool| {
            let mut out = s.to_owned();
            for (k, v) in vars {
                let v = if escape {
                    html_escape(v)
                } else {
                    (*v).to_owned()
                };
                out = out.replace(&format!("{{{{{k}}}}}"), &v);
            }
            out
        };
        let token = vars
            .iter()
            .find(|(k, _)| *k == "token")
            .map_or("", |(_, v)| v);
        let text = format!(
            "{}\n\n    {token}\n\nThe code expires in 15 minutes.\n\n{}\n",
            fill(p.intro, false),
            fill(p.outro, false)
        );
        let html = format!(
            "<!doctype html><html><body style=\"font-family: sans-serif; line-height: 1.5\">\
             <p>{}</p><p style=\"font-size: 1.5em; font-family: monospace; letter-spacing: 0.1em\"><strong>{}</strong></p>\
             <p>The code expires in 15 minutes.</p><p>{}</p></body></html>",
            fill(p.intro, true),
            html_escape(token),
            fill(p.outro, true)
        );
        Email {
            subject: fill(p.subject, false),
            text,
            html,
        }
    }
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_fill_and_escape() {
        let e = Template::ResetPassword.render(&[
            ("service", "Test <PDS>"),
            ("handle", "alice.test"),
            ("token", "ABCDE-FGHIJ"),
        ]);
        assert_eq!(e.subject, "Password Reset Requested");
        assert!(e.text.contains("alice.test on Test <PDS>"));
        assert!(e.text.contains("ABCDE-FGHIJ"));
        assert!(e.html.contains("Test &lt;PDS&gt;"));
        assert!(!e.html.contains("<PDS>"));
        assert!(!e.text.contains("{{"), "every placeholder filled");
    }
}
