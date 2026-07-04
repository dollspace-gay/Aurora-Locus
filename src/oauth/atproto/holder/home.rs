//! Holder home / landing page (Holder UI Phase 1, chainlink #424).
//!
//! The post-login landing. Authenticated via [`BrowserSessionContext`];
//! unauthenticated visitors are redirected to the login page rather than shown
//! a 401 (a browser-facing surface). Navigation links to the holder's
//! management pages, plus a sign-out form carrying the session CSRF token.

use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};

use super::view::page_shell;
use crate::context::AppContext;
use crate::oauth::atproto::browser_session::BrowserSessionContext;
use crate::oauth::atproto::html::html_escape;

/// `GET /oauth/atproto/holder/home`
pub async fn home_page(session: Option<BrowserSessionContext>, State(_ctx): State<AppContext>) -> Response {
    let Some(session) = session else {
        return Redirect::to(super::LOGIN_PATH).into_response();
    };
    let did = html_escape(&session.session.did);
    let csrf = html_escape(&session.session.csrf_token);
    let body = format!(
        "<main class=\"holder-shell\">\n\
  <h1>Your account</h1>\n\
  <p>Signed in as <code>{did}</code>.</p>\n\
  <nav class=\"holder-nav\">\n\
    <a href=\"/oauth/atproto/holder/auth-methods\">Sign-in methods</a>\n\
    <a href=\"/oauth/atproto/holder/devices\">Devices</a>\n\
    <a href=\"/oauth/atproto/holder/grants\">Connected apps</a>\n\
    <a href=\"/oauth/atproto/holder/preferences\">Preferences</a>\n\
  </nav>\n\
  <form method=\"post\" action=\"/oauth/atproto/holder/logout\" class=\"holder-actions\">\n\
    <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\n\
    <button type=\"submit\">Sign out</button>\n\
  </form>\n\
</main>",
        did = did,
        csrf = csrf,
    );
    // Phase 1: operator active theme. The per-holder theme picker (and themed
    // home) land with the preferences page.
    Html(page_shell("Your account", None, &body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::atproto::browser_session::BrowserSession;

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
        crate::oauth::atproto::browser_session::create_session(&ctx.account_db, did, None, None)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn unauthenticated_redirects_to_login() {
        let ctx = ctx().await;
        let resp = home_page(None, State(ctx.clone())).await;
        assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);
        let loc = resp
            .headers()
            .get(axum::http::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(loc, super::super::LOGIN_PATH);
    }

    #[tokio::test]
    async fn authenticated_renders_home() {
        let ctx = ctx().await;
        let session = session_for(&ctx, "did:web:alice.example.com").await;
        let resp = home_page(
            Some(BrowserSessionContext { session }),
            State(ctx.clone()),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(html.contains("did:web:alice.example.com"));
        assert!(html.contains("/oauth/atproto/holder/auth-methods"));
        assert!(html.contains("Sign out"));
    }
}
