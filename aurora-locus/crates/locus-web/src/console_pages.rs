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

impl Enrolling {
    /// The `otpauth://` link as a QR code (inline SVG, black on white in
    /// both themes so cameras read it); empty if it can't be encoded.
    #[must_use]
    pub fn qr_svg(&self) -> String {
        let Ok(code) = qrcode::QrCode::new(self.uri.as_bytes()) else {
            return String::new();
        };
        let svg = code
            .render::<qrcode::render::svg::Color<'_>>()
            .min_dimensions(200, 200)
            .dark_color(qrcode::render::svg::Color("#000000"))
            .light_color(qrcode::render::svg::Color("#ffffff"))
            .quiet_zone(true)
            .build();
        // Inline in HTML: drop the XML declaration.
        match svg.find("<svg") {
            Some(i) => svg[i..].to_owned(),
            None => String::new(),
        }
    }
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

/// A row of the accounts table.
#[derive(Debug, Clone)]
pub struct AccountRow {
    pub href: String,
    pub handle: String,
    pub did: String,
    pub email: String,
    pub status: String,
    /// The status pill's class.
    pub class: &'static str,
    pub created: String,
}

/// Accounts: search, and create.
#[derive(Template)]
#[template(path = "console_accounts.html")]
pub struct AccountsPage {
    pub shell: Shell,
    pub csrf: String,
    pub q: String,
    pub rows: Vec<AccountRow>,
    /// The next page's link, if there's more.
    pub more: Option<String>,
    pub can_create: bool,
    /// Handle domains, as `.example.com`, for the create form's hint.
    pub domains: String,
    pub notice: Option<String>,
    pub errors: Vec<String>,
}

/// An invite code as a row.
#[derive(Debug, Clone)]
pub struct InviteRow {
    pub code: String,
    pub available: i64,
    pub uses: usize,
    pub for_account: String,
    pub created: String,
    pub disabled: bool,
}

/// One app password.
#[derive(Debug, Clone)]
pub struct AppPasswordRow {
    pub name: String,
    pub created: String,
    pub privileged: bool,
}

/// One account and what can be done to it.
#[derive(Template)]
#[template(path = "console_account.html")]
pub struct AccountPage {
    pub shell: Shell,
    pub csrf: String,
    pub did: String,
    pub handle: String,
    pub base: String,
    pub facts: Vec<Fact>,
    pub stats: Vec<Fact>,
    pub private: Vec<Fact>,
    pub app_passwords: Vec<AppPasswordRow>,
    pub invites: Vec<InviteRow>,
    pub taken_down: bool,
    pub deactivated: bool,
    pub blocked: bool,
    pub invites_disabled: bool,
    pub email: String,
    pub can_moderate: bool,
    pub can_admin: bool,
    pub notice: Option<String>,
    pub errors: Vec<String>,
}

/// Invite codes.
#[derive(Template)]
#[template(path = "console_invites.html")]
pub struct InvitesPage {
    pub shell: Shell,
    pub csrf: String,
    pub rows: Vec<InviteRow>,
    pub sort: String,
    pub more: Option<String>,
    pub can_moderate: bool,
    pub required: bool,
    pub new_codes: Vec<String>,
    pub notice: Option<String>,
    pub errors: Vec<String>,
}

/// One of an operator's API tokens.
#[derive(Debug, Clone)]
pub struct TokenRow {
    pub id: String,
    pub name: String,
    pub created: String,
    pub last_used: String,
    pub expires: String,
}

/// An operator as a row.
#[derive(Debug, Clone)]
pub struct OperatorView {
    pub username: String,
    pub role: String,
    pub roles: Vec<Choice>,
    pub disabled: bool,
    pub totp: bool,
    pub created: String,
    pub tokens: Vec<TokenRow>,
    pub me: bool,
}

/// Operators and their API tokens (owners only).
#[derive(Template)]
#[template(path = "console_operators.html")]
pub struct OperatorsPage {
    pub shell: Shell,
    pub csrf: String,
    pub rows: Vec<OperatorView>,
    pub roles: Vec<Choice>,
    /// A token just made: its operator and value, shown once.
    pub new_token: Option<(String, String)>,
    pub notice: Option<String>,
    pub errors: Vec<String>,
}

/// A card of facts.
#[derive(Debug, Clone)]
pub struct Card {
    pub title: String,
    pub facts: Vec<Fact>,
}

/// A table cell: text, optionally as code, a link or a pill.
#[derive(Debug, Clone, Default)]
pub struct Cell {
    /// num, when, name, state, actions, list or text.
    pub class: &'static str,
    pub text: String,
    pub code: bool,
    pub href: String,
    /// A pill's class ("on", "off", "amber", …); empty for plain text.
    pub pill: &'static str,
    /// A form button in the cell (actions).
    pub button: Option<Button>,
}

impl Cell {
    #[must_use]
    pub fn new(class: &'static str, text: impl Into<String>) -> Self {
        Self {
            class,
            text: text.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn code(mut self) -> Self {
        self.code = true;
        self
    }

    #[must_use]
    pub fn link(mut self, href: impl Into<String>) -> Self {
        self.href = href.into();
        self
    }

    #[must_use]
    pub fn pill(mut self, class: &'static str) -> Self {
        self.pill = class;
        self
    }
}

/// A one-button form.
#[derive(Debug, Clone)]
pub struct Button {
    pub action: String,
    pub label: String,
    pub hidden: Vec<(String, String)>,
    pub danger: bool,
}

/// A table with a heading.
#[derive(Debug, Clone)]
pub struct Table {
    pub title: String,
    /// (class, label) per column.
    pub cols: Vec<(&'static str, String)>,
    pub rows: Vec<Vec<Cell>>,
    pub empty: String,
}

/// A field of a form.
#[derive(Debug, Clone)]
pub struct Input {
    pub name: String,
    pub label: String,
    /// text, password, email, number, select or textarea.
    pub kind: &'static str,
    pub value: String,
    pub options: Vec<Choice>,
    pub hint: String,
}

impl Input {
    #[must_use]
    pub fn new(name: &str, label: &str, kind: &'static str) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            kind,
            value: String::new(),
            options: Vec::new(),
            hint: String::new(),
        }
    }

    #[must_use]
    pub fn value(mut self, v: impl Into<String>) -> Self {
        self.value = v.into();
        self
    }

    #[must_use]
    pub fn hint(mut self, h: impl Into<String>) -> Self {
        self.hint = h.into();
        self
    }
}

/// A form on a section page.
#[derive(Debug, Clone)]
pub struct Form {
    pub title: String,
    pub text: String,
    pub action: String,
    /// GET (a lookup) or POST.
    pub get: bool,
    pub inputs: Vec<Input>,
    pub hidden: Vec<(String, String)>,
    pub button: String,
    pub danger: bool,
}

impl Form {
    /// Whether it sends a file (and so `multipart/form-data`).
    #[must_use]
    pub fn has_file(&self) -> bool {
        self.inputs.iter().any(|i| i.kind == "file")
    }
}

/// A preview of a page in a frame.
#[derive(Debug, Clone)]
pub struct Frame {
    pub title: String,
    pub src: String,
    /// The frame's width in CSS pixels (a phone's, or a desktop's).
    pub width: u32,
}

/// A console section: what it says, its facts, tables and forms, and its
/// settings (the rarely changed folded away).
#[derive(Template)]
#[template(path = "console_section.html")]
pub struct SectionPage {
    pub shell: Shell,
    pub csrf: String,
    pub intro: String,
    pub notice: Option<String>,
    pub errors: Vec<String>,
    pub cards: Vec<Card>,
    pub forms: Vec<Form>,
    pub tables: Vec<Table>,
    pub fields: Vec<SettingField>,
    pub advanced: Vec<SettingField>,
    /// A block of preformatted text (a document, log lines).
    pub pre: Option<(String, String)>,
    pub frames: Vec<Frame>,
    pub can_edit: bool,
    pub back: String,
}
