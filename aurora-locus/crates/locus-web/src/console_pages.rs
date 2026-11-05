//! The console's pages: sign-in, Home, settings, the operator's own pages,
//! and the account portal switch.

use askama::Template;

use crate::setup::Choice;
use crate::view::Shell;

/// A label and a value.
#[derive(Debug, Clone)]
pub struct Fact {
    pub label: String,
    pub value: String,
    /// "", "ok", "warn" or "bad".
    pub class: &'static str,
}

impl Fact {
    #[must_use]
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            class: "",
        }
    }

    #[must_use]
    pub fn class(mut self, class: &'static str) -> Self {
        self.class = class;
        self
    }
}

/// One setting as a form field.
#[derive(Debug, Clone)]
pub struct SettingField {
    pub key: String,
    pub description: String,
    /// checkbox, number, select, textarea or text.
    pub input: &'static str,
    pub value: String,
    pub checked: bool,
    pub options: Vec<Choice>,
    pub min: String,
    pub max: String,
    pub default: String,
    pub is_default: bool,
    /// The reference env var it maps from, if any.
    pub env: String,
}

/// Settings under one heading.
#[derive(Debug, Clone)]
pub struct SettingGroup {
    pub key: String,
    pub label: String,
    pub fields: Vec<SettingField>,
}

/// The sign-in page: username and password, then the sign-in code.
#[derive(Template)]
#[template(path = "console_sign_in.html")]
pub struct SignInPage {
    pub shell: Shell,
    pub csrf: String,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub username: String,
    /// Asking for the sign-in code (the password was right).
    pub code_stage: bool,
}

/// A row of the jobs table.
#[derive(Debug, Clone)]
pub struct JobRow {
    pub name: String,
    pub last_run: String,
    pub state: String,
    pub class: &'static str,
    pub error: String,
}

/// Home: how the PDS is doing.
#[derive(Template)]
#[template(path = "console_home.html")]
pub struct HomePage {
    pub shell: Shell,
    pub attention: Vec<Fact>,
    pub facts: Vec<Fact>,
    pub counts: Vec<Fact>,
    pub jobs: Vec<JobRow>,
}

/// Every setting, with search.
#[derive(Template)]
#[template(path = "console_settings.html")]
pub struct SettingsPage {
    pub shell: Shell,
    pub csrf: String,
    pub q: String,
    pub groups: Vec<SettingGroup>,
    pub can_edit: bool,
    pub back: String,
    pub notice: Option<String>,
    pub errors: Vec<String>,
}

/// The operator's own look of the console.
#[derive(Template)]
#[template(path = "console_appearance.html")]
pub struct AppearancePage {
    pub shell: Shell,
    pub csrf: String,
    pub modes: Vec<Choice>,
    pub schemes: Vec<Choice>,
    pub fonts: Vec<Choice>,
    pub saved: bool,
}

/// A sign-in code app being added.
#[derive(Debug, Clone)]
pub struct Enrolling {
    pub secret: String,
    pub uri: String,
}

/// One of the operator's sign-ins.
#[derive(Debug, Clone)]
pub struct SessionView {
    pub id: String,
    pub started: String,
    pub last_used: String,
    pub from: String,
    pub this_one: bool,
}

/// Password, sign-in code app and sign-ins.
#[derive(Template)]
#[template(path = "console_security.html")]
pub struct SecurityPage {
    pub shell: Shell,
    pub csrf: String,
    pub totp_on: bool,
    pub enrolling: Option<Enrolling>,
    pub sessions: Vec<SessionView>,
    pub notice: Option<String>,
    pub errors: Vec<String>,
}

/// Confirm the password (and code) before a sensitive action.
#[derive(Template)]
#[template(path = "console_reauth.html")]
pub struct ReauthPage {
    pub shell: Shell,
    pub csrf: String,
    pub next: String,
    pub totp: bool,
    pub error: Option<String>,
}

/// The account portal's switch and its settings.
#[derive(Template)]
#[template(path = "console_portal.html")]
pub struct PortalPage {
    pub shell: Shell,
    pub csrf: String,
    pub enabled: bool,
    pub url: String,
    pub can_edit: bool,
    pub back: String,
    pub fields: Vec<SettingField>,
    pub notice: Option<String>,
    pub errors: Vec<String>,
}
