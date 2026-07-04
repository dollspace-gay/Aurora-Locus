//! Holder sign-in method management (Holder UI Phase 1, chainlink #424; SD-A5 =
//! flexible).
//!
//! Lets a holder review and manage their registered auth methods: add a
//! password, remove a method (never the last one), and pick a primary. Passkey
//! and login-α are surfaced as deferred: passkey is always "coming soon" in
//! Phase 1; login-α opt-in is gated by
//! [`AppContext::holder_login_alpha_enabled`] (default off).
//!
//! Every page is authenticated via [`BrowserSessionContext`] (unauth →
//! redirect to login) and every POST carries the session CSRF token. Actions
//! redirect back to the page with a short `?ok=`/`?error=` code that the GET
//! renders as a banner.

use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;

use super::auth_method_manager::AuthMethod;
use super::view::page_shell;
use crate::context::AppContext;
use crate::error::PdsError;
use crate::oauth::atproto::browser_session::BrowserSessionContext;
use crate::oauth::atproto::html::html_escape;

const PAGE_PATH: &str = "/oauth/atproto/holder/auth-methods";

#[derive(Debug, Deserialize)]
pub struct Banner {
    #[serde(default)]
    pub ok: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddPasswordForm {
    pub password: String,
    pub confirm: String,
    pub csrf_token: String,
}

#[derive(Debug, Deserialize)]
pub struct CsrfOnlyForm {
    pub csrf_token: String,
}

#[derive(Debug, Deserialize)]
pub struct MethodIdForm {
    pub id: String,
    pub csrf_token: String,
}

/// Redirect back to the management page carrying a status code.
fn back(param: &str) -> Response {
    Redirect::to(&format!("{PAGE_PATH}?{param}")).into_response()
}

/// `GET /oauth/atproto/holder/auth-methods`
pub async fn auth_methods_page(
    session: Option<BrowserSessionContext>,
    State(ctx): State<AppContext>,
    Query(banner): Query<Banner>,
) -> Response {
    let Some(session) = session else {
        return Redirect::to(super::LOGIN_PATH).into_response();
    };
    let did = &session.session.did;
    let methods = ctx
        .holder_auth_methods
        .list_for_did(did)
        .await
        .unwrap_or_default();
    let csrf = html_escape(&session.session.csrf_token);
    let body = render_body(
        &methods,
        &csrf,
        ctx.holder_login_alpha_enabled,
        banner_html(&banner),
    );
    Html(page_shell("Sign-in methods", None, &body)).into_response()
}

/// `POST /oauth/atproto/holder/auth-methods/add-password`
pub async fn add_password(
    session: Option<BrowserSessionContext>,
    State(ctx): State<AppContext>,
    Form(form): Form<AddPasswordForm>,
) -> Response {
    let Some(session) = session else {
        return Redirect::to(super::LOGIN_PATH).into_response();
    };
    if !super::csrf_ok(&session, &form.csrf_token) {
        return back("error=csrf");
    }
    if form.password != form.confirm {
        return back("error=password_mismatch");
    }
    match ctx
        .holder_auth_methods
        .register_password(&session.session.did, &form.password)
        .await
    {
        Ok(_) => back("ok=password_added"),
        Err(PdsError::Validation(_)) => back("error=password_short"),
        Err(_) => back("error=generic"),
    }
}

/// `POST /oauth/atproto/holder/auth-methods/add-login-alpha`
pub async fn add_login_alpha(
    session: Option<BrowserSessionContext>,
    State(ctx): State<AppContext>,
    Form(form): Form<CsrfOnlyForm>,
) -> Response {
    let Some(session) = session else {
        return Redirect::to(super::LOGIN_PATH).into_response();
    };
    if !super::csrf_ok(&session, &form.csrf_token) {
        return back("error=csrf");
    }
    // Defense in depth: the opt-in button is disabled in the UI when login-α is
    // off, and the endpoint refuses too.
    if !ctx.holder_login_alpha_enabled {
        return back("error=login_alpha_disabled");
    }
    match ctx
        .holder_auth_methods
        .register_login_alpha(&session.session.did)
        .await
    {
        Ok(_) => back("ok=login_alpha_added"),
        Err(PdsError::Conflict(_)) => back("error=login_alpha_exists"),
        Err(_) => back("error=generic"),
    }
}

/// `POST /oauth/atproto/holder/auth-methods/remove`
pub async fn remove(
    session: Option<BrowserSessionContext>,
    State(ctx): State<AppContext>,
    Form(form): Form<MethodIdForm>,
) -> Response {
    let Some(session) = session else {
        return Redirect::to(super::LOGIN_PATH).into_response();
    };
    if !super::csrf_ok(&session, &form.csrf_token) {
        return back("error=csrf");
    }
    match ctx
        .holder_auth_methods
        .remove(&session.session.did, &form.id)
        .await
    {
        Ok(()) => back("ok=removed"),
        Err(PdsError::Validation(_)) => back("error=last_method"),
        Err(PdsError::NotFound(_)) => back("error=not_found"),
        Err(_) => back("error=generic"),
    }
}

/// `POST /oauth/atproto/holder/auth-methods/set-primary`
pub async fn set_primary(
    session: Option<BrowserSessionContext>,
    State(ctx): State<AppContext>,
    Form(form): Form<MethodIdForm>,
) -> Response {
    let Some(session) = session else {
        return Redirect::to(super::LOGIN_PATH).into_response();
    };
    if !super::csrf_ok(&session, &form.csrf_token) {
        return back("error=csrf");
    }
    match ctx
        .holder_auth_methods
        .set_primary(&session.session.did, &form.id)
        .await
    {
        Ok(()) => back("ok=primary_set"),
        Err(PdsError::NotFound(_)) => back("error=not_found"),
        Err(_) => back("error=generic"),
    }
}

/// Map a status code to a user-facing banner, or empty for none/unknown.
fn banner_html(banner: &Banner) -> String {
    if let Some(code) = &banner.ok {
        let msg = match code.as_str() {
            "password_added" => "Password added.",
            "login_alpha_added" => "Key-signing sign-in opted in.",
            "removed" => "Sign-in method removed.",
            "primary_set" => "Primary sign-in method updated.",
            _ => return String::new(),
        };
        return format!("<p class=\"holder-ok\" role=\"status\">{msg}</p>");
    }
    if let Some(code) = &banner.error {
        let msg = match code.as_str() {
            "password_mismatch" => "The passwords did not match.",
            "password_short" => "Password must be at least 8 characters.",
            "last_method" => "You cannot remove your last remaining sign-in method.",
            "login_alpha_disabled" => "Key-signing sign-in is coming soon.",
            "login_alpha_exists" => "Key-signing sign-in is already opted in.",
            "not_found" => "That sign-in method was not found.",
            "csrf" => "Your session expired. Please try again.",
            _ => "Something went wrong. Please try again.",
        };
        return format!("<p class=\"holder-error\" role=\"alert\">{msg}</p>");
    }
    String::new()
}

/// Render the page body: the methods table + add-method controls.
fn render_body(
    methods: &[AuthMethod],
    csrf: &str,
    login_alpha_enabled: bool,
    banner: String,
) -> String {
    let rows = if methods.is_empty() {
        "<tr><td colspan=\"4\">No sign-in methods yet.</td></tr>".to_string()
    } else {
        methods.iter().map(|m| render_row(m, csrf)).collect::<String>()
    };

    let login_alpha_control = if login_alpha_enabled {
        format!(
            "<form method=\"post\" action=\"/oauth/atproto/holder/auth-methods/add-login-alpha\">\n\
      <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\n\
      <button type=\"submit\">Opt in to key-signing sign-in</button>\n\
    </form>"
        )
    } else {
        "<button type=\"button\" disabled>Key-signing sign-in (coming soon)</button>".to_string()
    };

    format!(
        "<main class=\"holder-shell\">\n\
  <h1>Sign-in methods</h1>\n\
  <p><a href=\"/oauth/atproto/holder/home\">&larr; Back to your account</a></p>\n\
  {banner}\n\
  <table class=\"holder-table\">\n\
    <thead><tr><th>Method</th><th>Primary</th><th>Last used</th><th>Actions</th></tr></thead>\n\
    <tbody>\n{rows}\n</tbody>\n\
  </table>\n\
  <h2>Add a sign-in method</h2>\n\
  <form method=\"post\" action=\"/oauth/atproto/holder/auth-methods/add-password\" class=\"holder-form\">\n\
    <label for=\"password\">New password</label>\n\
    <input id=\"password\" name=\"password\" type=\"password\" autocomplete=\"new-password\" required>\n\
    <label for=\"confirm\">Confirm password</label>\n\
    <input id=\"confirm\" name=\"confirm\" type=\"password\" autocomplete=\"new-password\" required>\n\
    <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\n\
    <button type=\"submit\">Add password</button>\n\
  </form>\n\
  <div class=\"holder-actions\">\n\
    <button type=\"button\" disabled>Passkey (coming soon)</button>\n\
    {login_alpha_control}\n\
  </div>\n\
</main>",
        banner = banner,
        rows = rows,
        csrf = csrf,
        login_alpha_control = login_alpha_control,
    )
}

/// Render one method row.
fn render_row(m: &AuthMethod, csrf: &str) -> String {
    let label = html_escape(m.method_type.label());
    let name_suffix = match &m.device_name {
        Some(n) if !n.is_empty() => format!(" ({})", html_escape(n)),
        _ => String::new(),
    };
    let primary_cell = if m.is_primary {
        "<strong>Primary</strong>".to_string()
    } else {
        format!(
            "<form method=\"post\" action=\"/oauth/atproto/holder/auth-methods/set-primary\" style=\"display:inline\">\n\
        <input type=\"hidden\" name=\"id\" value=\"{id}\">\n\
        <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\n\
        <button type=\"submit\">Make primary</button>\n\
      </form>",
            id = html_escape(&m.id),
            csrf = csrf,
        )
    };
    let last_used = m
        .last_used_at
        .as_deref()
        .map(html_escape)
        .unwrap_or_else(|| "Never".to_string());
    format!(
        "<tr>\n\
      <td>{label}{name_suffix}</td>\n\
      <td>{primary_cell}</td>\n\
      <td>{last_used}</td>\n\
      <td>\n\
        <form method=\"post\" action=\"/oauth/atproto/holder/auth-methods/remove\" style=\"display:inline\">\n\
          <input type=\"hidden\" name=\"id\" value=\"{id}\">\n\
          <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\n\
          <button type=\"submit\">Remove</button>\n\
        </form>\n\
      </td>\n\
    </tr>",
        label = label,
        name_suffix = name_suffix,
        primary_cell = primary_cell,
        last_used = last_used,
        id = html_escape(&m.id),
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

    async fn count_methods(ctx: &AppContext, did: &str) -> usize {
        ctx.holder_auth_methods.list_for_did(did).await.unwrap().len()
    }

    #[tokio::test]
    async fn unauth_page_redirects_to_login() {
        let ctx = ctx().await;
        let resp = auth_methods_page(None, State(ctx.clone()), Query(Banner { ok: None, error: None })).await;
        assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn add_password_registers_and_lists() {
        let ctx = ctx().await;
        let did = "did:web:alice.example.com";
        let s = session_for(&ctx, did).await;
        let csrf = s.csrf_token.clone();

        let resp = add_password(
            Some(sctx(s.clone())),
            State(ctx.clone()),
            Form(AddPasswordForm {
                password: "hunter2hunter".to_string(),
                confirm: "hunter2hunter".to_string(),
                csrf_token: csrf,
            }),
        )
        .await;
        assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);
        assert_eq!(count_methods(&ctx, did).await, 1);
        // Verifiable via the login path.
        assert!(ctx
            .holder_auth_methods
            .verify_password(did, "hunter2hunter")
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn add_password_rejects_mismatch_and_short() {
        let ctx = ctx().await;
        let did = "did:web:bob.example.com";
        let s = session_for(&ctx, did).await;
        let csrf = s.csrf_token.clone();

        let mismatch = add_password(
            Some(sctx(s.clone())),
            State(ctx.clone()),
            Form(AddPasswordForm {
                password: "longenough1".to_string(),
                confirm: "different111".to_string(),
                csrf_token: csrf.clone(),
            }),
        )
        .await;
        let loc = mismatch.headers().get(axum::http::header::LOCATION).unwrap().to_str().unwrap();
        assert!(loc.contains("error=password_mismatch"));
        assert_eq!(count_methods(&ctx, did).await, 0);

        let short = add_password(
            Some(sctx(s)),
            State(ctx.clone()),
            Form(AddPasswordForm {
                password: "short".to_string(),
                confirm: "short".to_string(),
                csrf_token: csrf,
            }),
        )
        .await;
        let loc = short.headers().get(axum::http::header::LOCATION).unwrap().to_str().unwrap();
        assert!(loc.contains("error=password_short"));
    }

    #[tokio::test]
    async fn csrf_mismatch_is_rejected() {
        let ctx = ctx().await;
        let s = session_for(&ctx, "did:web:carol.example.com").await;
        let resp = add_password(
            Some(sctx(s)),
            State(ctx.clone()),
            Form(AddPasswordForm {
                password: "longenough1".to_string(),
                confirm: "longenough1".to_string(),
                csrf_token: "wrong".to_string(),
            }),
        )
        .await;
        let loc = resp.headers().get(axum::http::header::LOCATION).unwrap().to_str().unwrap();
        assert!(loc.contains("error=csrf"));
    }

    #[tokio::test]
    async fn remove_enforces_last_method_safety() {
        let ctx = ctx().await;
        let did = "did:web:dave.example.com";
        let s = session_for(&ctx, did).await;
        let csrf = s.csrf_token.clone();
        // Register two methods so one can be removed.
        let id1 = ctx.holder_auth_methods.register_password(did, "passwordone").await.unwrap();
        let _id2 = ctx.holder_auth_methods.register_password(did, "passwordtwo").await.unwrap();

        // Remove one → ok.
        let ok = remove(
            Some(sctx(s.clone())),
            State(ctx.clone()),
            Form(MethodIdForm { id: id1, csrf_token: csrf.clone() }),
        )
        .await;
        assert!(ok.headers().get(axum::http::header::LOCATION).unwrap().to_str().unwrap().contains("ok=removed"));
        assert_eq!(count_methods(&ctx, did).await, 1);

        // Removing the last one → refused.
        let remaining = ctx.holder_auth_methods.list_for_did(did).await.unwrap();
        let last = remove(
            Some(sctx(s)),
            State(ctx.clone()),
            Form(MethodIdForm { id: remaining[0].id.clone(), csrf_token: csrf }),
        )
        .await;
        assert!(last.headers().get(axum::http::header::LOCATION).unwrap().to_str().unwrap().contains("error=last_method"));
        assert_eq!(count_methods(&ctx, did).await, 1);
    }

    #[tokio::test]
    async fn set_primary_moves_the_flag() {
        let ctx = ctx().await;
        let did = "did:web:erin.example.com";
        let s = session_for(&ctx, did).await;
        let csrf = s.csrf_token.clone();
        let _id1 = ctx.holder_auth_methods.register_password(did, "passwordone").await.unwrap();
        let id2 = ctx.holder_auth_methods.register_password(did, "passwordtwo").await.unwrap();

        // id1 was primary (first). Promote id2.
        let resp = set_primary(
            Some(sctx(s)),
            State(ctx.clone()),
            Form(MethodIdForm { id: id2.clone(), csrf_token: csrf }),
        )
        .await;
        assert!(resp.headers().get(axum::http::header::LOCATION).unwrap().to_str().unwrap().contains("ok=primary_set"));
        let methods = ctx.holder_auth_methods.list_for_did(did).await.unwrap();
        let primary: Vec<_> = methods.iter().filter(|m| m.is_primary).collect();
        assert_eq!(primary.len(), 1);
        assert_eq!(primary[0].id, id2);
    }

    #[tokio::test]
    async fn add_login_alpha_disabled_by_default() {
        let ctx = ctx().await;
        let did = "did:web:frank.example.com";
        let s = session_for(&ctx, did).await;
        let csrf = s.csrf_token.clone();
        assert!(!ctx.holder_login_alpha_enabled);
        let resp = add_login_alpha(
            Some(sctx(s)),
            State(ctx.clone()),
            Form(CsrfOnlyForm { csrf_token: csrf }),
        )
        .await;
        assert!(resp.headers().get(axum::http::header::LOCATION).unwrap().to_str().unwrap().contains("error=login_alpha_disabled"));
        assert_eq!(count_methods(&ctx, did).await, 0);
    }

    #[tokio::test]
    async fn page_renders_methods_and_coming_soon() {
        let ctx = ctx().await;
        let did = "did:web:grace.example.com";
        let s = session_for(&ctx, did).await;
        ctx.holder_auth_methods.register_password(did, "passwordone").await.unwrap();
        let resp = auth_methods_page(
            Some(sctx(s)),
            State(ctx.clone()),
            Query(Banner { ok: None, error: None }),
        )
        .await;
        let bytes = axum::body::to_bytes(resp.into_body(), 128 * 1024).await.unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(html.contains("Password"));
        assert!(html.contains("Passkey (coming soon)"));
        assert!(html.contains("Key-signing sign-in (coming soon)"));
        assert!(html.contains("Add password"));
    }
}
