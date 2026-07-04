//! Holder self-service login page (Holder UI Phase 1, chainlink #424).
//!
//! A server-rendered, browser-facing login for the holder self-service UI,
//! distinct from β.2's machine AS-login (`/oauth/atproto/login`, JSON). Phase 1
//! ships the **password** method only (passkey + login-α deferred), so the page
//! is a single handle + password form rather than a multi-step method picker;
//! the picker lands when a second method becomes usable.
//!
//! Password verification routes by DID method: a did:web holder's password
//! lives in `holder_auth_method` (via [`HolderAuthMethodManager::verify_password`]),
//! a did:plc holder's in the legacy `app_password` table. On success the handler
//! mints a browser session ([`super::super::browser_session`]) exactly like
//! β.2's AS-login — same cookie, same session-fixation defense — and redirects
//! to the holder home.
//!
//! Being pre-auth, the POST carries no session-CSRF token (there is no session
//! yet); the security property is the credential, matching β.2. All failure
//! modes collapse to one uniform "invalid handle or password" so the page is not
//! an account-existence oracle.

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;
use sqlx::Row;

use super::super::browser_session;
use super::super::html::html_escape;
use crate::context::AppContext;
use crate::identity::did_method::is_web;

/// Where a successful login lands (the holder home, wired in a later commit).
const HOME_PATH: &str = "/oauth/atproto/holder/home";

#[derive(Debug, Deserialize)]
pub struct LoginForm {
    pub handle: String,
    pub password: String,
}

/// `GET /oauth/atproto/holder/login` — render the login form.
pub async fn login_page() -> Html<String> {
    Html(render_login(None))
}

/// `POST /oauth/atproto/holder/login` — verify the credential and mint a
/// browser session.
///
/// Uniform failure: an unknown handle, a wrong password, and a
/// no-password-method account all render the same 401 page — no existence
/// oracle.
pub async fn submit_login(
    State(ctx): State<AppContext>,
    headers: axum::http::HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    let handle = form.handle.trim();
    // Resolve the identifier (handle or typed DID) to a local DID. A miss
    // collapses into the uniform failure below (no existence oracle).
    let matched_did = match resolve_local_did(&ctx, handle).await {
        Some(did) => {
            let ok = if is_web(&did) {
                // did:web: password lives in holder_auth_method. Touch the
                // matched method on success.
                match ctx.holder_auth_methods.verify_password(&did, &form.password).await {
                    Ok(Some(method_id)) => {
                        let _ = ctx.holder_auth_methods.touch(&method_id).await;
                        true
                    }
                    _ => false,
                }
            } else {
                // did:plc (or other local): password lives in the legacy
                // app_password table.
                verify_app_password(&ctx, &did, &form.password)
                    .await
                    .unwrap_or(false)
            };
            ok.then_some(did)
        }
        None => None,
    };

    let did = match matched_did {
        Some(did) => did,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Html(render_login(Some("Invalid handle or password."))),
            )
                .into_response();
        }
    };

    // Session-fixation defense: discard any pre-existing session before minting
    // a fresh one (mirrors β.2's AS-login).
    if let Some(old) = browser_session::read_session_cookie(&headers) {
        let _ = browser_session::delete_session(&ctx.account_db, &old).await;
    }
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let session = match browser_session::create_session(&ctx.account_db, &did, user_agent, None).await
    {
        Ok(session) => session,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(render_login(Some(
                    "Could not start a session. Please try again.",
                ))),
            )
                .into_response();
        }
    };
    let cookie = browser_session::set_session_cookie(&session.id);
    ([(header::SET_COOKIE, cookie)], Redirect::to(HOME_PATH)).into_response()
}

/// Resolve a login identifier to a local DID. A `did:`-prefixed identifier is
/// taken as-is (verification gates it regardless); a handle is looked up
/// directly against the `actor` table — deliberately NOT via
/// `get_account_by_handle`, whose account-table join errors for a did:web
/// holder that has only an `actor` row and no `account` row.
async fn resolve_local_did(ctx: &AppContext, identifier: &str) -> Option<String> {
    if identifier.starts_with("did:") {
        return Some(identifier.to_string());
    }
    sqlx::query("SELECT did FROM actor WHERE handle = $1")
        .bind(identifier)
        .fetch_optional(&ctx.account_db)
        .await
        .ok()
        .flatten()
        .and_then(|row| row.try_get::<String, _>("did").ok())
}

/// Verify `plaintext` against a did:plc holder's app-passwords. Mirrors the
/// verification loop in `account::manager::login_with_app_password`, but here we
/// only need a yes/no (the caller mints a *browser* session, not a JWT session).
async fn verify_app_password(ctx: &AppContext, did: &str, plaintext: &str) -> crate::error::PdsResult<bool> {
    let rows = sqlx::query("SELECT password_hash FROM app_password WHERE did = $1")
        .bind(did)
        .fetch_all(&ctx.account_db)
        .await
        .map_err(crate::error::PdsError::Database)?;
    for row in &rows {
        let hash: String = row.try_get("password_hash").map_err(crate::error::PdsError::Database)?;
        if let Ok(true) = crate::auth::PasswordHasher::verify(plaintext, &hash) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Render the login page. `error` renders an inline error banner above the form.
fn render_login(error: Option<&str>) -> String {
    let error_banner = match error {
        Some(msg) => format!(
            "<p class=\"holder-error\" role=\"alert\">{}</p>",
            html_escape(msg)
        ),
        None => String::new(),
    };
    let main = format!(
        "<main class=\"holder-shell\">\n\
  <h1>Sign in</h1>\n\
  {error_banner}\n\
  <form method=\"post\" action=\"/oauth/atproto/holder/login\" class=\"holder-form\">\n\
    <label for=\"handle\">Handle</label>\n\
    <input id=\"handle\" name=\"handle\" type=\"text\" autocomplete=\"username\" \
autocapitalize=\"none\" autocorrect=\"off\" spellcheck=\"false\" required>\n\
    <label for=\"password\">Password</label>\n\
    <input id=\"password\" name=\"password\" type=\"password\" \
autocomplete=\"current-password\" required>\n\
    <button type=\"submit\">Sign in</button>\n\
  </form>\n\
  <p class=\"holder-note\">Passkey and security-key sign-in are coming soon.</p>\n\
</main>",
        error_banner = error_banner
    );
    // Pre-auth page: link the operator's active theme (no per-holder id yet).
    super::view::page_shell("Sign in", None, &main)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn ctx() -> AppContext {
        crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await
    }

    async fn body_string(resp: Response) -> String {
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// Seed a local account + (optionally) a did:web password method.
    async fn seed_web_holder(ctx: &AppContext, did: &str, handle: &str, password: &str) {
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(did)
            .bind(handle)
            .bind("2026-01-01T00:00:00Z")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        let hash = crate::auth::PasswordHasher::hash(password).unwrap();
        sqlx::query(
            "INSERT INTO holder_auth_method \
             (id, did, method_type, is_primary, password_hash, password_algo, created_at) \
             VALUES ($1, $2, 'password', $3, $4, 'argon2id', $5)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(did)
        .bind(true)
        .bind(&hash)
        .bind("2026-01-01T00:00:00Z")
        .execute(&ctx.account_db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn login_page_renders_form() {
        let html = login_page().await.0;
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("name=\"handle\""));
        assert!(html.contains("name=\"password\""));
        // The shared shell links the active theme + holder stylesheet.
        assert!(html.contains("href=\"/theme/active.css\""));
        assert!(html.contains("href=\"/holder/holder.css\""));
        assert!(html.contains("coming soon"));
    }

    #[tokio::test]
    async fn correct_password_mints_session_and_redirects() {
        let ctx = ctx().await;
        let did = "did:web:alice.example.com";
        seed_web_holder(&ctx, did, "alice.example.com", "correct horse").await;

        let resp = submit_login(
            State(ctx.clone()),
            axum::http::HeaderMap::new(),
            Form(LoginForm {
                handle: "alice.example.com".to_string(),
                password: "correct horse".to_string(),
            }),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let loc = resp
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(loc, HOME_PATH);
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
        let did = "did:web:bob.example.com";
        seed_web_holder(&ctx, did, "bob.example.com", "the-real-one").await;

        let resp = submit_login(
            State(ctx.clone()),
            axum::http::HeaderMap::new(),
            Form(LoginForm {
                handle: "bob.example.com".to_string(),
                password: "a-wrong-guess".to_string(),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = body_string(resp).await;
        assert!(body.contains("Invalid handle or password"));
    }

    #[tokio::test]
    async fn unknown_handle_is_uniform_401() {
        let ctx = ctx().await;
        let resp = submit_login(
            State(ctx.clone()),
            axum::http::HeaderMap::new(),
            Form(LoginForm {
                handle: "ghost.example.com".to_string(),
                password: "anything".to_string(),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = body_string(resp).await;
        assert!(body.contains("Invalid handle or password"));
    }
}
