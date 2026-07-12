//! Browser-interactive AS sign-in for local accounts (chainlink #439 follow-on).
//!
//! The atproto authorize flow ([`super::authorize`]) requires an authenticated
//! browser session before it will bind + consent an authorization request.
//! login-α (`/oauth/atproto/login`) covers did:web holders that sign a challenge
//! with their key, but the admin OAuth ceremony authenticates a LOCAL operator
//! with their account password in a browser — which login-α cannot do (it is
//! machine, key-signing, did:web-only). A live-VPS smoke of the admin OAuth flow
//! hit exactly this gap: the authorize→login bounce landed on the login-α
//! challenge endpoint and 400'd on its required `did` query field.
//!
//! This endpoint fills the gap: a server-rendered password form whose POST
//! verifies the account credential (`account.password_hash`, via
//! [`crate::account::AccountManager::verify_password`]) and mints the SAME
//! browser session the authorize/consent handlers read, then returns to the
//! authorize URL.
//!
//! `return_to` is validated to be an authorize URL on THIS server before any
//! redirect, so the endpoint can never be used as an open redirector. All
//! credential failures collapse to one uniform message (no existence oracle).

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;

use super::browser_session;
use super::html::html_escape;
use crate::context::AppContext;

/// Query for the sign-in page: where to return after a successful login.
#[derive(Debug, Deserialize)]
pub struct SigninPageQuery {
    #[serde(default)]
    pub return_to: Option<String>,
}

/// The sign-in form body.
#[derive(Debug, Deserialize)]
pub struct SigninForm {
    #[serde(default)]
    pub identifier: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub return_to: Option<String>,
}

/// `GET /oauth/atproto/signin?return_to=…` — render the sign-in form.
pub async fn signin_page(State(ctx): State<AppContext>, Query(q): Query<SigninPageQuery>) -> Response {
    let return_to = sanitize_return_to(&ctx, q.return_to.as_deref());
    page(StatusCode::OK, &return_to, None)
}

/// `POST /oauth/atproto/signin` — verify the account password, mint a browser
/// session, and 302 to the validated `return_to`.
pub async fn submit_signin(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Form(form): Form<SigninForm>,
) -> Response {
    let return_to = sanitize_return_to(&ctx, form.return_to.as_deref());
    let identifier = form.identifier.as_deref().unwrap_or_default().trim();
    let password = form.password.as_deref().unwrap_or_default();

    // Verify the account credential (account.password_hash) via the same
    // timing-attack-mitigated path the password admin login uses. Any failure —
    // unknown identifier, wrong password, deactivated/taken-down, no local
    // credential — collapses to one uniform message (no existence oracle).
    let account = match ctx.account_manager.verify_password(identifier, password).await {
        Ok(account) => account,
        Err(_) => return page(StatusCode::UNAUTHORIZED, &return_to, Some("Invalid handle or password.")),
    };

    // Session-fixation defense: discard any pre-existing session before minting.
    if let Some(old) = browser_session::read_session_cookie(&headers) {
        let _ = browser_session::delete_session(&ctx.account_db, &old).await;
    }
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let session =
        match browser_session::create_session(&ctx.account_db, &account.did, user_agent, None).await {
            Ok(session) => session,
            Err(_) => {
                return page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &return_to,
                    Some("Could not start a session. Please try again."),
                )
            }
        };

    let cookie = browser_session::set_session_cookie(&session.id);
    // Fetch-driven single-page admin login: when the caller is JS
    // (`Accept: application/json`), set the session cookie and return 204 with NO
    // redirect — the caller's JS drives the subsequent navigation into the OAuth
    // flow, which then finds this session and auto-approves (so the signin page
    // is never rendered). A normal top-level browser navigation
    // (`Accept: text/html` — federated clients, direct visits) still gets the 302
    // back to `return_to`.
    if wants_json(&headers) {
        (StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response()
    } else {
        ([(header::SET_COOKIE, cookie)], Redirect::to(&return_to)).into_response()
    }
}

/// Whether the caller prefers a JSON/programmatic response (a `fetch`) over an
/// HTML page + redirect (a top-level browser navigation).
fn wants_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|accept| accept.contains("application/json"))
        .unwrap_or(false)
}

/// Validate `return_to`: it MUST be an authorize URL on this server. Anything
/// else — off-site, wrong path, or absent — falls back to the bare authorize
/// path, so this endpoint can never be turned into an open redirector.
fn sanitize_return_to(ctx: &AppContext, return_to: Option<&str>) -> String {
    let authorize_url = format!("{}/oauth/atproto/authorize", ctx.service_url());
    match return_to {
        Some(url) if url.starts_with(&authorize_url) => url.to_string(),
        _ => authorize_url,
    }
}

/// Render the sign-in page (with an optional error banner) as a `no-store`
/// HTML response. Every interpolated value is HTML-escaped.
fn page(status: StatusCode, return_to: &str, error: Option<&str>) -> Response {
    let error_html = error
        .map(|e| format!(r#"<p class="error" role="alert">{}</p>"#, html_escape(e)))
        .unwrap_or_default();
    let body = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Sign in</title>
</head>
<body>
  <main>
    <h1>Sign in</h1>
    {error_html}
    <form method="post" action="/oauth/atproto/signin">
      <input type="hidden" name="return_to" value="{return_to}">
      <label>Handle or DID<br>
        <input type="text" name="identifier" autocomplete="username" autofocus required>
      </label><br>
      <label>Password<br>
        <input type="password" name="password" autocomplete="current-password" required>
      </label><br>
      <button type="submit">Sign in</button>
    </form>
  </main>
</body>
</html>"#,
        error_html = error_html,
        return_to = html_escape(return_to),
    );
    (status, [(header::CACHE_CONTROL, "no-store")], Html(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn ctx() -> AppContext {
        crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await
    }

    /// Seed a local account with a real argon2 password hash.
    async fn seed_account(ctx: &AppContext, did: &str, handle: &str, password: &str) {
        let hash = crate::auth::PasswordHasher::hash(password).unwrap();
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(did)
            .bind(handle)
            .bind("2026-01-01T00:00:00Z")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO account (did, email, password_hash, email_confirmed_at, invites_disabled) \
             VALUES ($1, $2, $3, NULL, 0)",
        )
        .bind(did)
        .bind(Some("op@example.test"))
        .bind(&hash)
        .execute(&ctx.account_db)
        .await
        .unwrap();
    }

    async fn body(resp: Response) -> String {
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn authorize_url(ctx: &AppContext) -> String {
        format!("{}/oauth/atproto/authorize?client_id=x&request_uri=y", ctx.service_url())
    }

    #[tokio::test]
    async fn page_renders_form_with_return_to_and_no_store() {
        let ctx = ctx().await;
        let rt = authorize_url(&ctx);
        let resp = signin_page(
            State(ctx.clone()),
            Query(SigninPageQuery {
                return_to: Some(rt.clone()),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get(header::CACHE_CONTROL).unwrap(), "no-store");
        let html = body(resp).await;
        assert!(html.contains(r#"name="identifier""#));
        assert!(html.contains(r#"name="password""#));
        assert!(html.contains(r#"action="/oauth/atproto/signin""#));
        assert!(html.contains(&html_escape(&rt)));
    }

    #[tokio::test]
    async fn offsite_return_to_falls_back_to_authorize_path() {
        let ctx = ctx().await;
        let resp = signin_page(
            State(ctx.clone()),
            Query(SigninPageQuery {
                return_to: Some("https://evil.example.com/steal".to_string()),
            }),
        )
        .await;
        let html = body(resp).await;
        // The hidden field must be the local authorize URL, never the off-site one.
        assert!(html.contains(&format!("{}/oauth/atproto/authorize", ctx.service_url())));
        assert!(!html.contains("evil.example.com"));
    }

    #[tokio::test]
    async fn correct_password_mints_session_and_redirects_to_return_to() {
        let ctx = ctx().await;
        let did = "did:plc:signinadmin000000000000000";
        seed_account(&ctx, did, "admin.localhost", "correct-horse-battery-staple").await;
        let rt = authorize_url(&ctx);

        let resp = submit_signin(
            State(ctx.clone()),
            HeaderMap::new(),
            Form(SigninForm {
                identifier: Some("admin.localhost".to_string()),
                password: Some("correct-horse-battery-staple".to_string()),
                return_to: Some(rt.clone()),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get(header::LOCATION).unwrap().to_str().unwrap(), rt);
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(set_cookie.contains(browser_session::SESSION_COOKIE));
        assert!(set_cookie.contains("HttpOnly"));
    }

    #[tokio::test]
    async fn fetch_mode_returns_204_with_cookie_and_no_redirect() {
        // The single-page admin login path: JS POSTs with Accept: application/json
        // and drives navigation itself, so success is 204 + Set-Cookie, never a
        // 302 (which a fetch would follow into the OAuth flow prematurely).
        let ctx = ctx().await;
        let did = "did:plc:signinadmin000000000000000";
        seed_account(&ctx, did, "admin.localhost", "correct-horse-battery-staple").await;

        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT, "application/json".parse().unwrap());
        let resp = submit_signin(
            State(ctx.clone()),
            headers,
            Form(SigninForm {
                identifier: Some("admin.localhost".to_string()),
                password: Some("correct-horse-battery-staple".to_string()),
                return_to: None,
            }),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(
            resp.headers().get(header::LOCATION).is_none(),
            "fetch mode must not redirect"
        );
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(set_cookie.contains(browser_session::SESSION_COOKIE));
        assert!(set_cookie.contains("HttpOnly"));
    }

    #[tokio::test]
    async fn wrong_password_is_uniform_401() {
        let ctx = ctx().await;
        let did = "did:plc:signinadmin000000000000000";
        seed_account(&ctx, did, "admin.localhost", "the-right-password").await;
        let resp = submit_signin(
            State(ctx.clone()),
            HeaderMap::new(),
            Form(SigninForm {
                identifier: Some("admin.localhost".to_string()),
                password: Some("the-WRONG-password".to_string()),
                return_to: Some(authorize_url(&ctx)),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(body(resp).await.contains("Invalid handle or password"));
    }

    #[tokio::test]
    async fn unknown_identifier_is_uniform_401() {
        let ctx = ctx().await;
        let resp = submit_signin(
            State(ctx.clone()),
            HeaderMap::new(),
            Form(SigninForm {
                identifier: Some("ghost.localhost".to_string()),
                password: Some("whatever".to_string()),
                return_to: Some(authorize_url(&ctx)),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(body(resp).await.contains("Invalid handle or password"));
    }
}
