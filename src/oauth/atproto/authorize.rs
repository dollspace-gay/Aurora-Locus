//! atproto-OAuth authorize endpoint + consent screen (Arc 2 Phase β.3,
//! chainlink #420 / LOCKED design §3.2 steps 1-5).
//!
//! `GET /oauth/atproto/authorize` is the resource owner's entry point. It
//! resolves the client (either inline parameters or a PAR `request_uri`),
//! verifies the client trust + redirect URI, requires an authenticated browser
//! session (login-α; redirecting to login when absent), persists the
//! authorization request bound to the session DID, and renders the consent
//! screen. The consent POSTs are handled in [`super::consent`].
//!
//! Browser-facing failures render a small HTML error page (not a JSON dump):
//! the resource owner is looking at this in their browser.

use axum::extract::{Query, RawQuery, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use chrono::{Duration, Utc};

use super::browser_session::{self, BrowserSession};
use super::client_metadata::ClientMetadata;
use super::html::html_escape;
use super::params::{self, RawAuthParams};
use super::request_store::{self, AtprotoAuthorizationRequest};
use crate::context::AppContext;

/// Consent-window lifetime: how long the resource owner has to approve/deny
/// after reaching the authorize step.
const CONSENT_WINDOW_SECS: i64 = 600;

/// `GET /oauth/atproto/authorize`
pub async fn authorize(
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    Query(raw): Query<RawAuthParams>,
) -> Response {
    match authorize_inner(&ctx, &headers, raw_query, raw).await {
        Ok(resp) => resp,
        Err(resp) => resp,
    }
}

/// The client + parameters of an authorization request, resolved from either
/// the inline query parameters or a referenced PAR row.
struct Resolved {
    /// `Some(id)` when an existing (PAR) row must be promoted; `None` when a
    /// fresh row must be inserted.
    existing_request_id: Option<String>,
    client_id: String,
    redirect_uri: String,
    scope: String,
    state: Option<String>,
    code_challenge: String,
}

async fn authorize_inner(
    ctx: &AppContext,
    headers: &HeaderMap,
    raw_query: Option<String>,
    raw: RawAuthParams,
) -> Result<Response, Response> {
    // 1. Resolve the request: a PAR `request_uri` reference, or inline params.
    let resolved = resolve_request(ctx, &raw).await?;

    // 2. Resolve + trust the client by its metadata document.
    let metadata = ctx
        .client_metadata_fetcher
        .fetch(&resolved.client_id)
        .await
        .map_err(|e| {
            error_page(
                StatusCode::BAD_REQUEST,
                "Invalid client",
                &format!("Could not resolve the client's metadata: {e}"),
            )
        })?;

    // 3. The redirect_uri must be exactly registered for this client.
    if !metadata.allows_redirect_uri(&resolved.redirect_uri) {
        return Err(error_page(
            StatusCode::BAD_REQUEST,
            "Invalid redirect_uri",
            "redirect_uri not registered for this client.",
        ));
    }

    // 4. Require an authenticated browser session (login-α). Absent → bounce to
    //    login with a return_to back to this authorize URL.
    let session = match resolve_session(ctx, headers).await {
        Some(s) => s,
        None => return Ok(login_redirect(ctx, raw_query.as_deref())),
    };

    // 5. Persist the authorization request bound to the session DID, with the
    //    consent-window TTL. Direct requests insert fresh; PAR requests promote
    //    the pushed row in place.
    let now = Utc::now();
    let consent_expiry = (now + Duration::seconds(CONSENT_WINDOW_SECS)).to_rfc3339();
    let request_id = match &resolved.existing_request_id {
        Some(id) => {
            request_store::promote_par_request(&ctx.account_db, id, &session.did, &consent_expiry)
                .await
                .map_err(internal_error_page)?;
            id.clone()
        }
        None => {
            let id = super::opaque_token();
            let req = AtprotoAuthorizationRequest {
                request_id: id.clone(),
                request_uri: None,
                client_id: resolved.client_id.clone(),
                redirect_uri: resolved.redirect_uri.clone(),
                scope: resolved.scope.clone(),
                state: resolved.state.clone(),
                code_challenge: resolved.code_challenge.clone(),
                code_challenge_method: "S256".to_string(),
                did: Some(session.did.clone()),
                code_hash: None,
                code_used_at: None,
                denied_at: None,
                created_at: now.to_rfc3339(),
                expires_at: consent_expiry,
            };
            request_store::insert(&ctx.account_db, &req)
                .await
                .map_err(internal_error_page)?;
            id
        }
    };

    // 6. Render the consent screen. The CSRF token is the SESSION's token
    //    (β.2's browser_session.csrf_token) — the request_id is a correlation
    //    key, never a trust token (F-3.2).
    Ok(render_consent_screen(
        &request_id,
        &session.csrf_token,
        &metadata,
        &resolved.scope,
        &resolved.redirect_uri,
    ))
}

/// Resolve the request parameters from either a PAR `request_uri` (loaded from
/// the store) or the inline query parameters (validated here).
async fn resolve_request(
    ctx: &AppContext,
    raw: &RawAuthParams,
) -> Result<Resolved, Response> {
    if let Some(request_uri) = raw.request_uri.as_deref().filter(|s| !s.is_empty()) {
        let row = request_store::get_by_request_uri(&ctx.account_db, request_uri)
            .await
            .map_err(internal_error_page)?
            .ok_or_else(|| {
                error_page(
                    StatusCode::BAD_REQUEST,
                    "Invalid request_uri",
                    "The request_uri is unknown or has already been used.",
                )
            })?;
        if row.is_expired(Utc::now()) {
            return Err(error_page(
                StatusCode::BAD_REQUEST,
                "Expired request_uri",
                "The pushed authorization request has expired; please restart.",
            ));
        }
        Ok(Resolved {
            existing_request_id: Some(row.request_id),
            client_id: row.client_id,
            redirect_uri: row.redirect_uri,
            scope: row.scope,
            state: row.state,
            code_challenge: row.code_challenge,
        })
    } else {
        let validated = params::validate(raw).map_err(|e| {
            error_page(StatusCode::BAD_REQUEST, "Invalid request", &e.description())
        })?;
        Ok(Resolved {
            existing_request_id: None,
            client_id: validated.client_id,
            redirect_uri: validated.redirect_uri,
            scope: validated.scope.to_canonical_string(),
            state: validated.state,
            code_challenge: validated.code_challenge,
        })
    }
}

/// Resolve a valid browser session from the request cookie, if any.
async fn resolve_session(ctx: &AppContext, headers: &HeaderMap) -> Option<BrowserSession> {
    let id = browser_session::read_session_cookie(headers)?;
    browser_session::get_valid_session(&ctx.account_db, &id)
        .await
        .ok()
        .flatten()
}

/// 302 to the login endpoint, preserving the authorize URL as `return_to` so
/// the login-α handshake can bounce the holder back here afterward.
fn login_redirect(ctx: &AppContext, raw_query: Option<&str>) -> Response {
    let authorize_url = match raw_query {
        Some(q) if !q.is_empty() => {
            format!("{}/oauth/atproto/authorize?{}", ctx.service_url(), q)
        }
        _ => format!("{}/oauth/atproto/authorize", ctx.service_url()),
    };
    let return_to: String =
        url::form_urlencoded::byte_serialize(authorize_url.as_bytes()).collect();
    let location = format!(
        "{}/oauth/atproto/login?return_to={}",
        ctx.service_url(),
        return_to
    );
    (
        StatusCode::FOUND,
        [(header::LOCATION, location)],
    )
        .into_response()
}

/// Render the HTML consent screen. Every interpolated value is HTML-escaped.
fn render_consent_screen(
    request_id: &str,
    csrf_token: &str,
    metadata: &ClientMetadata,
    scope: &str,
    redirect_uri: &str,
) -> Response {
    let client_name = metadata
        .client_name
        .as_deref()
        .unwrap_or(&metadata.client_id);
    let scope_items: String = scope
        .split_whitespace()
        .map(|s| format!("<li><code>{}</code></li>", html_escape(s)))
        .collect();

    let body = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Authorize application</title>
</head>
<body>
  <main>
    <h1>Authorize <strong>{client_name}</strong></h1>
    <p>The application <strong>{client_name}</strong> is requesting access to your account.</p>
    <h2>Requested permissions</h2>
    <ul>{scope_items}</ul>
    <p>After authorizing, you will be redirected to:<br><code>{redirect_uri}</code></p>
    <form method="post" action="/oauth/atproto/consent/approve" style="display:inline">
      <input type="hidden" name="request_id" value="{request_id}">
      <input type="hidden" name="csrf_token" value="{csrf_token}">
      <button type="submit">Approve</button>
    </form>
    <form method="post" action="/oauth/atproto/consent/deny" style="display:inline">
      <input type="hidden" name="request_id" value="{request_id}">
      <input type="hidden" name="csrf_token" value="{csrf_token}">
      <button type="submit">Deny</button>
    </form>
  </main>
</body>
</html>"#,
        client_name = html_escape(client_name),
        scope_items = scope_items,
        redirect_uri = html_escape(redirect_uri),
        request_id = html_escape(request_id),
        csrf_token = html_escape(csrf_token),
    );

    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Html(body),
    )
        .into_response()
}

/// Minimal HTML error page for browser-facing authorize failures.
fn error_page(status: StatusCode, title: &str, detail: &str) -> Response {
    let body = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>{title}</title></head>
<body><main><h1>{title}</h1><p>{detail}</p></main></body>
</html>"#,
        title = html_escape(title),
        detail = html_escape(detail),
    );
    (status, Html(body)).into_response()
}

fn internal_error_page(e: crate::error::PdsError) -> Response {
    error_page(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Server error",
        &e.to_string(),
    )
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

    fn query(client_id: &str, redirect_uri: &str) -> RawAuthParams {
        RawAuthParams {
            client_id: Some(client_id.to_string()),
            response_type: Some("code".to_string()),
            scope: Some("atproto".to_string()),
            redirect_uri: Some(redirect_uri.to_string()),
            state: Some("st".to_string()),
            code_challenge: Some("chal".to_string()),
            code_challenge_method: Some("S256".to_string()),
            request_uri: None,
        }
    }

    #[tokio::test]
    async fn missing_client_id_renders_error_page() {
        let ctx = ctx().await;
        let mut raw = query("https://app/cm.json", "https://app/cb");
        raw.client_id = None;
        let resp = authorize(
            State(ctx.clone()),
            HeaderMap::new(),
            RawQuery(None),
            Query(raw),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let html = body_string(resp).await;
        assert!(html.contains("Invalid request"));
    }

    #[tokio::test]
    async fn no_session_redirects_to_login_with_return_to() {
        let ctx = ctx().await;
        // Valid inline params but no session cookie → redirect to login. The
        // client metadata fetch must succeed first, so serve a localhost doc.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let client_id = format!("http://127.0.0.1:{port}/client-metadata.json");
        let redirect_uri = "https://app.example.com/cb".to_string();
        let body = format!(
            r#"{{"client_id":"{client_id}","redirect_uris":["{redirect_uri}"]}}"#
        );
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await.unwrap();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
        });

        let raw_query = format!(
            "client_id={}&response_type=code&scope=atproto&redirect_uri={}&state=st&code_challenge=chal&code_challenge_method=S256",
            urlencoding(&client_id),
            urlencoding(&redirect_uri)
        );
        let resp = authorize(
            State(ctx.clone()),
            HeaderMap::new(),
            RawQuery(Some(raw_query)),
            Query(query(&client_id, &redirect_uri)),
        )
        .await;
        server.await.unwrap();

        assert_eq!(resp.status(), StatusCode::FOUND);
        let location = resp
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(location.contains("/oauth/atproto/login?return_to="));
        // The return_to round-trips the authorize URL (percent-encoded).
        assert!(location.contains("authorize"));
    }

    fn urlencoding(s: &str) -> String {
        url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
    }

    #[tokio::test]
    async fn consent_screen_renders_with_request_fields() {
        // Direct render-path unit check: a synthetic metadata + scope produces a
        // consent form carrying the request_id, csrf token, and escaped client
        // name + redirect URI.
        let metadata = ClientMetadata {
            client_id: "https://app.example.com/cm.json".to_string(),
            redirect_uris: vec!["https://app.example.com/cb".to_string()],
            client_name: Some("Cool <App>".to_string()),
            scope: None,
            response_types: None,
            grant_types: None,
            token_endpoint_auth_method: None,
            dpop_bound_access_tokens: None,
            jwks_uri: None,
            application_type: None,
        };
        let resp = render_consent_screen(
            "req-123",
            "csrf-abc",
            &metadata,
            "atproto transition:generic",
            "https://app.example.com/cb",
        );
        assert_eq!(resp.status(), StatusCode::OK);
        let html = body_string(resp).await;
        assert!(html.contains(r#"name="request_id" value="req-123""#));
        assert!(html.contains(r#"name="csrf_token" value="csrf-abc""#));
        assert!(html.contains("/oauth/atproto/consent/approve"));
        assert!(html.contains("/oauth/atproto/consent/deny"));
        // Client name is escaped (no raw angle brackets).
        assert!(html.contains("Cool &lt;App&gt;"));
        assert!(html.contains("<code>atproto</code>"));
        assert!(html.contains("<code>transition:generic</code>"));
    }
}
