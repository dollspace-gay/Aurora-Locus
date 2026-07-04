//! Holder device management (Holder UI Phase 1, chainlink #424).
//!
//! Lists the holder's registered OAuth devices (the ε `atproto_device`
//! registry) and lets them revoke one — the valuable holder-facing surface:
//! see your signed-in devices, revoke a lost or stale one (which cascade-revokes
//! the tokens bound to it, per [`AtprotoDeviceManager::revoke_device`]).
//!
//! Device *registration* is intentionally NOT a holder-UI action: a device's
//! DPoP key is held by the OAuth client that makes bearer requests and is
//! registered during that client's flow. A key generated in the holder-UI
//! browser would be stranded here, useless to any client — so the holder UI
//! manages and revokes devices rather than minting them.
//!
//! Authenticated via [`BrowserSessionContext`]; the revoke POST carries the
//! session CSRF token. DID-scoped (a holder only sees/revokes their own).

use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;

use super::view::page_shell;
use crate::context::AppContext;
use crate::oauth::atproto::browser_session::BrowserSessionContext;
use crate::oauth::atproto::device_manager::AtprotoDeviceRow;
use crate::oauth::atproto::html::html_escape;

const PAGE_PATH: &str = "/oauth/atproto/holder/devices";

#[derive(Debug, Deserialize)]
pub struct Banner {
    #[serde(default)]
    pub ok: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RevokeForm {
    pub device_id: String,
    pub csrf_token: String,
}

fn back(param: &str) -> Response {
    Redirect::to(&format!("{PAGE_PATH}?{param}")).into_response()
}

/// `GET /oauth/atproto/holder/devices`
pub async fn devices_page(
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
    let devices = ctx
        .atproto_device_manager
        .list_devices(&session.session.did)
        .await
        .unwrap_or_default();
    let csrf = html_escape(&session.session.csrf_token);
    let body = render_body(&devices, &csrf, banner_html(&banner));
    Html(page_shell("Devices", theme.as_deref(), &body)).into_response()
}

/// `POST /oauth/atproto/holder/devices/revoke`
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
    match ctx
        .atproto_device_manager
        .revoke_device(&session.session.did, &form.device_id)
        .await
    {
        Ok(()) => back("ok=revoked"),
        Err(_) => back("error=generic"),
    }
}

fn banner_html(banner: &Banner) -> String {
    if banner.ok.as_deref() == Some("revoked") {
        return "<p class=\"holder-ok\" role=\"status\">Device revoked.</p>".to_string();
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

fn render_body(devices: &[AtprotoDeviceRow], csrf: &str, banner: String) -> String {
    let rows = if devices.is_empty() {
        "<tr><td colspan=\"4\">No registered devices.</td></tr>".to_string()
    } else {
        devices.iter().map(|d| render_row(d, csrf)).collect::<String>()
    };
    format!(
        "<main class=\"holder-shell\">\n\
  <h1>Devices</h1>\n\
  <p><a href=\"/oauth/atproto/holder/home\">&larr; Back to your account</a></p>\n\
  {banner}\n\
  <p class=\"holder-note\">Devices are registered when you sign an app in. \
Revoking a device signs it out everywhere and revokes its access tokens.</p>\n\
  <table class=\"holder-table\">\n\
    <thead><tr><th>Device</th><th>First seen</th><th>Last seen</th><th>Actions</th></tr></thead>\n\
    <tbody>\n{rows}\n</tbody>\n\
  </table>\n\
</main>",
        banner = banner,
        rows = rows,
    )
}

fn render_row(d: &AtprotoDeviceRow, csrf: &str) -> String {
    let name = d
        .device_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(html_escape)
        .unwrap_or_else(|| "Unnamed device".to_string());
    let ua = d
        .user_agent
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(html_escape)
        .unwrap_or_default();
    let ua_line = if ua.is_empty() {
        String::new()
    } else {
        format!("<br><small>{ua}</small>")
    };
    format!(
        "<tr>\n\
      <td>{name}{ua_line}</td>\n\
      <td>{created}</td>\n\
      <td>{last_seen}</td>\n\
      <td>\n\
        <form method=\"post\" action=\"/oauth/atproto/holder/devices/revoke\" style=\"display:inline\">\n\
          <input type=\"hidden\" name=\"device_id\" value=\"{id}\">\n\
          <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\n\
          <button type=\"submit\">Revoke</button>\n\
        </form>\n\
      </td>\n\
    </tr>",
        name = name,
        ua_line = ua_line,
        created = html_escape(&d.created_at),
        last_seen = html_escape(&d.last_seen_at),
        id = html_escape(&d.device_id),
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

    fn jwk(x: &str) -> String {
        format!(r#"{{"kty":"EC","crv":"P-256","x":"{x}","y":"stubY"}}"#)
    }

    #[tokio::test]
    async fn unauth_redirects() {
        let ctx = ctx().await;
        let resp = devices_page(None, State(ctx.clone()), Query(Banner { ok: None, error: None })).await;
        assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn lists_and_revokes_own_devices() {
        let ctx = ctx().await;
        let did = "did:web:alice.example.com";
        let s = session_for(&ctx, did).await;
        let csrf = s.csrf_token.clone();
        let dev = ctx
            .atproto_device_manager
            .register_device(did, &jwk("k1"), Some("Phone"), None)
            .await
            .unwrap();

        // Page shows the device.
        let resp = devices_page(Some(sctx(s.clone())), State(ctx.clone()), Query(Banner { ok: None, error: None })).await;
        let html = String::from_utf8(
            axum::body::to_bytes(resp.into_body(), 128 * 1024).await.unwrap().to_vec(),
        )
        .unwrap();
        assert!(html.contains("Phone"));

        // Revoke → gone.
        let r = revoke(
            Some(sctx(s)),
            State(ctx.clone()),
            Form(RevokeForm { device_id: dev.device_id, csrf_token: csrf }),
        )
        .await;
        assert!(r.headers().get(axum::http::header::LOCATION).unwrap().to_str().unwrap().contains("ok=revoked"));
        assert!(ctx.atproto_device_manager.list_devices(did).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn revoke_requires_csrf() {
        let ctx = ctx().await;
        let s = session_for(&ctx, "did:web:bob.example.com").await;
        let r = revoke(
            Some(sctx(s)),
            State(ctx.clone()),
            Form(RevokeForm { device_id: "x".to_string(), csrf_token: "wrong".to_string() }),
        )
        .await;
        assert!(r.headers().get(axum::http::header::LOCATION).unwrap().to_str().unwrap().contains("error=csrf"));
    }
}
