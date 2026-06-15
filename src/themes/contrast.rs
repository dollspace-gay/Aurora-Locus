//! WCAG 2.2 contrast verification (§11.10.3/§11.10.4) — validation step 9.
//!
//! Hand-rolled per recon §4 / SD3 (no new dependency): a bounded CSS color
//! parser (#hex, `rgb()`/`rgba()`, `color-mix(in srgb, …)`, `var()` resolved
//! against the theme's token map, plus `white`/`black`/`transparent`) and the
//! WCAG 2.2 relative-luminance / contrast-ratio math. Translucent colors are
//! flattened over their paired surface (and surfaces over white) so the ratio
//! reflects what an operator actually sees.
//!
//! [`verify`] resolves each of the §11.10.3 contrast-requiring token pairs to
//! concrete colors and returns a failure message per pair below its required
//! threshold (fail-closed: an unresolvable token is itself a failure).

use std::collections::HashMap;

/// The §11.10.3 contrast-requiring token pairs: (foreground, background,
/// minimum WCAG ratio).
const CONTRAST_PAIRS: &[(&str, &str, f64)] = &[
    ("--color-text-primary", "--color-surface-primary", 7.0),
    ("--color-text-primary", "--color-surface-secondary", 7.0),
    ("--color-text-primary", "--color-surface-tertiary", 4.5),
    ("--color-text-secondary", "--color-surface-primary", 4.5),
    ("--color-text-secondary", "--color-surface-secondary", 4.5),
    ("--color-text-tertiary", "--color-surface-primary", 3.0),
    ("--color-text-inverted", "--color-accent-primary", 4.5),
    ("--color-border-focus", "--color-surface-primary", 3.0),
    ("--color-border-focus", "--color-surface-secondary", 3.0),
    ("--color-status-success", "--color-surface-primary", 3.0),
    ("--color-status-warning", "--color-surface-primary", 3.0),
    ("--color-status-danger", "--color-surface-primary", 3.0),
    ("--color-status-info", "--color-surface-primary", 3.0),
];

/// Max `var()`/`color-mix()` resolution depth (cycle guard).
const MAX_RESOLVE_DEPTH: usize = 8;

/// An sRGB color with straight alpha, all channels in `0.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Color {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

const WHITE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };

/// Verify the §11.10.3 pairs against a theme's resolved token map (token
/// name → declared value, leaf-wins). Returns one `theme.contrast.failed`
/// message per failing pair.
pub fn verify(tokens: &HashMap<String, String>) -> Vec<String> {
    let mut failures = Vec::new();
    for (fg_name, bg_name, required) in CONTRAST_PAIRS {
        let fg = tokens.get(*fg_name).and_then(|v| parse(v, tokens, 0));
        let bg = tokens.get(*bg_name).and_then(|v| parse(v, tokens, 0));
        match (fg, bg) {
            (Some(fg), Some(bg)) => {
                let bg_flat = flatten(bg, WHITE);
                let fg_flat = flatten(fg, bg_flat);
                let ratio = contrast_ratio(luminance(fg_flat), luminance(bg_flat));
                if ratio + 1e-3 < *required {
                    failures.push(format!(
                        "theme.contrast.failed: {} on {} = {:.2}:1 (need {:.1}:1)",
                        fg_name, bg_name, ratio, required
                    ));
                }
            }
            _ => failures.push(format!(
                "theme.contrast.failed: {} on {} — could not resolve a color value",
                fg_name, bg_name
            )),
        }
    }
    failures
}

/// Flatten `over` (with straight alpha) onto opaque `under`. Returns an
/// opaque color (alpha 1) — `under` is assumed opaque by callers.
fn flatten(over: Color, under: Color) -> Color {
    let a = over.a;
    Color {
        r: over.r * a + under.r * (1.0 - a),
        g: over.g * a + under.g * (1.0 - a),
        b: over.b * a + under.b * (1.0 - a),
        a: 1.0,
    }
}

/// WCAG 2.2 relative luminance of an opaque sRGB color.
fn luminance(c: Color) -> f64 {
    fn lin(x: f64) -> f64 {
        if x <= 0.03928 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// WCAG contrast ratio between two luminances.
fn contrast_ratio(l1: f64, l2: f64) -> f64 {
    let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

/// Parse a CSS color value to a [`Color`], resolving `var()` against the
/// token map and `color-mix(in srgb, …)` recursively. Returns `None` for
/// values outside the supported subset (named colors other than
/// white/black/transparent, gradients, etc.).
fn parse(value: &str, tokens: &HashMap<String, String>, depth: usize) -> Option<Color> {
    if depth > MAX_RESOLVE_DEPTH {
        return None;
    }
    let v = value.trim();

    if let Some(inner) = strip_fn(v, "var") {
        // var(--name) or var(--name, fallback)
        let parts = split_top_level(inner, ',');
        let name = parts.first()?.trim();
        if let Some(resolved) = tokens.get(name) {
            return parse(resolved, tokens, depth + 1);
        }
        // fall back to the second arg if present
        if let Some(fallback) = parts.get(1) {
            return parse(fallback, tokens, depth + 1);
        }
        return None;
    }

    if let Some(inner) = strip_fn(v, "color-mix") {
        return parse_color_mix(inner, tokens, depth);
    }

    if let Some(hex) = v.strip_prefix('#') {
        return parse_hex(hex);
    }

    if let Some(inner) = strip_fn(v, "rgba").or_else(|| strip_fn(v, "rgb")) {
        return parse_rgb(inner);
    }

    match v.to_ascii_lowercase().as_str() {
        "white" => Some(WHITE),
        "black" => Some(Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }),
        "transparent" => Some(Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }),
        _ => None,
    }
}

/// `color-mix(in srgb, <c1> [p1%], <c2> [p2%])` — sRGB component mix.
/// Percentages default to equal weight; alpha is mixed too.
fn parse_color_mix(inner: &str, tokens: &HashMap<String, String>, depth: usize) -> Option<Color> {
    let parts = split_top_level(inner, ',');
    if parts.len() < 3 {
        return None;
    }
    // parts[0] is the color space ("in srgb"); we only support srgb.
    if !parts[0].trim().to_ascii_lowercase().contains("srgb") {
        return None;
    }
    let (c1, p1) = parse_mix_component(parts[1].trim(), tokens, depth)?;
    let (c2, p2) = parse_mix_component(parts[2].trim(), tokens, depth)?;
    // Resolve weights: if both omitted → 50/50; if one omitted → it fills the
    // remainder; normalize so they sum to 1.
    let (w1, w2) = match (p1, p2) {
        (Some(a), Some(b)) => (a, b),
        (Some(a), None) => (a, 100.0 - a),
        (None, Some(b)) => (100.0 - b, b),
        (None, None) => (50.0, 50.0),
    };
    let sum = w1 + w2;
    if sum <= 0.0 {
        return None;
    }
    let (w1, w2) = (w1 / sum, w2 / sum);
    Some(Color {
        r: c1.r * w1 + c2.r * w2,
        g: c1.g * w1 + c2.g * w2,
        b: c1.b * w1 + c2.b * w2,
        a: c1.a * w1 + c2.a * w2,
    })
}

/// A `color-mix` component: `<color> [<pct>%]`.
fn parse_mix_component(
    s: &str,
    tokens: &HashMap<String, String>,
    depth: usize,
) -> Option<(Color, Option<f64>)> {
    // The percentage, if present, is the last whitespace-separated token
    // ending in '%'. Splitting on the LAST space risks breaking rgb( a b c )
    // notation, but color-mix components in our subset use comma-rgb, so the
    // color has no internal spaces once we account for that. Detect a
    // trailing "<num>%".
    if let Some(idx) = s.rfind(char::is_whitespace) {
        let (color_part, pct_part) = s.split_at(idx);
        let pct_part = pct_part.trim();
        if let Some(num) = pct_part.strip_suffix('%') {
            if let Ok(p) = num.trim().parse::<f64>() {
                let color = parse(color_part.trim(), tokens, depth + 1)?;
                return Some((color, Some(p)));
            }
        }
    }
    let color = parse(s, tokens, depth + 1)?;
    Some((color, None))
}

/// `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`.
fn parse_hex(hex: &str) -> Option<Color> {
    let h = hex.trim();
    let expand = |c: char| -> String { format!("{c}{c}") };
    let (r, g, b, a) = match h.len() {
        3 | 4 => {
            let chars: Vec<char> = h.chars().collect();
            let r = u8::from_str_radix(&expand(chars[0]), 16).ok()?;
            let g = u8::from_str_radix(&expand(chars[1]), 16).ok()?;
            let b = u8::from_str_radix(&expand(chars[2]), 16).ok()?;
            let a = if h.len() == 4 {
                u8::from_str_radix(&expand(chars[3]), 16).ok()?
            } else {
                255
            };
            (r, g, b, a)
        }
        6 | 8 => {
            let r = u8::from_str_radix(&h[0..2], 16).ok()?;
            let g = u8::from_str_radix(&h[2..4], 16).ok()?;
            let b = u8::from_str_radix(&h[4..6], 16).ok()?;
            let a = if h.len() == 8 {
                u8::from_str_radix(&h[6..8], 16).ok()?
            } else {
                255
            };
            (r, g, b, a)
        }
        _ => return None,
    };
    Some(Color {
        r: r as f64 / 255.0,
        g: g as f64 / 255.0,
        b: b as f64 / 255.0,
        a: a as f64 / 255.0,
    })
}

/// `rgb(r, g, b)` / `rgba(r, g, b, a)` — channels 0–255 (or %); alpha 0–1.
fn parse_rgb(inner: &str) -> Option<Color> {
    let parts = split_top_level(inner, ',');
    if parts.len() < 3 {
        return None;
    }
    let chan = |s: &str| -> Option<f64> {
        let s = s.trim();
        if let Some(p) = s.strip_suffix('%') {
            p.trim().parse::<f64>().ok().map(|x| (x / 100.0).clamp(0.0, 1.0))
        } else {
            s.parse::<f64>().ok().map(|x| (x / 255.0).clamp(0.0, 1.0))
        }
    };
    let r = chan(parts[0])?;
    let g = chan(parts[1])?;
    let b = chan(parts[2])?;
    let a = match parts.get(3) {
        Some(s) => s.trim().parse::<f64>().ok()?.clamp(0.0, 1.0),
        None => 1.0,
    };
    Some(Color { r, g, b, a })
}

/// If `v` is `name( … )`, return the inner content (case-insensitive name).
fn strip_fn<'a>(v: &'a str, name: &str) -> Option<&'a str> {
    let v = v.trim();
    let lower = v.to_ascii_lowercase();
    let prefix = format!("{name}(");
    if lower.starts_with(&prefix) && v.ends_with(')') {
        Some(&v[prefix.len()..v.len() - 1])
    } else {
        None
    }
}

/// Split `s` on `delim` at parenthesis depth 0 (so `rgb(a,b,c)` stays whole).
fn split_top_level(s: &str, delim: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c == delim && depth == 0 => {
                out.push(&s[start..i]);
                start = i + ch.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(parse("#000", &HashMap::new(), 0), Some(Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }));
        assert_eq!(parse("#ffffff", &HashMap::new(), 0), Some(WHITE));
        let c = parse("#3b82f6", &HashMap::new(), 0).unwrap();
        assert!((c.r - 0.231).abs() < 0.01 && (c.b - 0.965).abs() < 0.01);
    }

    #[test]
    fn black_on_white_is_21() {
        let l_black = luminance(Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 });
        let l_white = luminance(WHITE);
        let ratio = contrast_ratio(l_white, l_black);
        assert!((ratio - 21.0).abs() < 0.1, "got {ratio}");
    }

    #[test]
    fn var_resolution() {
        let m = map(&[("--accent", "#3b82f6")]);
        assert_eq!(parse("var(--accent)", &m, 0), parse("#3b82f6", &m, 0));
        // missing var falls back to the second arg
        assert_eq!(parse("var(--missing, #000)", &m, 0), parse("#000", &m, 0));
    }

    #[test]
    fn rgba_and_flatten() {
        // 50%-black over white → mid-grey
        let c = parse("rgba(0,0,0,0.5)", &HashMap::new(), 0).unwrap();
        let flat = flatten(c, WHITE);
        assert!((flat.r - 0.5).abs() < 1e-6);
    }

    #[test]
    fn color_mix_srgb() {
        // 50/50 mix of black and white → grey 0.5
        let c = parse("color-mix(in srgb, #000 50%, #fff 50%)", &HashMap::new(), 0).unwrap();
        assert!((c.r - 0.5).abs() < 1e-6 && (c.g - 0.5).abs() < 1e-6);
        // one omitted percentage fills the remainder
        let c2 = parse("color-mix(in srgb, #000 25%, #fff)", &HashMap::new(), 0).unwrap();
        assert!((c2.r - 0.75).abs() < 1e-6);
    }

    #[test]
    fn verify_passes_high_contrast_and_fails_low() {
        // Build a minimal token map covering the pairs, all high-contrast.
        let mut good: HashMap<String, String> = HashMap::new();
        for (fg, bg, _) in CONTRAST_PAIRS {
            good.insert(fg.to_string(), "#000000".to_string());
            good.insert(bg.to_string(), "#ffffff".to_string());
        }
        // accent-primary is a bg for text-inverted; #000 bg + #000 fg would
        // fail, so set accent light and inverted dark for that pair.
        good.insert("--color-accent-primary".to_string(), "#ffffff".to_string());
        good.insert("--color-text-inverted".to_string(), "#000000".to_string());
        assert!(verify(&good).is_empty(), "all-black-on-white passes: {:?}", verify(&good));

        // Low-contrast: grey on grey fails the 7:1 text pairs.
        let mut bad = good.clone();
        bad.insert("--color-text-primary".to_string(), "#888888".to_string());
        bad.insert("--color-surface-primary".to_string(), "#999999".to_string());
        let fails = verify(&bad);
        assert!(
            fails.iter().any(|f| f.contains("--color-text-primary on --color-surface-primary")),
            "low contrast flagged: {fails:?}"
        );
    }

    #[test]
    fn unresolvable_token_fails_closed() {
        let mut m: HashMap<String, String> = HashMap::new();
        for (fg, bg, _) in CONTRAST_PAIRS {
            m.insert(fg.to_string(), "#000000".to_string());
            m.insert(bg.to_string(), "#ffffff".to_string());
        }
        // a named color outside the supported subset → unresolvable → failure
        m.insert("--color-text-primary".to_string(), "rebeccapurple".to_string());
        let fails = verify(&m);
        assert!(fails.iter().any(|f| f.contains("could not resolve")));
    }
}
