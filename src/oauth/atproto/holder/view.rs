//! Shared HTML page shell for the holder self-service pages.
//!
//! Every holder page is a full server-rendered document that links the holder
//! stylesheet (`/holder/holder.css`) and the active theme. Pre-auth pages link
//! the operator's active theme (`/theme/active.css`); post-auth pages may pass a
//! per-holder theme id (`/theme/active.css?id=…`), which the theme serve route
//! resolves with graceful fallback to active for an unknown id.

use super::super::html::html_escape;

/// Wrap `body` (caller-escaped HTML) in a complete HTML document.
///
/// `theme_id` selects a specific theme via `/theme/active.css?id=…`
/// (post-auth per-holder preference); `None` links `/theme/active.css` (the
/// operator's active theme, pre-auth). The id is filtered to a conservative
/// slug charset so it cannot inject query or markup — an empty/garbage id
/// degrades to the active theme.
pub(crate) fn page_shell(title: &str, theme_id: Option<&str>, body: &str) -> String {
    let theme_href = match theme_id {
        Some(id) => {
            let safe: String = id
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if safe.is_empty() {
                "/theme/active.css".to_string()
            } else {
                format!("/theme/active.css?id={safe}")
            }
        }
        None => "/theme/active.css".to_string(),
    };
    format!(
        "<!DOCTYPE html>\n\
<html lang=\"en\">\n\
<head>\n\
  <meta charset=\"utf-8\">\n\
  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
  <title>{title}</title>\n\
  <link rel=\"stylesheet\" href=\"{theme_href}\">\n\
  <link rel=\"stylesheet\" href=\"/holder/holder.css\">\n\
</head>\n\
<body>\n{body}\n</body>\n</html>\n",
        title = html_escape(title),
        theme_href = theme_href,
        body = body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_links_active_theme_when_no_id() {
        let html = page_shell("Sign in", None, "<main>hi</main>");
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<title>Sign in</title>"));
        assert!(html.contains("href=\"/theme/active.css\""));
        assert!(html.contains("href=\"/holder/holder.css\""));
        assert!(html.contains("<main>hi</main>"));
    }

    #[test]
    fn shell_links_specific_theme_when_id_given() {
        let html = page_shell("Home", Some("dark"), "<main/>");
        assert!(html.contains("href=\"/theme/active.css?id=dark\""));
    }

    #[test]
    fn shell_sanitises_theme_id() {
        // Injection attempt in the id is filtered to the slug charset.
        let html = page_shell("Home", Some("dark\"><script>"), "<main/>");
        assert!(html.contains("href=\"/theme/active.css?id=darkscript\""));
        assert!(!html.contains("<script>"));
        // An entirely-invalid id degrades to the active theme.
        let html2 = page_shell("Home", Some("!!!"), "<main/>");
        assert!(html2.contains("href=\"/theme/active.css\""));
    }

    #[test]
    fn shell_escapes_title() {
        let html = page_shell("<x>", None, "");
        assert!(html.contains("<title>&lt;x&gt;</title>"));
    }
}
