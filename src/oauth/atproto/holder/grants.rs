//! Holder connected-apps (OAuth grants) management (Holder UI Phase 1,
//! chainlink #424).
//!
//! Lists the holder's active OAuth grants (non-revoked `token` rows) and lets
//! them revoke one by token id — "which apps can access my account, and cut one
//! off". Authenticated via [`BrowserSessionContext`]; the revoke POST carries
//! the session CSRF token and is DID-scoped (a holder revokes only their own
//! grants).

use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;
use sqlx::Row;

use super::view::page_shell;
use crate::context::AppContext;
use crate::error::PdsError;
use crate::oauth::atproto::browser_session::BrowserSessionContext;
use crate::oauth::atproto::html::html_escape;

const PAGE_PATH: &str = "/oauth/atproto/holder/grants";

#[derive(Debug, Deserialize)]
pub struct Banner {
    #[serde(default)]
    pub ok: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RevokeForm {
    pub token_id: String,
    pub csrf_token: String,
}

/// One rendered grant row.
struct Grant {
    token_id: String,
    client_id: String,
    scope: String,
    issued_at: String,
    expires_at: String,
}

fn back(param: &str) -> Response {
    Redirect::to(&format!("{PAGE_PATH}?{param}")).into_response()
}

/// `GET /oauth/atproto/holder/grants`
pub async fn grants_page(
    session: Option<BrowserSessionContext>,
    State(ctx): State<AppContext>,
    Query(banner): Query<Banner>,
) -> Response {
    let Some(session) = session else {
        return Redirect::to(super::LOGIN_PATH).into_response();
    };
    let theme = ctx
        .holder_preferences
        .get(&session.session.did)
        .await
        .ok()
        .and_then(|p| p.theme);
    let grants = load_grants(&ctx, &session.session.did).await.unwrap_or_default();
    let csrf = html_escape(&session.session.csrf_token);
    let body = render_body(&grants, &csrf, banner_html(&banner));
    Html(page_shell("Connected apps", theme.as_deref(), &body)).into_response()
}

/// `POST /oauth/atproto/holder/grants/revoke`
pub async fn revoke(
    session: Option<BrowserSessionContext>,
    State(ctx): State<AppContext>,
    Form(form): Form<RevokeForm>,
) -> Response {
    let Some(session) = session else {
        return Redirect::to(super::LOGIN_PATH).into_response();
    };
    if !super::csrf_ok(&session, &form.csrf_token) {
        return back("error=csrf");
    }
    let now = chrono::Utc::now().to_rfc3339();
    // `revoked = TRUE` is the dual-dialect literal (sqlite 1 / pg boolean).
    let res = sqlx::query(
        "UPDATE token SET revoked = TRUE, revoked_at = $1 \
         WHERE did = $2 AND token_id = $3 AND NOT revoked",
    )
    .bind(&now)
    .bind(&session.session.did)
    .bind(&form.token_id)
    .execute(&ctx.account_db)
    .await
    .map_err(PdsError::Database);
    match res {
        Ok(_) => back("ok=revoked"),
        Err(_) => back("error=generic"),
    }
}

async fn load_grants(ctx: &AppContext, did: &str) -> Result<Vec<Grant>, PdsError> {
    let rows = sqlx::query(
        "SELECT token_id, client_id, scope, created_at, expires_at \
         FROM token WHERE did = $1 AND NOT revoked ORDER BY created_at DESC",
    )
    .bind(did)
    .fetch_all(&ctx.account_db)
    .await
    .map_err(PdsError::Database)?;
    Ok(rows
        .into_iter()
        .map(|r| Grant {
            token_id: r.get("token_id"),
            client_id: r.get("client_id"),
            scope: r.get("scope"),
            issued_at: r.get("created_at"),
            expires_at: r.get("expires_at"),
        })
        .collect())
}

fn banner_html(banner: &Banner) -> String {
    if banner.ok.as_deref() == Some("revoked") {
        return "<p class=\"holder-ok\" role=\"status\">Access revoked.</p>".to_string();
    }
    if let Some(code) = &banner.error {
        let msg = match code.as_str() {
            "csrf" => "Your session expired. Please try again.",
            _ => "Something went wrong. Please try again.",
        };
        return format!("<p class=\"holder-error\" role=\"alert\">{msg}</p>");
    }
    String::new()
}

fn render_body(grants: &[Grant], csrf: &str, banner: String) -> String {
    let rows = if grants.is_empty() {
        "<tr><td colspan=\"4\">No connected apps.</td></tr>".to_string()
    } else {
        grants.iter().map(|g| render_row(g, csrf)).collect::<String>()
    };
    format!(
        "<main class=\"holder-shell\">\n\
  <h1>Connected apps</h1>\n\
  <p><a href=\"/oauth/atproto/holder/home\">&larr; Back to your account</a></p>\n\
  {banner}\n\
  <table class=\"holder-table\">\n\
    <thead><tr><th>App</th><th>Access</th><th>Expires</th><th>Actions</th></tr></thead>\n\
    <tbody>\n{rows}\n</tbody>\n\
  </table>\n\
</main>",
        banner = banner,
        rows = rows,
    )
}

fn render_row(g: &Grant, csrf: &str) -> String {
    format!(
        "<tr>\n\
      <td><code>{client}</code><br><small>issued {issued}</small></td>\n\
      <td>{scope}</td>\n\
      <td>{expires}</td>\n\
      <td>\n\
        <form method=\"post\" action=\"/oauth/atproto/holder/grants/revoke\" style=\"display:inline\">\n\
          <input type=\"hidden\" name=\"token_id\" value=\"{id}\">\n\
          <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\n\
          <button type=\"submit\">Revoke access</button>\n\
        </form>\n\
      </td>\n\
    </tr>",
        client = html_escape(&g.client_id),
        issued = html_escape(&g.issued_at),
        scope = html_escape(&g.scope),
        expires = html_escape(&g.expires_at),
        id = html_escape(&g.token_id),
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

    async fn seed_token(ctx: &AppContext, did: &str, token_id: &str, client_id: &str) {
        sqlx::query(
            "INSERT INTO token (token_id, did, client_id, scope, created_at, updated_at, \
             expires_at, access_token_hash) VALUES ($1,$2,$3,$4,$5,$5,$6,$7)",
        )
        .bind(token_id)
        .bind(did)
        .bind(client_id)
        .bind("atproto transition:generic")
        .bind("2026-01-01T00:00:00Z")
        .bind("2099-01-01T00:00:00Z")
        .bind(format!("hash-{token_id}"))
        .execute(&ctx.account_db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn unauth_redirects() {
        let ctx = ctx().await;
        let resp = grants_page(None, State(ctx.clone()), Query(Banner { ok: None, error: None })).await;
        assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn lists_and_revokes_own_grants() {
        let ctx = ctx().await;
        let did = "did:web:alice.example.com";
        let s = session_for(&ctx, did).await;
        let csrf = s.csrf_token.clone();
        seed_token(&ctx, did, "tok-1", "https://app.example.com/cm.json").await;

        let resp = grants_page(Some(sctx(s.clone())), State(ctx.clone()), Query(Banner { ok: None, error: None })).await;
        let html = String::from_utf8(
            axum::body::to_bytes(resp.into_body(), 128 * 1024).await.unwrap().to_vec(),
        )
        .unwrap();
        assert!(html.contains("https://app.example.com/cm.json"));

        let r = revoke(
            Some(sctx(s)),
            State(ctx.clone()),
            Form(RevokeForm { token_id: "tok-1".to_string(), csrf_token: csrf }),
        )
        .await;
        assert!(r.headers().get(axum::http::header::LOCATION).unwrap().to_str().unwrap().contains("ok=revoked"));
        // Gone from the active list.
        let after = load_grants(&ctx, did).await.unwrap();
        assert!(after.is_empty());
    }

    #[tokio::test]
    async fn revoke_requires_csrf() {
        let ctx = ctx().await;
        let s = session_for(&ctx, "did:web:bob.example.com").await;
        let r = revoke(
            Some(sctx(s)),
            State(ctx.clone()),
            Form(RevokeForm { token_id: "x".to_string(), csrf_token: "wrong".to_string() }),
        )
        .await;
        assert!(r.headers().get(axum::http::header::LOCATION).unwrap().to_str().unwrap().contains("error=csrf"));
    }
}
