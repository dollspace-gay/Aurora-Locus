//! Shared HTML-escaping helper for the atproto-OAuth server-rendered pages.
//!
//! The atproto provider hand-rolls its HTML with `format!` → `Html<String>`
//! (no templating crate; consistent with the codebase). Every interpolated
//! holder- or client-supplied value MUST be escaped so it cannot break out of
//! the surrounding markup. The consent screen ([`super::authorize`]) and the
//! holder self-service pages ([`super::holder`]) both render several such
//! values; they share this one implementation so escaping cannot diverge
//! between them.

/// Escape the five HTML-significant characters so interpolated caller-supplied
/// values (client_name, redirect_uri, scopes, device names, DIDs) cannot break
/// out of the markup.
pub(crate) fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escape_neutralises_markup() {
        assert_eq!(
            html_escape(r#"<script>"&'"#),
            "&lt;script&gt;&quot;&amp;&#x27;"
        );
    }

    #[test]
    fn html_escape_passes_through_plain_text() {
        assert_eq!(html_escape("MacBook home 2026"), "MacBook home 2026");
    }
}
