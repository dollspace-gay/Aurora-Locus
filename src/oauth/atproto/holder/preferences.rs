//! Holder display preferences — theme picker (Holder UI Phase 1, chainlink #424).
//!
//! Lets a holder choose a display theme for their self-service pages. The
//! picker is populated from the installed-theme registry
//! ([`crate::themes::ThemeRegistry`]); an empty selection clears the preference
//! and follows the operator's active theme. Authenticated via
//! [`BrowserSessionContext`]; the save POST carries the session CSRF token.
//!
//! The chosen theme feeds the post-auth pages' `<link rel="stylesheet"
//! href="/theme/active.css?id=…">` (see [`super::view::page_shell`]); an
//! unknown/removed id degrades to the active theme at the serve route, and the
//! save handler additionally validates the id against the registry so only a
//! real, valid theme is ever stored.

use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;

use super::view::page_shell;
use crate::context::AppContext;
use crate::oauth::atproto::browser_session::BrowserSessionContext;
use crate::oauth::atproto::html::html_escape;

const PAGE_PATH: &str = "/oauth/atproto/holder/preferences";

#[derive(Debug, Deserialize)]
pub struct Banner {
    #[serde(default)]
    pub ok: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetPreferencesForm {
    /// Selected theme id; empty string clears the preference.
    #[serde(default)]
    pub theme: String,
    pub csrf_token: String,
}

fn back(param: &str) -> Response {
    Redirect::to(&format!("{PAGE_PATH}?{param}")).into_response()
}

/// `GET /oauth/atproto/holder/preferences`
pub async fn preferences_page(
    session: Option<BrowserSessionContext>,
    State(ctx): State<AppContext>,
    Query(banner): Query<Banner>,
) -> Response {
    let Some(session) = session else {
        return Redirect::to(super::LOGIN_PATH).into_response();
    };
    let did = &session.session.did;
    let current = ctx
        .holder_preferences
        .get(did)
        .await
        .ok()
        .and_then(|p| p.theme);

    // Valid installed themes, id + display name.
    let mut themes: Vec<(String, String)> = ctx
        .theme_registry
        .list()
        .into_iter()
        .filter(|t| t.valid)
        .map(|t| (t.theme_id, t.theme_name))
        .collect();
    themes.sort_by_key(|t| t.1.to_lowercase());

    let csrf = html_escape(&session.session.csrf_token);
    let body = render_body(&themes, current.as_deref(), &csrf, banner_html(&banner));
    // Preview the holder's own chosen theme on this page.
    Html(page_shell("Preferences", current.as_deref(), &body)).into_response()
}

/// `POST /oauth/atproto/holder/preferences`
pub async fn set_preferences(
    session: Option<BrowserSessionContext>,
    State(ctx): State<AppContext>,
    Form(form): Form<SetPreferencesForm>,
) -> Response {
    let Some(session) = session else {
        return Redirect::to(super::LOGIN_PATH).into_response();
    };
    if !super::csrf_ok(&session, &form.csrf_token) {
        return back("error=csrf");
    }
    let did = &session.session.did;
    let selected = form.theme.trim();

    // Empty → clear the preference (follow operator active).
    if selected.is_empty() {
        return match ctx.holder_preferences.set_theme(did, None).await {
            Ok(()) => back("ok=cleared"),
            Err(_) => back("error=generic"),
        };
    }
    // Only a real, valid installed theme id may be stored.
    let is_valid = ctx
        .theme_registry
        .list()
        .into_iter()
        .any(|t| t.valid && t.theme_id == selected);
    if !is_valid {
        return back("error=unknown_theme");
    }
    match ctx.holder_preferences.set_theme(did, Some(selected)).await {
        Ok(()) => back("ok=saved"),
        Err(_) => back("error=generic"),
    }
}

fn banner_html(banner: &Banner) -> String {
    if let Some(code) = &banner.ok {
        let msg = match code.as_str() {
            "saved" => "Theme saved.",
            "cleared" => "Theme reset to the site default.",
            _ => return String::new(),
        };
        return format!("<p class=\"holder-ok\" role=\"status\">{msg}</p>");
    }
    if let Some(code) = &banner.error {
        let msg = match code.as_str() {
            "unknown_theme" => "That theme is not available.",
            "csrf" => "Your session expired. Please try again.",
            _ => "Something went wrong. Please try again.",
        };
        return format!("<p class=\"holder-error\" role=\"alert\">{msg}</p>");
    }
    String::new()
}

fn render_body(themes: &[(String, String)], current: Option<&str>, csrf: &str, banner: String) -> String {
    let default_selected = if current.is_none() { " selected" } else { "" };
    let mut options = format!(
        "<option value=\"\"{default_selected}>Site default (operator theme)</option>"
    );
    for (id, name) in themes {
        let sel = if current == Some(id.as_str()) {
            " selected"
        } else {
            ""
        };
        options.push_str(&format!(
            "<option value=\"{id}\"{sel}>{name}</option>",
            id = html_escape(id),
            sel = sel,
            name = html_escape(name),
        ));
    }
    format!(
        "<main class=\"holder-shell\">\n\
  <h1>Preferences</h1>\n\
  <p><a href=\"/oauth/atproto/holder/home\">&larr; Back to your account</a></p>\n\
  {banner}\n\
  <form method=\"post\" action=\"/oauth/atproto/holder/preferences\" class=\"holder-form\">\n\
    <label for=\"theme\">Display theme</label>\n\
    <select id=\"theme\" name=\"theme\">\n{options}\n</select>\n\
    <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\n\
    <button type=\"submit\">Save</button>\n\
  </form>\n\
</main>",
        banner = banner,
        options = options,
        csrf = csrf,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::atproto::browser_session::{create_session, BrowserSession};

    async fn ctx() -> AppContext {
        crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await
    }

    async fn session_for(ctx: &AppContext, did: &str) -> BrowserSession {
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(did)
            .bind(format!("{}.example.com", did.replace(':', "-")))
            .bind("2026-01-01T00:00:00Z")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        create_session(&ctx.account_db, did, None, None).await.unwrap()
    }

    fn sctx(session: BrowserSession) -> BrowserSessionContext {
        BrowserSessionContext { session }
    }

    #[tokio::test]
    async fn unauth_redirects() {
        let ctx = ctx().await;
        let resp = preferences_page(None, State(ctx.clone()), Query(Banner { ok: None, error: None })).await;
        assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn page_lists_installed_themes() {
        let ctx = ctx().await;
        let s = session_for(&ctx, "did:web:alice.example.com").await;
        let resp = preferences_page(
            Some(sctx(s)),
            State(ctx.clone()),
            Query(Banner { ok: None, error: None }),
        )
        .await;
        let bytes = axum::body::to_bytes(resp.into_body(), 128 * 1024).await.unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(html.contains("Site default"));
        // aurora-classic ships and is valid.
        assert!(html.contains("value=\"aurora-classic\""));
    }

    #[tokio::test]
    async fn save_valid_theme_persists() {
        let ctx = ctx().await;
        let did = "did:web:bob.example.com";
        let s = session_for(&ctx, did).await;
        let csrf = s.csrf_token.clone();
        let resp = set_preferences(
            Some(sctx(s)),
            State(ctx.clone()),
            Form(SetPreferencesForm { theme: "dark".to_string(), csrf_token: csrf }),
        )
        .await;
        assert!(resp.headers().get(axum::http::header::LOCATION).unwrap().to_str().unwrap().contains("ok=saved"));
        assert_eq!(
            ctx.holder_preferences.get(did).await.unwrap().theme.as_deref(),
            Some("dark")
        );
    }

    #[tokio::test]
    async fn save_unknown_theme_is_rejected() {
        let ctx = ctx().await;
        let did = "did:web:carol.example.com";
        let s = session_for(&ctx, did).await;
        let csrf = s.csrf_token.clone();
        let resp = set_preferences(
            Some(sctx(s)),
            State(ctx.clone()),
            Form(SetPreferencesForm { theme: "no-such-theme".to_string(), csrf_token: csrf }),
        )
        .await;
        assert!(resp.headers().get(axum::http::header::LOCATION).unwrap().to_str().unwrap().contains("error=unknown_theme"));
        assert_eq!(ctx.holder_preferences.get(did).await.unwrap().theme, None);
    }

    #[tokio::test]
    async fn empty_selection_clears() {
        let ctx = ctx().await;
        let did = "did:web:dave.example.com";
        let s = session_for(&ctx, did).await;
        let csrf = s.csrf_token.clone();
        ctx.holder_preferences.set_theme(did, Some("ember")).await.unwrap();
        let resp = set_preferences(
            Some(sctx(s)),
            State(ctx.clone()),
            Form(SetPreferencesForm { theme: String::new(), csrf_token: csrf }),
        )
        .await;
        assert!(resp.headers().get(axum::http::header::LOCATION).unwrap().to_str().unwrap().contains("ok=cleared"));
        assert_eq!(ctx.holder_preferences.get(did).await.unwrap().theme, None);
    }

    #[tokio::test]
    async fn csrf_mismatch_rejected() {
        let ctx = ctx().await;
        let s = session_for(&ctx, "did:web:erin.example.com").await;
        let resp = set_preferences(
            Some(sctx(s)),
            State(ctx.clone()),
            Form(SetPreferencesForm { theme: "dark".to_string(), csrf_token: "wrong".to_string() }),
        )
        .await;
        assert!(resp.headers().get(axum::http::header::LOCATION).unwrap().to_str().unwrap().contains("error=csrf"));
    }
}
