//! What every page carries: the shell (title, stylesheet, the viewer's
//! theme, and when signed in the sidebar), and the two plain pages every
//! area uses: a message, and a confirm page for anything that can't be
//! undone.

use askama::Template;

use crate::assets::CSS_HREF;
use crate::theme::Theme;

/// The frame around a page's content.
#[derive(Debug, Clone)]
pub struct Shell {
    /// The page's own title (before "· {brand}").
    pub title: String,
    /// The service's name as the operator set it.
    pub brand: String,
    pub css: String,
    /// Set when someone is signed in: the sidebar instead of the top bar.
    pub nav: Option<Nav>,
    /// The operator's branding, on the pages account holders see.
    pub dress: Option<Dress>,
    /// A live status page's event stream (the page loads the one script);
    /// empty for every other page.
    pub live: String,
}

/// Branding on a page: its stylesheet, icons, links and text.
#[derive(Debug, Clone, Default)]
pub struct Dress {
    /// The theme's stylesheet (after the base one).
    pub css: String,
    pub favicon: String,
    pub logo: String,
    pub links: Vec<Link>,
    pub footer: String,
    /// A line under the page title, where the page has one.
    pub blurb: String,
    /// Classes on `<html>` for a page with no signed-in theme (a preview
    /// forcing dark).
    pub html_class: String,
}

impl Shell {
    /// A page before sign-in.
    #[must_use]
    pub fn signed_out(title: impl Into<String>, brand: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            brand: brand.into(),
            css: CSS_HREF.clone(),
            nav: None,
            dress: None,
            live: String::new(),
        }
    }

    /// A page inside an area someone has signed in to.
    #[must_use]
    pub fn signed_in(title: impl Into<String>, brand: impl Into<String>, nav: Nav) -> Self {
        Self {
            title: title.into(),
            brand: brand.into(),
            css: CSS_HREF.clone(),
            nav: Some(nav),
            dress: None,
            live: String::new(),
        }
    }

    /// The script's path (live pages only load it).
    #[must_use]
    pub fn js(&self) -> String {
        crate::assets::JS_HREF.clone()
    }

    /// Whether the branding has a logo (it replaces the built-in mark).
    #[must_use]
    pub fn has_logo(&self) -> bool {
        self.dress.as_ref().is_some_and(|d| !d.logo.is_empty())
    }

    /// The classes on `<html>`: the viewer's theme, or none before sign-in.
    #[must_use]
    pub fn html_class(&self) -> String {
        match (&self.nav, &self.dress) {
            (Some(n), _) => n.theme.class(),
            (None, Some(d)) => d.html_class.clone(),
            (None, None) => String::new(),
        }
    }
}

/// The signed-in sidebar.
#[derive(Debug, Clone)]
pub struct Nav {
    /// The area: "Operator console" or "Your account".
    pub area: &'static str,
    /// Where the area starts, `/admin` or `/account`.
    pub base: &'static str,
    /// Who is signed in, as the menu shows them.
    pub who: String,
    pub csrf: String,
    pub theme: Theme,
    pub sections: Vec<Section>,
    /// The signed-in person's own pages (Appearance, …), in the menu under
    /// their name.
    pub own: Vec<Item>,
    /// The current item's href, which is marked; empty for none.
    pub current: String,
    pub banners: Vec<Banner>,
}

impl Nav {
    #[must_use]
    pub fn sign_out_action(&self) -> String {
        format!("{}/sign-out", self.base)
    }

    /// Whether the page is one of the person's own (the menu opens).
    #[must_use]
    pub fn on_own(&self) -> bool {
        self.own.iter().any(|i| self.is_current(i))
    }

    #[must_use]
    pub fn is_current(&self, item: &Item) -> bool {
        item.href == self.current
    }

    /// The small line above the title: the area and the section the page is
    /// in.
    #[must_use]
    pub fn eyebrow(&self) -> String {
        if self.on_own() {
            return format!("{} · Your account", self.area);
        }
        for s in &self.sections {
            if s.items.iter().any(|i| self.is_current(i)) {
                return format!("{} · {}", self.area, s.label);
            }
        }
        self.area.to_owned()
    }
}

/// A labelled group of sidebar items.
#[derive(Debug, Clone)]
pub struct Section {
    /// A stable key, for the label's id.
    pub key: &'static str,
    pub label: &'static str,
    pub items: Vec<Item>,
}

/// One sidebar link.
#[derive(Debug, Clone)]
pub struct Item {
    pub name: &'static str,
    pub href: String,
    /// A short count or word after the name ("3", "off").
    pub tag: Option<String>,
}

/// A line across the top of every page in an area, until its cause is gone.
#[derive(Debug, Clone)]
pub struct Banner {
    /// True draws it as a problem.
    pub bad: bool,
    pub text: String,
    pub link: Option<Link>,
}

/// A link with its words.
#[derive(Debug, Clone)]
pub struct Link {
    pub href: String,
    pub label: String,
}

/// A page that only says something.
#[derive(Template)]
#[template(path = "message.html")]
pub struct MessagePage {
    pub shell: Shell,
    pub message: String,
    pub link: Option<Link>,
}

/// The public page at `/`, when the operator turns it on.
#[derive(Template)]
#[template(path = "landing.html")]
pub struct LandingPage {
    pub shell: Shell,
    /// The account portal is on: link it.
    pub portal: bool,
    /// Sign-up is open in the portal: link it too.
    pub sign_up: bool,
}

/// A hidden field carried through a confirm page.
#[derive(Debug, Clone)]
pub struct Hidden {
    pub name: String,
    pub value: String,
}

/// The page before an action that can't be undone: what it does, what can't
/// be taken back, and (for the worst) a word to type.
#[derive(Template)]
#[template(path = "confirm.html")]
pub struct ConfirmPage {
    pub shell: Shell,
    pub csrf: String,
    pub error: Option<String>,
    pub what: String,
    pub undo: String,
    pub action: String,
    pub back: String,
    pub hidden: Vec<Hidden>,
    /// The word to type, for actions that destroy data.
    pub typed: Option<String>,
    pub button: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Font, Mode, Scheme};

    fn nav() -> Nav {
        Nav {
            area: "Operator console",
            base: "/admin",
            who: "alice".into(),
            csrf: "c5rf".into(),
            theme: Theme {
                mode: Mode::Dark,
                scheme: Scheme::Teal,
                font: Font::Plex,
            },
            sections: vec![Section {
                key: "people",
                label: "People",
                items: vec![
                    Item {
                        name: "Accounts",
                        href: "/admin/accounts".into(),
                        tag: None,
                    },
                    Item {
                        name: "Invites",
                        href: "/admin/invites".into(),
                        tag: Some("3".into()),
                    },
                ],
            }],
            own: vec![Item {
                name: "Appearance",
                href: "/admin/appearance".into(),
                tag: None,
            }],
            current: "/admin/invites".into(),
            banners: vec![Banner {
                bad: true,
                text: "Mail isn't set up.".into(),
                link: None,
            }],
        }
    }

    #[test]
    fn signed_in_pages_carry_the_sidebar_theme_and_one_current_item() {
        let page = MessagePage {
            shell: Shell::signed_in("Done", "Example PDS", nav()),
            message: "Saved <b>".into(),
            link: None,
        }
        .render()
        .unwrap();
        assert!(
            page.contains(r#"<html lang="en" class="mode-dark scheme-teal font-plex">"#),
            "{page}"
        );
        assert!(page.contains("<title>Done · Example PDS</title>"));
        assert!(page.contains(&format!(r#"href="{}""#, *CSS_HREF)));
        assert!(page.contains(r#"class="sidebar""#) && !page.contains(r#"class="top""#));
        assert_eq!(page.matches(r#"aria-current="page""#).count(), 1);
        assert!(page.contains(r#"href="/admin/invites" aria-current="page""#));
        assert!(page.contains(r#"<span class="tag">3</span>"#));
        assert!(page.contains("Operator console · People"));
        assert!(
            page.contains(r#"<div class="banner bad">Mail isn&#x27;t set up."#),
            "{page}"
        );
        assert!(page.contains("Saved &lt;b&gt;"), "escaped");
        assert!(page.contains(r#"action="/admin/sign-out""#) && page.contains(r#"value="c5rf""#));
    }

    #[test]
    fn signed_out_pages_have_the_top_bar_and_no_theme() {
        let page = MessagePage {
            shell: Shell::signed_out("Welcome", "Example PDS"),
            message: "Hi".into(),
            link: Some(Link {
                href: "/admin".into(),
                label: "Sign in".into(),
            }),
        }
        .render()
        .unwrap();
        assert!(page.contains(r#"<html lang="en">"#));
        assert!(page.contains(r#"class="top""#) && !page.contains("sidebar"));
        assert!(page.contains(r#"<a class="button secondary" href="/admin">Sign in</a>"#));
    }

    #[test]
    fn the_confirm_page_asks_for_the_word_only_when_given_one() {
        let mut page = ConfirmPage {
            shell: Shell::signed_in("Delete the account?", "Example PDS", nav()),
            csrf: "c5rf".into(),
            error: None,
            what: "This deletes @bob.test.".into(),
            undo: "It can't be undone.".into(),
            action: "/admin/accounts/delete".into(),
            back: "/admin/accounts".into(),
            hidden: vec![Hidden {
                name: "did".into(),
                value: "did:plc:bob".into(),
            }],
            typed: Some("delete".into()),
            button: "Delete".into(),
        };
        let html = page.render().unwrap();
        assert!(html.contains(r#"name="typed""#) && html.contains("<code>delete</code>"));
        assert!(html.contains(r#"name="did" value="did:plc:bob""#));
        page.typed = None;
        assert!(!page.render().unwrap().contains(r#"name="typed""#));
    }

    /// No page may lean on inline style or script: the CSP forbids both.
    #[test]
    fn no_template_uses_inline_style_or_script() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/templates");
        let mut seen = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let mut text = std::fs::read_to_string(&path).unwrap();
            // The script tags allowed are external, by path, nothing inline:
            // the bundled console script and the hCaptcha widget.
            text = text.replace(r#"<script src="{{ shell.js() }}" defer></script>"#, "");
            text = text.replace(
                r#"<script src="{{ hcaptcha_script }}" async defer></script>"#,
                "",
            );
            for bad in [
                "style=",
                "<style",
                "<script",
                "onclick",
                "onsubmit",
                "javascript:",
            ] {
                assert!(!text.contains(bad), "{} uses {bad}", path.display());
            }
            seen += 1;
        }
        assert!(seen >= 3);
    }
}
