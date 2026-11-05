//! The console's pages, their names and where they live: one source for the
//! sidebar, each page's title and eyebrow, and where Cancel may go.

use crate::theme::Theme;
use crate::view::{Banner, Item, Nav, Section};

/// The lowest role a page needs (pages show read-only parts to lower
/// roles; actions check their own role).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Need {
    Viewer,
    Moderator,
    Admin,
    Owner,
}

impl Need {
    /// From a stored role name; unknown is the lowest.
    #[must_use]
    pub fn from_role(role: &str) -> Self {
        match role {
            "owner" => Self::Owner,
            "admin" => Self::Admin,
            "moderator" => Self::Moderator,
            _ => Self::Viewer,
        }
    }
}

/// A console page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageId {
    Home,
    Accounts,
    Invites,
    Operators,
    Moderation,
    Scanning,
    Private,
    Portal,
    Branding,
    Identity,
    Services,
    Email,
    Proxy,
    Limits,
    Storage,
    Jobs,
    Audit,
    Logs,
    Settings,
    Appearance,
    Security,
}

/// A sidebar heading: its key (for element ids) and label.
pub const SECTIONS: &[(&str, &str)] = &[
    ("overview", "Overview"),
    ("people", "People"),
    ("moderation", "Moderation"),
    ("private", "Private posts"),
    ("pages", "Account pages"),
    ("network", "Network"),
    ("system", "System"),
];

/// A page: its id, address, name, product tag, heading (none for the
/// operator's own pages) and the role needed to see it.
#[derive(Debug)]
pub struct Page {
    pub id: PageId,
    pub href: &'static str,
    pub name: &'static str,
    pub tag: Option<&'static str>,
    pub section: Option<&'static str>,
    pub need: Need,
}

const fn page(
    id: PageId,
    href: &'static str,
    name: &'static str,
    tag: Option<&'static str>,
    section: Option<&'static str>,
    need: Need,
) -> Page {
    Page {
        id,
        href,
        name,
        tag,
        section,
        need,
    }
}

/// Every page, in the sidebar's order, the operator's own pages last.
pub const PAGES: &[Page] = &[
    page(
        PageId::Home,
        "/admin",
        "Home",
        None,
        Some("overview"),
        Need::Viewer,
    ),
    page(
        PageId::Accounts,
        "/admin/accounts",
        "Accounts",
        None,
        Some("people"),
        Need::Viewer,
    ),
    page(
        PageId::Invites,
        "/admin/invites",
        "Invites",
        None,
        Some("people"),
        Need::Viewer,
    ),
    page(
        PageId::Operators,
        "/admin/operators",
        "Operators",
        None,
        Some("people"),
        Need::Owner,
    ),
    page(
        PageId::Moderation,
        "/admin/moderation",
        "Moderation",
        None,
        Some("moderation"),
        Need::Viewer,
    ),
    page(
        PageId::Scanning,
        "/admin/scanning",
        "Scanning",
        None,
        Some("moderation"),
        Need::Viewer,
    ),
    page(
        PageId::Private,
        "/admin/private",
        "Private posts",
        Some("kryphocron"),
        Some("private"),
        Need::Viewer,
    ),
    page(
        PageId::Portal,
        "/admin/portal",
        "Account portal",
        None,
        Some("pages"),
        Need::Viewer,
    ),
    page(
        PageId::Branding,
        "/admin/branding",
        "Branding",
        None,
        Some("pages"),
        Need::Viewer,
    ),
    page(
        PageId::Identity,
        "/admin/identity",
        "Identity",
        None,
        Some("network"),
        Need::Viewer,
    ),
    page(
        PageId::Services,
        "/admin/services",
        "Services",
        None,
        Some("network"),
        Need::Viewer,
    ),
    page(
        PageId::Email,
        "/admin/email",
        "Email",
        None,
        Some("network"),
        Need::Viewer,
    ),
    page(
        PageId::Proxy,
        "/admin/proxy",
        "Proxy and TLS",
        None,
        Some("network"),
        Need::Viewer,
    ),
    page(
        PageId::Limits,
        "/admin/limits",
        "Limits",
        None,
        Some("network"),
        Need::Viewer,
    ),
    page(
        PageId::Storage,
        "/admin/storage",
        "Storage",
        None,
        Some("system"),
        Need::Viewer,
    ),
    page(
        PageId::Jobs,
        "/admin/jobs",
        "Jobs",
        None,
        Some("system"),
        Need::Viewer,
    ),
    page(
        PageId::Audit,
        "/admin/audit",
        "Audit log",
        None,
        Some("system"),
        Need::Viewer,
    ),
    page(
        PageId::Logs,
        "/admin/logs",
        "Logs",
        None,
        Some("system"),
        Need::Admin,
    ),
    page(
        PageId::Settings,
        "/admin/settings",
        "All settings",
        None,
        Some("system"),
        Need::Viewer,
    ),
    page(
        PageId::Appearance,
        "/admin/appearance",
        "Appearance",
        None,
        None,
        Need::Viewer,
    ),
    page(
        PageId::Security,
        "/admin/security",
        "Sign-in security",
        None,
        None,
        Need::Viewer,
    ),
];

/// The pages built so far. A heading with none of them isn't drawn.
pub const BUILT: &[PageId] = &[
    PageId::Home,
    PageId::Accounts,
    PageId::Invites,
    PageId::Operators,
    PageId::Moderation,
    PageId::Scanning,
    PageId::Private,
    PageId::Portal,
    PageId::Identity,
    PageId::Services,
    PageId::Email,
    PageId::Proxy,
    PageId::Limits,
    PageId::Storage,
    PageId::Jobs,
    PageId::Audit,
    PageId::Logs,
    PageId::Settings,
    PageId::Appearance,
    PageId::Security,
];

impl PageId {
    #[must_use]
    pub fn page(self) -> &'static Page {
        PAGES.iter().find(|p| p.id == self).unwrap_or(&PAGES[0])
    }

    #[must_use]
    pub fn built(self) -> bool {
        BUILT.contains(&self)
    }

    #[must_use]
    pub fn href(self) -> &'static str {
        self.page().href
    }

    /// Its name, with its product: "Private posts · kryphocron".
    #[must_use]
    pub fn title(self) -> String {
        let p = self.page();
        match p.tag {
            Some(tag) => format!("{} · {tag}", p.name),
            None => p.name.to_owned(),
        }
    }

    /// The built page whose href is `path` exactly.
    #[must_use]
    pub fn at(path: &str) -> Option<Self> {
        PAGES
            .iter()
            .find(|p| p.href == path && p.id.built())
            .map(|p| p.id)
    }
}

/// The page a path belongs to: itself, or the page it's under
/// (`/admin/accounts/did:…` is under Accounts).
#[must_use]
pub fn locate(path: &str) -> Option<PageId> {
    let path = path.split_once('?').map_or(path, |(p, _)| p);
    if let Some(id) = PageId::at(path) {
        return Some(id);
    }
    PAGES
        .iter()
        .filter(|p| p.id != PageId::Home && p.id.built())
        .find(|p| path.starts_with(p.href) && path.as_bytes().get(p.href.len()) == Some(&b'/'))
        .map(|p| p.id)
}

/// The sidebar for an operator with `role` on `current`.
#[must_use]
pub fn nav(
    who: &str,
    role: &str,
    csrf: &str,
    theme: Theme,
    current: PageId,
    banners: Vec<Banner>,
) -> Nav {
    let need = Need::from_role(role);
    let item = |p: &Page| Item {
        name: p.name,
        href: p.href.to_owned(),
        tag: p.tag.map(str::to_owned),
    };
    let sections = SECTIONS
        .iter()
        .map(|(key, label)| Section {
            key,
            label,
            items: PAGES
                .iter()
                .filter(|p| p.section == Some(*key) && p.id.built() && p.need <= need)
                .map(item)
                .collect(),
        })
        .filter(|s| !s.items.is_empty())
        .collect();
    let own = PAGES
        .iter()
        .filter(|p| p.section.is_none() && p.id.built())
        .map(item)
        .collect();
    Nav {
        area: "Operator console",
        base: "/admin",
        who: who.to_owned(),
        csrf: csrf.to_owned(),
        theme,
        sections,
        own,
        current: current.href().to_owned(),
        banners,
    }
}

/// Whether `back` may be where Cancel and Back go: a built page of the
/// console (or a page under one), as a plain relative path that can't step
/// out of `/admin` or to another site.
#[must_use]
pub fn safe_back(back: &str) -> bool {
    if back.len() > 512 || !back.starts_with("/admin") {
        return false;
    }
    if !matches!(back.as_bytes().get(6), None | Some(b'/' | b'?')) {
        return false;
    }
    if back.contains("//")
        || back.contains('\\')
        || back.contains('#')
        || back.bytes().any(|b| b.is_ascii_control() || b == b' ')
    {
        return false;
    }
    let lower = back.to_ascii_lowercase();
    if lower.contains("%2e") || lower.contains("%2f") || lower.contains("%5c") {
        return false;
    }
    let path = back.split_once('?').map_or(back, |(p, _)| p);
    if path.split('/').any(|seg| seg == ".." || seg == ".") {
        return false;
    }
    locate(path).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_has_one_row_and_a_unique_href() {
        for a in PAGES {
            assert!(a.href.starts_with("/admin"), "{}", a.href);
            assert_eq!(a.id.page().id, a.id);
            if let Some(s) = a.section {
                assert!(SECTIONS.iter().any(|(k, _)| *k == s), "{s}");
            }
            for b in PAGES {
                if a.id != b.id {
                    assert_ne!(a.href, b.href);
                }
            }
        }
        assert_eq!(PageId::Private.title(), "Private posts · kryphocron");
    }

    #[test]
    fn the_sidebar_hides_pages_above_the_role() {
        let names = |role| {
            nav("a", role, "c", Theme::default(), PageId::Home, vec![])
                .sections
                .iter()
                .flat_map(|s| s.items.iter().map(|i| i.href.clone()))
                .collect::<Vec<_>>()
        };
        for p in PAGES.iter().filter(|p| p.id.built() && p.section.is_some()) {
            assert!(names("owner").contains(&p.href.to_owned()), "{:?}", p.id);
            assert_eq!(
                names("viewer").contains(&p.href.to_owned()),
                p.need == Need::Viewer,
                "{:?}",
                p.id
            );
        }
        let n = nav(
            "a",
            "viewer",
            "c",
            Theme::default(),
            PageId::Appearance,
            vec![],
        );
        assert_eq!(n.own.len(), 2);
        assert!(n.on_own() && n.eyebrow().ends_with("Your account"));
    }

    #[test]
    fn safe_back_accepts_only_console_pages() {
        for ok in [
            "/admin",
            "/admin?x=1",
            "/admin/settings",
            "/admin/settings?q=mail",
            "/admin/portal",
        ] {
            assert!(safe_back(ok), "{ok}");
        }
        let long = format!("/admin/settings?{}", "a".repeat(600));
        for bad in [
            "//evil.example",
            "/\\evil",
            "/admin@evil.example",
            "/adminx",
            "/admin/../x",
            "/admin/%2e%2e/x",
            "https://evil.example/admin",
            "javascript:alert(1)",
            "/admin/settings\r\nx",
            long.as_str(),
            "/admin/nowhere",
            "/admin/settings#x",
            "/admin//settings",
            "",
        ] {
            assert!(!safe_back(bad), "{bad:?}");
        }
    }
}
