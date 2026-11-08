//! Branding: the operator's theme for the pages account holders see.
//!
//! The OAuth sign-in and consent pages take only the theme's checked
//! variables (colours that pass a contrast check, the logo, background
//! images, a bundled font, corner and spacing presets): never custom CSS
//! or uploaded fonts, so a theme can't hide, reorder or re-letter what a
//! user is approving. The account portal and sign-up also get the custom
//! CSS (sanitized by a CSS parser that re-serializes only what it accepts)
//! and an uploaded font.

use std::fmt::Write as _;

use cssparser::{ParseError, Parser, ParserInput, ToCss, Token};
use serde::{Deserialize, Serialize};

/// One mode's colours, as `#rrggbb`; empty keeps the built-in one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Colors {
    pub primary: String,
    pub error: String,
    pub warning: String,
    pub info: String,
    pub success: String,
}

impl Colors {
    fn each(&self) -> [(&'static str, &str); 5] {
        [
            ("primary", self.primary.as_str()),
            ("error", self.error.as_str()),
            ("warning", self.warning.as_str()),
            ("info", self.info.as_str()),
            ("success", self.success.as_str()),
        ]
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Links {
    pub home: String,
    pub terms: String,
    pub privacy: String,
    pub support: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Texts {
    pub sign_in_heading: String,
    pub sign_in_blurb: String,
    pub sign_up_blurb: String,
    pub portal_blurb: String,
    pub footer: String,
}

/// A theme (a draft or a published version).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    /// Shown instead of the hostname; empty keeps the service name setting.
    pub service_name: String,
    /// Uploaded assets, by id.
    pub logo: String,
    pub favicon: String,
    pub background_light: String,
    pub background_dark: String,
    pub light: Colors,
    pub dark: Colors,
    pub links: Links,
    /// A bundled font: hyperlegible, system or plex.
    pub font: String,
    /// An uploaded font (portal and sign-up only), by asset id.
    pub uploaded_font: String,
    /// sharp, default or round.
    pub radius: String,
    /// compact, default or roomy.
    pub spacing: String,
    pub text: Texts,
    /// Sanitized custom CSS (portal and sign-up only).
    pub custom_css: String,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            service_name: String::new(),
            logo: String::new(),
            favicon: String::new(),
            background_light: String::new(),
            background_dark: String::new(),
            light: Colors::default(),
            dark: Colors::default(),
            links: Links::default(),
            font: "system".into(),
            uploaded_font: String::new(),
            radius: "default".into(),
            spacing: "default".into(),
            text: Texts::default(),
            custom_css: String::new(),
        }
    }
}

/// The page backgrounds colours are checked against (the cards the text
/// and buttons sit on).
pub const LIGHT_BACKGROUND: &str = "#ffffff";
pub const DARK_BACKGROUND: &str = "#181b21";
/// The smallest contrast a colour needs against its background (WCAG AA
/// for text).
pub const MIN_CONTRAST: f64 = 4.5;

pub const RADII: [&str; 3] = ["sharp", "default", "round"];
pub const SPACINGS: [&str; 3] = ["compact", "default", "roomy"];
pub const FONTS: [&str; 3] = ["hyperlegible", "system", "plex"];

/// The longest page text.
pub const MAX_TEXT: usize = 500;
/// The largest custom CSS.
pub const MAX_CSS: usize = 32 * 1024;

fn parse_hex(c: &str) -> Option<(u8, u8, u8)> {
    let h = c.strip_prefix('#')?;
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// WCAG contrast ratio of two `#rrggbb` colours.
#[must_use]
pub fn contrast(a: &str, b: &str) -> Option<f64> {
    let (la, lb) = (luminance(parse_hex(a)?), luminance(parse_hex(b)?));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    Some((hi + 0.05) / (lo + 0.05))
}

impl Theme {
    /// Everything that keeps this theme from being published. `assets`
    /// holds (id, kind) of the uploads that exist.
    #[must_use]
    pub fn problems(&self, assets: &[(String, String)]) -> Vec<String> {
        let mut out = Vec::new();
        for (mode, colors, bg) in [
            ("light", &self.light, LIGHT_BACKGROUND),
            ("dark", &self.dark, DARK_BACKGROUND),
        ] {
            for (name, c) in colors.each() {
                if c.is_empty() {
                    continue;
                }
                match contrast(c, bg) {
                    None => out.push(format!("The {mode} {name} colour must be like #1f6feb.")),
                    Some(r) if r < MIN_CONTRAST => out.push(format!(
                        "The {mode} {name} colour {c} is too faint on its background ({r:.1}:1; it needs {MIN_CONTRAST}:1)."
                    )),
                    Some(_) => {}
                }
            }
        }
        for (name, url) in [
            ("home", &self.links.home),
            ("terms", &self.links.terms),
            ("privacy", &self.links.privacy),
            ("support", &self.links.support),
        ] {
            let fine = url.is_empty()
                || url.starts_with("https://")
                || (name == "support" && url.starts_with("mailto:"));
            if !fine {
                out.push(format!("The {name} link must start with https://."));
            }
        }
        for (field, kinds, id) in [
            ("logo", &["logo"][..], &self.logo),
            ("favicon", &["favicon", "logo"][..], &self.favicon),
            (
                "light background",
                &["background"][..],
                &self.background_light,
            ),
            (
                "dark background",
                &["background"][..],
                &self.background_dark,
            ),
            ("uploaded font", &["font"][..], &self.uploaded_font),
        ] {
            if !id.is_empty()
                && !assets
                    .iter()
                    .any(|(a, k)| a == id && kinds.contains(&k.as_str()))
            {
                out.push(format!("The {field} isn't one of the uploads."));
            }
        }
        if !FONTS.contains(&self.font.as_str()) {
            out.push("Pick one of the bundled fonts.".into());
        }
        if !RADII.contains(&self.radius.as_str()) || !SPACINGS.contains(&self.spacing.as_str()) {
            out.push("Pick the corners and spacing from the lists.".into());
        }
        let t = &self.text;
        for (name, v) in [
            ("service name", &self.service_name),
            ("sign-in heading", &t.sign_in_heading),
            ("sign-in text", &t.sign_in_blurb),
            ("sign-up text", &t.sign_up_blurb),
            ("portal text", &t.portal_blurb),
            ("footer", &t.footer),
        ] {
            if v.chars().count() > MAX_TEXT {
                out.push(format!("The {name} is longer than {MAX_TEXT} characters."));
            }
        }
        let paths: Vec<String> = assets.iter().map(|(id, _)| asset_path(id)).collect();
        if let Err(e) = sanitize_css(&self.custom_css, &paths) {
            out.push(format!("Custom CSS: {e}"));
        }
        out
    }

    /// The asset ids this theme uses.
    #[must_use]
    pub fn assets(&self) -> Vec<String> {
        [
            &self.logo,
            &self.favicon,
            &self.background_light,
            &self.background_dark,
            &self.uploaded_font,
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect()
    }
}

/// Where an uploaded asset is served.
#[must_use]
pub fn asset_path(id: &str) -> String {
    format!("/branding/{id}")
}

/// Which colours apply: the device's choice, or one forced (previews).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Light,
    Dark,
}

fn radius_px(r: &str) -> (&'static str, &'static str) {
    match r {
        "sharp" => ("2px", "2px"),
        "round" => ("18px", "12px"),
        _ => ("10px", "8px"),
    }
}

fn font_stack(f: &str) -> &'static str {
    match f {
        "hyperlegible" => "\"Atkinson Hyperlegible\", system-ui, sans-serif",
        "plex" => "\"IBM Plex Sans\", system-ui, sans-serif",
        _ => "system-ui, -apple-system, \"Segoe UI\", sans-serif",
    }
}

fn colour_vars(c: &Colors, scheme_suffix: &str, out: &mut String) {
    // The OAuth pages' variables, and the default scheme's accent of the
    // portal (an account's own scheme choice still wins).
    for (name, v) in c.each() {
        if v.is_empty() {
            continue;
        }
        let var = match name {
            "info" => "notice",
            other => other,
        };
        let _ = write!(out, "--{var}: {v}; ");
        match name {
            "primary" => {
                let _ = write!(
                    out,
                    "--s-accent-{scheme_suffix}: {v}; --s-hover-{scheme_suffix}: {v}; "
                );
            }
            "error" => {
                let _ = write!(out, "--bad: {v}; ");
            }
            "warning" => {
                let _ = write!(out, "--warn: {v}; ");
            }
            "success" => {
                let _ = write!(out, "--ok: {v}; ");
            }
            _ => {}
        }
    }
}

/// The checked variables: what the OAuth sign-in and consent pages get.
#[must_use]
pub fn vars_css(t: &Theme, mode: Mode) -> String {
    let mut out = String::from("/* Branding: checked variables. */\n");
    let (radius, radius_sm) = radius_px(&t.radius);
    let _ = writeln!(
        out,
        ":root, .scheme-sky {{ --radius: {radius}; --radius-sm: {radius_sm}; --brand-font: {}; }}",
        font_stack(&t.font)
    );
    let mut light = String::new();
    colour_vars(&t.light, "l", &mut light);
    let mut dark = String::new();
    colour_vars(&t.dark, "d", &mut dark);
    let bg = |id: &str| {
        (!id.is_empty()).then(|| {
            format!(
                "body {{ background-image: url(\"{}\"); background-size: cover; }}",
                asset_path(id)
            )
        })
    };
    match mode {
        Mode::Light | Mode::Auto => {
            if !light.is_empty() {
                let _ = writeln!(out, ":root, .scheme-sky {{ {light}}}");
            }
            if let Some(b) = bg(&t.background_light) {
                let _ = writeln!(out, "{b}");
            }
        }
        Mode::Dark => {}
    }
    match mode {
        Mode::Auto => {
            if !dark.is_empty() || !t.background_dark.is_empty() {
                let _ = writeln!(out, "@media (prefers-color-scheme: dark) {{");
                if !dark.is_empty() {
                    let _ = writeln!(out, "  :root, .scheme-sky {{ {dark}}}");
                }
                if let Some(b) = bg(&t.background_dark) {
                    let _ = writeln!(out, "  {b}");
                }
                let _ = writeln!(out, "}}");
            }
        }
        Mode::Dark => {
            if !dark.is_empty() {
                let _ = writeln!(out, ":root, .scheme-sky {{ {dark}}}");
            }
            if let Some(b) = bg(&t.background_dark) {
                let _ = writeln!(out, "{b}");
            }
        }
        Mode::Light => {}
    }
    let pad = match t.spacing.as_str() {
        "compact" => Some("1rem"),
        "roomy" => Some("1.9rem"),
        _ => None,
    };
    if let Some(p) = pad {
        let _ = writeln!(out, ".card {{ padding: {p}; }}");
    }
    out
}

/// Everything: the variables, the uploaded font and the custom CSS. For
/// the account portal and sign-up only.
#[must_use]
pub fn full_css(t: &Theme, mode: Mode) -> String {
    let mut out = vars_css(t, mode);
    if !t.uploaded_font.is_empty() {
        let _ = writeln!(
            out,
            "@font-face {{ font-family: \"Brand\"; src: url(\"{}\"); font-display: swap; }}\n:root, .scheme-sky, .font-system, .font-hyperlegible, .font-plex {{ --font-sans: \"Brand\", system-ui, sans-serif; --brand-font: \"Brand\", system-ui, sans-serif; }}",
            asset_path(&t.uploaded_font)
        );
    }
    if !t.custom_css.is_empty() {
        out.push_str("/* Custom CSS. */\n");
        out.push_str(&t.custom_css);
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------- the sanitizer

/// Functions a value may never call (they load, run or embed).
const BANNED_FUNCTIONS: &[&str] = &[
    "url",
    "image-set",
    "-webkit-image-set",
    "image",
    "cross-fade",
    "-webkit-cross-fade",
    "element",
    "-moz-element",
    "expression",
    "paint",
    "src",
];
/// Properties that run code in old browsers.
const BANNED_PROPERTIES: &[&str] = &["behavior", "-moz-binding", "-ms-behavior"];

type Err<'i> = ParseError<'i, String>;

fn fail<'i>(p: &Parser<'i, '_>, why: impl Into<String>) -> Err<'i> {
    p.new_custom_error(why.into())
}

/// Copies one token (and its block, if it opens one) into `out`, refusing
/// what can load or run something. `allowed` holds the served paths of
/// uploads a `url()` may name.
fn copy_token<'i>(
    p: &mut Parser<'i, '_>,
    tok: &Token<'i>,
    allowed: &[String],
    out: &mut String,
) -> Result<(), Err<'i>> {
    match tok {
        Token::UnquotedUrl(u) => {
            if !allowed.iter().any(|a| a == u.as_ref()) {
                return Err(fail(p, format!("url({u}) isn't one of the uploads")));
            }
            let _ = tok.to_css(out);
        }
        Token::Function(name) => {
            let lower = name.to_ascii_lowercase();
            if lower == "url" {
                // url("…"): only an upload's exact path.
                let target = p.parse_nested_block(|p| {
                    let s = p.expect_string()?.as_ref().to_owned();
                    p.expect_exhausted()?;
                    Ok(s)
                })?;
                if !allowed.contains(&target) {
                    return Err(fail(
                        p,
                        format!("url(\"{target}\") isn't one of the uploads"),
                    ));
                }
                let _ = write!(out, "url(\"{target}\")");
                return Ok(());
            }
            if BANNED_FUNCTIONS.contains(&lower.as_str()) {
                return Err(fail(p, format!("{lower}() isn't allowed")));
            }
            let _ = tok.to_css(out);
            p.parse_nested_block(|p| copy_all(p, allowed, out))?;
            out.push(')');
        }
        Token::ParenthesisBlock => {
            out.push('(');
            p.parse_nested_block(|p| copy_all(p, allowed, out))?;
            out.push(')');
        }
        Token::SquareBracketBlock => {
            out.push('[');
            p.parse_nested_block(|p| copy_all(p, allowed, out))?;
            out.push(']');
        }
        Token::CurlyBracketBlock => return Err(fail(p, "a { where a value was expected")),
        Token::BadUrl(_) | Token::BadString(_) => return Err(fail(p, "a broken string or url")),
        Token::AtKeyword(k) => return Err(fail(p, format!("@{k} isn't allowed here"))),
        Token::CDO | Token::CDC => return Err(fail(p, "<!-- and --> aren't allowed")),
        Token::Comment(_) => {}
        Token::Ident(i) if i.eq_ignore_ascii_case("expression") => {
            return Err(fail(p, "expression isn't allowed"));
        }
        other => {
            let _ = other.to_css(out);
        }
    }
    Ok(())
}

fn copy_all<'i>(
    p: &mut Parser<'i, '_>,
    allowed: &[String],
    out: &mut String,
) -> Result<(), Err<'i>> {
    while let Ok(tok) = p.next_including_whitespace_and_comments() {
        let tok = tok.clone();
        copy_token(p, &tok, allowed, out)?;
    }
    Ok(())
}

/// A declaration block's contents: `name: value;` each, re-serialized.
fn declarations<'i>(
    p: &mut Parser<'i, '_>,
    allowed: &[String],
    out: &mut String,
) -> Result<(), Err<'i>> {
    loop {
        let name = match p.next() {
            Ok(Token::Ident(n)) => n.as_ref().to_ascii_lowercase(),
            Ok(Token::Semicolon) => continue,
            Ok(t) => {
                let t = t.clone();
                return Err(fail(
                    p,
                    format!("expected a property name, found {}", t.to_css_string()),
                ));
            }
            Err(_) => return Ok(()),
        };
        if BANNED_PROPERTIES.contains(&name.as_str()) {
            return Err(fail(p, format!("the {name} property isn't allowed")));
        }
        p.expect_colon()?;
        let mut value = String::new();
        p.parse_until_after(cssparser::Delimiter::Semicolon, |p| {
            copy_all(p, allowed, &mut value)
        })?;
        let value = value.trim();
        if value.is_empty() {
            return Err(fail(p, format!("{name} has no value")));
        }
        let _ = write!(out, "{name}: {value}; ");
    }
}

/// Rules (and, outside `@media`, `@media` blocks of rules).
fn rules<'i>(
    p: &mut Parser<'i, '_>,
    allowed: &[String],
    nested: bool,
    out: &mut String,
) -> Result<(), Err<'i>> {
    loop {
        let tok = match p.next() {
            Ok(t) => t.clone(),
            Err(_) => return Ok(()),
        };
        let mut prelude = String::new();
        let media = match &tok {
            Token::AtKeyword(k) if k.eq_ignore_ascii_case("media") && !nested => true,
            Token::AtKeyword(k) => return Err(fail(p, format!("@{k} isn't allowed"))),
            Token::CurlyBracketBlock => return Err(fail(p, "a block with no selector")),
            Token::Semicolon => continue,
            t => {
                copy_token(p, t, allowed, &mut prelude)?;
                false
            }
        };
        // The rest of the prelude, up to the block.
        loop {
            match p.next_including_whitespace_and_comments() {
                Ok(Token::CurlyBracketBlock) => break,
                Ok(Token::Semicolon) => return Err(fail(p, "a ; where a { was expected")),
                Ok(t) => {
                    let t = t.clone();
                    copy_token(p, &t, allowed, &mut prelude)?;
                }
                Err(_) => return Err(fail(p, "a rule with no { … }")),
            }
        }
        let prelude = prelude.trim();
        if media {
            let _ = writeln!(out, "@media {prelude} {{");
            p.parse_nested_block(|p| rules(p, allowed, true, out))?;
            out.push_str("}\n");
        } else {
            let _ = write!(out, "{prelude} {{ ");
            p.parse_nested_block(|p| declarations(p, allowed, out))?;
            out.push_str("}\n");
        }
    }
}

/// Custom CSS, parsed and re-serialized: rules and `@media` only, no other
/// at-rules (`@import`, `@font-face`, …), no `url()` but an upload's exact
/// served path, no `image-set()`, `expression` or `behavior`. Escapes are
/// decoded by the parser before anything is checked.
pub fn sanitize_css(input: &str, allowed: &[String]) -> Result<String, String> {
    if input.trim().is_empty() {
        return Ok(String::new());
    }
    if input.len() > MAX_CSS {
        return Err(format!("it's longer than {} KB", MAX_CSS / 1024));
    }
    let mut pi = ParserInput::new(input);
    let mut p = Parser::new(&mut pi);
    let mut out = String::new();
    rules(&mut p, allowed, false, &mut out).map_err(|e| {
        let line = e.location.line + 1;
        match e.kind {
            cssparser::ParseErrorKind::Custom(m) => format!("{m} (line {line})"),
            other => format!("it doesn't parse ({other:?}, line {line})"),
        }
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UP: &str = "/branding/abc123";

    fn ok(css: &str) -> String {
        sanitize_css(css, &[UP.into()]).unwrap_or_else(|e| panic!("{css}: {e}"))
    }

    fn refused(css: &str) {
        assert!(
            sanitize_css(css, &[UP.into()]).is_err(),
            "{css} was let through"
        );
    }

    #[test]
    fn plain_rules_and_media_survive() {
        let out = ok(
            ".card h2 { color: #123456; margin: 0 auto !important }\n@media (max-width: 40rem) { .card { padding: calc(1rem + 2px) } }",
        );
        assert!(
            out.contains(".card h2 {")
                && out.contains("color: #123456;")
                && out.contains("!important"),
            "{out}"
        );
        assert!(
            out.contains("@media (max-width: 40rem)") && out.contains("calc(1rem + 2px)"),
            "{out}"
        );
        assert!(ok(&format!(".hero {{ background: url({UP}) no-repeat }}")).contains(UP));
        assert!(ok(&format!(".hero {{ background: url(\"{UP}\") }}")).contains(UP));
        assert_eq!(ok("   "), "");
        // The end of the input closes an open block, as CSS says; the
        // output always has its braces.
        assert!(ok(".a { color: red").trim_end().ends_with('}'));
    }

    #[test]
    fn what_loads_or_runs_is_refused_even_escaped() {
        for bad in [
            "@import url(https://evil.example/x.css);",
            "@import 'x.css';",
            "@font-face { font-family: x; src: url(/x) }",
            "@namespace svg url(x);",
            ".a { background: url(https://evil.example/x.png) }",
            ".a { background: url('https://evil.example/x.png') }",
            ".a { background: u\\72l(https://evil.example/x.png) }",
            ".a { background: image-set('x.png' 1x) }",
            ".a { background: -webkit-image-set(url(x) 1x) }",
            ".a { width: expression(alert(1)) }",
            ".a { width: e\\78pression(alert(1)) }",
            ".a { behavior: url(x.htc) }",
            ".a { -moz-binding: url(x.xml#y) }",
            ".a { b\\65havior: x }",
            ".a { background: url(data:image/png;base64,AAAA) }",
            "@media screen { @import 'x'; }",
            "@media screen { @media print { .a { color: red } } }",
            "<!-- .a { color: red } -->",
            "{ color: red }",
        ] {
            refused(bad);
        }
    }

    #[test]
    fn contrast_is_checked_per_mode() {
        assert!((contrast("#000000", "#ffffff").unwrap() - 21.0).abs() < 0.01);
        let mut t = Theme::default();
        t.light.primary = "#1f6feb".into();
        t.dark.primary = "#4d8dff".into();
        assert!(t.problems(&[]).is_empty(), "{:?}", t.problems(&[]));
        t.light.primary = "#eeeeee".into();
        t.dark.error = "red".into();
        let p = t.problems(&[]);
        assert!(
            p.iter()
                .any(|m| m.contains("light primary") && m.contains("too faint")),
            "{p:?}"
        );
        assert!(
            p.iter()
                .any(|m| m.contains("dark error") && m.contains("#1f6feb")),
            "{p:?}"
        );
    }

    #[test]
    fn links_assets_and_lengths_are_checked() {
        let mut t = Theme::default();
        t.links.home = "http://example.com".into();
        t.links.support = "mailto:help@example.com".into();
        t.logo = "nope".into();
        t.text.footer = "x".repeat(MAX_TEXT + 1);
        t.radius = "wobbly".into();
        let p = t.problems(&[("abc".into(), "logo".into())]);
        assert_eq!(p.len(), 4, "{p:?}");
        t = Theme {
            logo: "abc".into(),
            ..Theme::default()
        };
        assert!(t.problems(&[("abc".into(), "logo".into())]).is_empty());
        assert!(
            !t.problems(&[("abc".into(), "font".into())]).is_empty(),
            "the kind must fit"
        );
    }

    #[test]
    fn the_oauth_variables_carry_no_custom_css_or_font() {
        let t = Theme {
            custom_css: ok(".evil { display: none }"),
            uploaded_font: "f1".into(),
            light: Colors {
                primary: "#1f6feb".into(),
                ..Colors::default()
            },
            dark: Colors {
                primary: "#4d8dff".into(),
                ..Colors::default()
            },
            background_light: "bg1".into(),
            ..Theme::default()
        };
        let vars = vars_css(&t, Mode::Auto);
        assert!(
            !vars.contains(".evil") && !vars.contains("@font-face") && !vars.contains("\"Brand\""),
            "{vars}"
        );
        assert!(
            vars.contains("--primary: #1f6feb")
                && vars.contains("prefers-color-scheme: dark")
                && vars.contains("/branding/bg1"),
            "{vars}"
        );
        let full = full_css(&t, Mode::Auto);
        assert!(
            full.contains(".evil") && full.contains("@font-face") && full.contains("/branding/f1"),
            "{full}"
        );
        let dark = vars_css(&t, Mode::Dark);
        assert!(
            dark.contains("--primary: #4d8dff")
                && !dark.contains("#1f6feb")
                && !dark.contains("@media"),
            "{dark}"
        );
    }
}
