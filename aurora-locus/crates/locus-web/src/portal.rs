//! The account portal's pages, their names and where they live, and its
//! sign-in pages.

use askama::Template;

use crate::theme::Theme;
use crate::view::{Banner, Item, Nav, Section, Shell};

/// A portal page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageId {
    Overview,
    Handle,
    Email,
    Password,
    AppPasswords,
    Apps,
    Export,
    Identity,
    Moving,
    Deactivate,
    Delete,
    Appearance,
}

/// (id, href, name, section key)
pub const PAGES: &[(PageId, &str, &str, Option<&str>)] = &[
    (PageId::Overview, "/account", "Overview", Some("account")),
    (PageId::Handle, "/account/handle", "Handle", Some("account")),
    (PageId::Email, "/account/email", "Email", Some("account")),
    (
        PageId::Password,
        "/account/password",
        "Password",
        Some("account"),
    ),
    (
        PageId::AppPasswords,
        "/account/app-passwords",
        "App passwords",
        Some("apps"),
    ),
    (
        PageId::Apps,
        "/account/apps",
        "Connected apps",
        Some("apps"),
    ),
    (
        PageId::Export,
        "/account/export",
        "Download your data",
        Some("data"),
    ),
    (
        PageId::Identity,
        "/account/identity",
        "Identity and recovery",
        Some("data"),
    ),
    (
        PageId::Moving,
        "/account/moving",
        "Moving an account here",
        Some("data"),
    ),
    (
        PageId::Deactivate,
        "/account/deactivate",
        "Deactivate",
        Some("leaving"),
    ),
    (PageId::Delete, "/account/delete", "Delete", Some("leaving")),
    (
        PageId::Appearance,
        "/account/appearance",
        "Appearance",
        None,
    ),
];

pub const SECTIONS: &[(&str, &str)] = &[
    ("account", "Account"),
    ("apps", "Apps"),
    ("data", "Your data"),
    ("leaving", "Leaving"),
];

impl PageId {
    fn row(self) -> &'static (PageId, &'static str, &'static str, Option<&'static str>) {
        PAGES.iter().find(|p| p.0 == self).unwrap_or(&PAGES[0])
    }

    #[must_use]
    pub fn href(self) -> &'static str {
        self.row().1
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        self.row().2
    }
}

/// The portal's sidebar for `who` on `current`.
#[must_use]
pub fn nav(who: &str, csrf: &str, theme: Theme, current: PageId, banners: Vec<Banner>) -> Nav {
    let item = |p: &(PageId, &'static str, &'static str, Option<&'static str>)| Item {
        name: p.2,
        href: p.1.to_owned(),
        tag: None,
    };
    Nav {
        area: "Your account",
        base: "/account",
        who: who.to_owned(),
        csrf: csrf.to_owned(),
        theme,
        sections: SECTIONS
            .iter()
            .map(|(key, label)| Section {
                key,
                label,
                items: PAGES
                    .iter()
                    .filter(|p| p.3 == Some(*key))
                    .map(item)
                    .collect(),
            })
            .collect(),
        own: PAGES.iter().filter(|p| p.3.is_none()).map(item).collect(),
        current: current.href().to_owned(),
        banners,
    }
}

/// Whether `next` may be where the portal goes after sign-in: a plain
/// path under `/account`.
#[must_use]
pub fn safe_next(next: &str) -> bool {
    next.len() <= 512
        && (next == "/account" || next.starts_with("/account/"))
        && !next.contains("//")
        && !next.contains('\\')
        && !next.split('/').any(|s| s == "..")
        && !next.bytes().any(|b| b.is_ascii_control() || b == b' ')
}

/// The portal's sign-in (and its code step) and sign-up pages.
#[derive(Template)]
#[template(path = "portal_sign_in.html")]
pub struct SignInPage {
    pub shell: Shell,
    pub csrf: String,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub identifier: String,
    /// The account needs the code emailed to it.
    pub code_stage: bool,
    pub sign_up: bool,
    /// For sign-up: the handle domains, and whether a code is needed.
    pub domains: Vec<String>,
    pub invite_required: bool,
    pub handle: String,
    pub email: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_portal_menu_and_next() {
        let n = nav("alice.test", "c", Theme::default(), PageId::Email, vec![]);
        assert_eq!(n.sections.len(), 4);
        assert_eq!(n.own.len(), 1);
        assert!(n.eyebrow().ends_with("Account"));
        for ok in ["/account", "/account/email", "/account/apps?x=1"] {
            assert!(safe_next(ok), "{ok}");
        }
        for bad in [
            "/admin",
            "//evil",
            "/account/../admin",
            "https://evil/account",
            "/accountx",
            "/account\\x",
        ] {
            assert!(!safe_next(bad), "{bad}");
        }
    }
}
