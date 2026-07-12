//! Holder sign-out (Holder UI Phase 1, chainlink #424).
//!
//! Deletes the browser session and clears the cookie, then redirects to the
//! login page. CSRF-protected (the sign-out form carries the session token) so
//! a cross-site request cannot force-logout the holder. Idempotent: with no
//! session, it simply redirects.

use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;

use crate::context::AppContext;
use crate::oauth::atproto::browser_session::{self, BrowserSessionContext};

#[derive(Debug, Deserialize)]
pub struct LogoutForm {
    pub csrf_token: String,
}

/// `POST /oauth/atproto/holder/logout`
pub async fn logout(
    session: Option<BrowserSessionContext>,
    State(ctx): State<AppContext>,
    Form(form): Form<LogoutForm>,
) -> Response {
    // With a session, require the CSRF token before tearing it down; on
    // mismatch send the holder home rather than logging them out.
    if let Some(session) = &session {
        if !super::csrf_ok(session, &form.csrf_token) {
            return Redirect::to(super::HOME_PATH).into_response();
        }
        let _ = browser_session::delete_session(&ctx.account_db, &session.session.id).await;
    }
    (
        [(header::SET_COOKIE, browser_session::clear_session_cookie())],
        Redirect::to(super::LOGIN_PATH),
    )
        .into_response()
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
        browser_session::create_session(&ctx.account_db, did, None, None)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn logout_clears_session_and_cookie() {
        let ctx = ctx().await;
        let session = session_for(&ctx, "did:web:a.example.com").await;
        let id = session.id.clone();
        let csrf = session.csrf_token.clone();

        let resp = logout(
            Some(BrowserSessionContext { session }),
            State(ctx.clone()),
            Form(LogoutForm { csrf_token: csrf }),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(set_cookie.contains(browser_session::SESSION_COOKIE));
        // Session row is gone.
        assert!(browser_session::get_valid_session(&ctx.account_db, &id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn logout_csrf_mismatch_keeps_session() {
        let ctx = ctx().await;
        let session = session_for(&ctx, "did:web:b.example.com").await;
        let id = session.id.clone();

        let resp = logout(
            Some(BrowserSessionContext { session }),
            State(ctx.clone()),
            Form(LogoutForm {
                csrf_token: "wrong".to_string(),
            }),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);
        let loc = resp
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(loc, super::super::HOME_PATH);
        // Session survives a bad CSRF.
        assert!(browser_session::get_valid_session(&ctx.account_db, &id)
            .await
            .unwrap()
            .is_some());
    }
}
