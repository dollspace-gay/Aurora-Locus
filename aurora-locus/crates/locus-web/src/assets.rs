//! The UI's assets, compiled in: the stylesheet at a content-hashed path
//! (cached for a year), and the bundled fonts (SIL Open Font License 1.1,
//! each licence beside its files under `/static/fonts/`).

use std::sync::LazyLock;

use sha2::Digest;

/// The one stylesheet.
pub const CSS: &str = include_str!("../static/locus.css");

/// The stylesheet's path: its content hash in the name.
pub static CSS_HREF: LazyLock<String> = LazyLock::new(|| {
    let hash = hex::encode(sha2::Sha256::digest(CSS.as_bytes()));
    format!("/static/locus-{}.css", &hash[..12])
});

/// The one script: live refresh on the console's status pages.
pub const JS: &str = include_str!("../static/locus.js");

/// The script's path: its content hash in the name.
pub static JS_HREF: LazyLock<String> = LazyLock::new(|| {
    let hash = hex::encode(sha2::Sha256::digest(JS.as_bytes()));
    format!("/static/locus-{}.js", &hash[..12])
});

/// Every page's `Content-Security-Policy`: nothing from anywhere else, no
/// scripts, no inline anything.
pub const CSP: &str = "default-src 'none'; style-src 'self'; font-src 'self'; img-src 'self'; \
     form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

/// A live status page's: [`CSP`] plus the one script, and its event stream
/// and refetch, from this site only.
pub const CSP_LIVE: &str = "default-src 'none'; style-src 'self'; font-src 'self'; img-src 'self'; \
     script-src 'self'; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

/// A page's `Cache-Control`.
pub const NO_STORE: &str = "no-store, no-transform";

/// An asset's `Cache-Control`: its name changes with its content.
pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";

macro_rules! fonts {
    ($($name:literal),* $(,)?) => {
        /// The bundled font files and their licences, by file name.
        pub static FONTS: &[(&str, &[u8])] = &[
            $(($name, include_bytes!(concat!("../static/fonts/", $name)))),*
        ];
    };
}

fonts!(
    "atkinson-hyperlegible-1.006-400.woff2",
    "atkinson-hyperlegible-1.006-400-italic.woff2",
    "atkinson-hyperlegible-1.006-700.woff2",
    "jetbrains-mono-2.304-400.woff2",
    "jetbrains-mono-2.304-600.woff2",
    "ibm-plex-sans-1.1.0-400.woff2",
    "ibm-plex-sans-1.1.0-600.woff2",
    "ibm-plex-sans-1.1.0-700.woff2",
    "ibm-plex-mono-2.5.0-400.woff2",
    "ibm-plex-mono-2.5.0-600.woff2",
    "OFL-atkinson-hyperlegible-1.006.txt",
    "OFL-jetbrains-mono-2.304.txt",
    "OFL-ibm-plex-sans-1.1.0.txt",
    "OFL-ibm-plex-mono-2.5.0.txt",
);

/// An asset by its path under `/static/`: (content type, bytes).
#[must_use]
pub fn asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    if format!("/static/{path}") == *CSS_HREF {
        return Some(("text/css; charset=utf-8", CSS.as_bytes()));
    }
    if format!("/static/{path}") == *JS_HREF {
        return Some(("text/javascript; charset=utf-8", JS.as_bytes()));
    }
    let name = path.strip_prefix("fonts/")?;
    let (_, bytes) = FONTS.iter().find(|(n, _)| *n == name)?;
    let kind = if name.ends_with(".woff2") {
        "font/woff2"
    } else {
        "text/plain; charset=utf-8"
    };
    Some((kind, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stylesheet_and_every_font_it_names_are_served() {
        let css_path = CSS_HREF.strip_prefix("/static/").unwrap();
        assert_eq!(asset(css_path).unwrap().0, "text/css; charset=utf-8");
        assert!(
            asset("locus-000000000000.css").is_none(),
            "only the current hash"
        );
        for url in CSS.split("url(").skip(1) {
            let url = &url[..url.find(')').unwrap()];
            let path = url
                .strip_prefix("/static/")
                .unwrap_or_else(|| panic!("{url}"));
            assert!(asset(path).is_some(), "{url} isn't served");
        }
        for (name, _) in FONTS.iter().filter(|(n, _)| n.ends_with(".woff2")) {
            assert!(CSS.contains(name), "{name} is bundled but unused");
        }
        assert!(asset("fonts/../../etc/passwd").is_none());
        let js_path = JS_HREF.strip_prefix("/static/").unwrap();
        assert_eq!(asset(js_path).unwrap().0, "text/javascript; charset=utf-8");
        assert!(asset("locus-000000000000.js").is_none());
        // The live CSP is the strict one plus scripts and connections from
        // this site, nothing else.
        assert_eq!(
            CSP_LIVE.replace(" script-src 'self'; connect-src 'self';", ""),
            CSP
        );
    }

    #[test]
    fn nothing_hard_codes_a_colour_after_the_tokens() {
        let rules = &CSS[CSS.find("/* Top bar").expect("the token blocks end")..];
        for bad in [
            "#0", "#1", "#2", "#3", "#4", "#5", "#6", "#7", "#8", "#9", "#a", "#b", "#c", "#d",
            "#e", "#f", "rgb(",
        ] {
            for (i, _) in rules.match_indices(bad) {
                let line = rules[..i].rsplit('\n').next().unwrap_or("");
                let next = rules[i..].split('\n').next().unwrap_or("");
                // The sidebar's menu shadow is the one literal the design keeps.
                assert!(
                    next.contains("rgba(0, 0, 0, .35)") || line.contains("box-shadow: 0 8px 24px"),
                    "a colour outside the tokens: {line}{next}"
                );
            }
        }
    }
}
