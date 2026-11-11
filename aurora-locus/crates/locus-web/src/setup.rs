//! The setup wizard's pages: the token page, then one page per step, each
//! saving its answers before the next.

use askama::Template;

use crate::view::Shell;

/// The wizard's steps, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Hostname,
    Storage,
    Proxy,
    Handles,
    Operator,
    Email,
    Services,
    Private,
    Keys,
    Review,
}

impl Step {
    pub const ALL: [Self; 10] = [
        Self::Hostname,
        Self::Storage,
        Self::Proxy,
        Self::Handles,
        Self::Operator,
        Self::Email,
        Self::Services,
        Self::Private,
        Self::Keys,
        Self::Review,
    ];

    #[must_use]
    pub fn slug(self) -> &'static str {
        match self {
            Self::Hostname => "hostname",
            Self::Storage => "storage",
            Self::Proxy => "proxy",
            Self::Handles => "handles",
            Self::Operator => "operator",
            Self::Email => "email",
            Self::Services => "services",
            Self::Private => "private",
            Self::Keys => "keys",
            Self::Review => "review",
        }
    }

    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Hostname => "Hostname",
            Self::Storage => "Storage",
            Self::Proxy => "Proxy and TLS",
            Self::Handles => "Handles",
            Self::Operator => "First operator",
            Self::Email => "Email",
            Self::Services => "Network services",
            Self::Private => "Private posts",
            Self::Keys => "Keys",
            Self::Review => "Review and finish",
        }
    }

    #[must_use]
    pub fn parse(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.slug() == slug)
    }

    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    #[must_use]
    pub fn next(self) -> Option<Self> {
        Self::ALL.get(self.index() + 1).copied()
    }

    #[must_use]
    pub fn href(self) -> String {
        format!("/setup/{}", self.slug())
    }
}

/// One entry of the step list.
#[derive(Debug, Clone)]
pub struct StepLink {
    pub title: &'static str,
    pub href: String,
    pub current: bool,
    pub done: bool,
}

/// The step list with `current` marked and the ones before it done.
#[must_use]
pub fn step_links(current: Step) -> Vec<StepLink> {
    Step::ALL
        .into_iter()
        .map(|s| StepLink {
            title: s.title(),
            href: s.href(),
            current: s == current,
            done: s.index() < current.index(),
        })
        .collect()
}

/// A radio or checkbox option.
#[derive(Debug, Clone)]
pub struct Choice {
    pub value: String,
    pub label: String,
    pub checked: bool,
}

/// A DNS record to create.
#[derive(Debug, Clone)]
pub struct DnsRow {
    pub name: String,
    pub points_to: String,
    pub note: String,
}

/// One final check's result.
#[derive(Debug, Clone)]
pub struct CheckRow {
    pub id: String,
    pub label: String,
    /// pass, warn, fail or skip.
    pub outcome: &'static str,
    pub detail: String,
    pub acked: bool,
}

impl CheckRow {
    #[must_use]
    pub fn class(&self) -> &'static str {
        match self.outcome {
            "pass" => "ok",
            "warn" => "warn",
            "fail" => "bad",
            _ => "muted",
        }
    }

    #[must_use]
    pub fn word(&self) -> &'static str {
        match self.outcome {
            "pass" => "Passed",
            "warn" => "Warning",
            "fail" => "Failed",
            _ => "Not checked",
        }
    }
}

/// Every step's fields; each page shows its own.
#[derive(Debug, Clone, Default)]
pub struct SetupForm {
    // Hostname.
    pub hostname: String,
    pub service_name: String,
    pub contact_email: String,
    pub dev: bool,
    // Storage.
    pub database: String,
    pub blob_store: String,
    pub storage_ok: bool,
    pub storage_detail: String,
    // Proxy.
    pub proxy_options: Vec<Choice>,
    pub trusted_proxies: String,
    // Handles.
    pub handle_options: Vec<Choice>,
    pub other_handle_domains: String,
    pub existing_subdomains: String,
    pub dns: Vec<DnsRow>,
    pub cloudflare: bool,
    // Operator.
    pub username: String,
    pub password_set: bool,
    // Email.
    pub email_status: String,
    pub email_class: &'static str,
    pub email_ready: bool,
    pub smtp_url_set: bool,
    /// The email provider select and every provider's fields.
    pub email_inputs: Vec<crate::console_pages::Input>,
    pub from_address: String,
    pub email_skipped: bool,
    pub test_sent_to: String,
    // Services.
    pub appview_url: String,
    pub appview_did: String,
    pub crawlers: String,
    pub moderation_url: String,
    pub moderation_did: String,
    pub report_url: String,
    pub report_did: String,
    // Private posts.
    pub private_tier: bool,
    pub private_appview_url: String,
    pub private_appview_did: String,
    // Keys.
    pub rotation_key: String,
    pub master_key_source: String,
    pub master_key_id: String,
    pub master_key_path: String,
    pub recovery_did_key: String,
    pub keys_ack: bool,
    // Review.
    pub checks: Vec<CheckRow>,
    pub blocking: bool,
    pub summary: Vec<(String, String)>,
}

/// The first page: the one-time token.
#[derive(Template)]
#[template(path = "setup_token.html")]
pub struct TokenPage {
    pub shell: Shell,
    pub error: Option<String>,
    /// Where the token file is, if this install has one.
    pub token_path: String,
}

/// A step's page.
#[derive(Template)]
#[template(path = "setup.html")]
pub struct StepPage {
    pub shell: Shell,
    pub csrf: String,
    pub step: Step,
    pub steps: Vec<StepLink>,
    pub errors: Vec<String>,
    pub notice: Option<String>,
    pub f: SetupForm,
}

impl StepPage {
    #[must_use]
    pub fn is(&self, slug: &str) -> bool {
        self.step.slug() == slug
    }

    #[must_use]
    pub fn back(&self) -> Option<String> {
        let i = self.step.index();
        (i > 0).then(|| Step::ALL[i - 1].href())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(step: Step) -> String {
        StepPage {
            shell: Shell::signed_out("Set up", "Locus"),
            csrf: "c5rf".into(),
            step,
            steps: step_links(step),
            errors: vec!["Enter the PDS hostname.".into()],
            notice: None,
            f: SetupForm {
                checks: vec![CheckRow {
                    id: "dns".into(),
                    label: "DNS".into(),
                    outcome: "warn",
                    detail: "Not yet.".into(),
                    acked: false,
                }],
                ..SetupForm::default()
            },
        }
        .render()
        .unwrap()
    }

    #[test]
    fn every_step_renders_its_form_and_marks_itself_current() {
        for step in Step::ALL {
            let html = page(step);
            assert!(
                html.contains(&format!(r#"action="/setup/{}""#, step.slug())),
                "{step:?}"
            );
            assert_eq!(
                html.matches(r#"aria-current="step""#).count(),
                1,
                "{step:?}"
            );
            assert!(html.contains(r#"name="csrf" value="c5rf""#), "{step:?}");
            assert!(html.contains("Enter the PDS hostname."), "{step:?}");
        }
        let review = page(Step::Review);
        assert!(
            review.contains(r#"name="ack" value="dns""#),
            "a warning is acknowledged by its id"
        );
        assert_eq!(Step::Review.next(), None);
        assert_eq!(Step::parse("email"), Some(Step::Email));
    }
}
