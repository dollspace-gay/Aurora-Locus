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

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::{Form, Json};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use super::super::browser_session;
use super::super::html::html_escape;
use crate::context::AppContext;
use crate::identity::did_method::is_web;

/// Where a successful login lands (the holder home).
const HOME_PATH: &str = "/oauth/atproto/holder/home";

#[derive(Debug, Default, Deserialize)]
pub struct LoginForm {
    /// Password method: the handle (or typed DID).
    #[serde(default)]
    pub handle: Option<String>,
    /// Password method: the plaintext password.
    #[serde(default)]
    pub password: Option<String>,
    /// Method discriminant — `login_alpha` selects the key-signing path;
    /// absent/anything else is the password path (Phase 1 back-compat).
    #[serde(default)]
    pub method: Option<String>,
    /// login-α: the holder DID the JS island resolved + signed for.
    #[serde(default)]
    pub did: Option<String>,
    /// login-α: the challenge nonce that was signed.
    #[serde(default)]
    pub nonce: Option<String>,
    /// login-α: base64url(64-byte compact R‖S ES256K signature over
    /// SHA-256(nonce)).
    #[serde(default)]
    pub signature: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChallengeQuery {
    /// Handle or typed DID the holder is signing in as.
    pub identifier: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChallengeResponse {
    /// The resolved did:web DID (what the island signs + submits).
    pub did: String,
    /// Single-use challenge nonce; the island signs `SHA-256(challenge)`.
    pub challenge: String,
}

/// `GET /oauth/atproto/holder/login` — render the login form.
pub async fn login_page(State(ctx): State<AppContext>) -> Html<String> {
    Html(render_login(None, ctx.holder_login_alpha_enabled))
}

/// `GET /oauth/atproto/holder/login/challenge?identifier=…` — issue a login-α
/// challenge for the resolved did:web holder (consumed by `login-alpha.js`).
///
/// Only served when login-α is enabled. Resolves the identifier to a local
/// did:web DID and issues a single-use nonce; a non-did:web or unknown
/// identifier is a 404 (login-α is did:web-only — the `#atproto` key path).
pub async fn login_challenge(
    State(ctx): State<AppContext>,
    Query(q): Query<ChallengeQuery>,
) -> Response {
    if !ctx.holder_login_alpha_enabled {
        return (StatusCode::NOT_FOUND, "login-α is not enabled").into_response();
    }
    match resolve_local_did(&ctx, q.identifier.trim()).await {
        Some(did) if is_web(&did) => {
            let challenge = ctx.browser_login_nonces.generate_nonce().await;
            Json(ChallengeResponse { did, challenge }).into_response()
        }
        _ => (StatusCode::NOT_FOUND, "no such did:web account").into_response(),
    }
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
    // login-α (key-signing) is a distinct submission shape; route to it.
    if form.method.as_deref() == Some("login_alpha") {
        return submit_login_alpha(&ctx, &headers, &form).await;
    }

    let handle = form.handle.as_deref().unwrap_or_default();
    let handle = handle.trim();
    let password = form.password.as_deref().unwrap_or_default();
    // Resolve the identifier (handle or typed DID) to a local DID. A miss
    // collapses into the uniform failure below (no existence oracle).
    let matched_did = match resolve_local_did(&ctx, handle).await {
        Some(did) => {
            let ok = if is_web(&did) {
                // did:web: password lives in holder_auth_method. Touch the
                // matched method on success.
                match ctx.holder_auth_methods.verify_password(&did, password).await {
                    Ok(Some(method_id)) => {
                        let _ = ctx.holder_auth_methods.touch(&method_id).await;
                        true
                    }
                    _ => false,
                }
            } else {
                // did:plc (or other local): password lives in the legacy
                // app_password table.
                verify_app_password(&ctx, &did, password)
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
                Html(render_login(
                    Some("Invalid handle or password."),
                    ctx.holder_login_alpha_enabled,
                )),
            )
                .into_response();
        }
    };

    mint_session_redirect(&ctx, &headers, &did).await
}

/// The login-α (key-signing) branch of the holder login POST. Verifies a
/// signature over `SHA-256(nonce)` against the holder's published
/// `identity_public_key` — reusing β.2's exact verification primitive — and, on
/// success, mints a browser session. did:web only.
///
/// login-α needs no pre-registered `holder_auth_method` row: it proves
/// possession of the account's innate `#atproto` key, so any did:web holder can
/// use it (this is the intended bootstrap credential — a fresh holder with no
/// password can still get in). The auth-methods opt-in row is a UI marker, not
/// a login prerequisite. All failures collapse to a uniform 401 (no oracle).
async fn submit_login_alpha(
    ctx: &AppContext,
    headers: &axum::http::HeaderMap,
    form: &LoginForm,
) -> Response {
    let uniform_401 = || {
        (
            StatusCode::UNAUTHORIZED,
            Html(render_login(
                Some("Could not verify the signature. Check your key and try again."),
                ctx.holder_login_alpha_enabled,
            )),
        )
            .into_response()
    };
    if !ctx.holder_login_alpha_enabled {
        return (StatusCode::NOT_FOUND, "login-α is not enabled").into_response();
    }
    let (did, nonce, signature) = match (&form.did, &form.nonce, &form.signature) {
        (Some(d), Some(n), Some(s)) if !d.is_empty() && !n.is_empty() && !s.is_empty() => {
            (d.as_str(), n.as_str(), s.as_str())
        }
        _ => return uniform_401(),
    };
    if !is_web(did) {
        return uniform_401();
    }
    // Single-use challenge: consume first so a replay cannot reach the
    // (more expensive) signature check.
    match ctx.browser_login_nonces.check_and_consume_nonce(nonce).await {
        Ok(true) => {}
        _ => return uniform_401(),
    }
    // Resolve the holder's published #atproto key. Unknown account → uniform 401.
    let account = match ctx.account_manager.get_did_web_account_by_did(did).await {
        Ok(Some(acct)) => acct,
        _ => return uniform_401(),
    };
    if crate::oauth::atproto::login::verify_login_signature(
        &account.identity_public_key,
        nonce,
        signature,
    )
    .is_err()
    {
        return uniform_401();
    }
    mint_session_redirect(ctx, headers, did).await
}

/// Mint a fresh browser session for `did` and 302 to the holder home. Shared by
/// the password + login-α branches: session-fixation defense (discard any
/// pre-existing session), then a new session + cookie.
async fn mint_session_redirect(
    ctx: &AppContext,
    headers: &axum::http::HeaderMap,
    did: &str,
) -> Response {
    if let Some(old) = browser_session::read_session_cookie(headers) {
        let _ = browser_session::delete_session(&ctx.account_db, &old).await;
    }
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let session = match browser_session::create_session(&ctx.account_db, did, user_agent, None).await
    {
        Ok(session) => session,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(render_login(
                    Some("Could not start a session. Please try again."),
                    ctx.holder_login_alpha_enabled,
                )),
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

/// Render the login page. `error` renders an inline error banner above the
/// form. When `login_alpha_enabled`, the page also offers the login-α
/// (key-signing) method — a second form driven by `login-alpha.js` — and a
/// method picker to switch between them.
fn render_login(error: Option<&str>, login_alpha_enabled: bool) -> String {
    let error_banner = match error {
        Some(msg) => format!(
            "<p class=\"holder-error\" role=\"alert\">{}</p>",
            html_escape(msg)
        ),
        None => String::new(),
    };

    let password_form = "\
  <form method=\"post\" action=\"/oauth/atproto/holder/login\" class=\"holder-form\" data-login-section=\"password\">\n\
    <label for=\"handle\">Handle</label>\n\
    <input id=\"handle\" name=\"handle\" type=\"text\" autocomplete=\"username\" \
autocapitalize=\"none\" autocorrect=\"off\" spellcheck=\"false\" required>\n\
    <label for=\"password\">Password</label>\n\
    <input id=\"password\" name=\"password\" type=\"password\" \
autocomplete=\"current-password\" required>\n\
    <button type=\"submit\">Sign in</button>\n\
  </form>";

    let main = if login_alpha_enabled {
        // Method picker + password form + login-α form. login-alpha.js reveals
        // the selected section, fetches a challenge, signs SHA-256(nonce) with
        // the pasted #atproto key, and submits did/nonce/signature. The private
        // key field has NO `name`, so it never reaches the server even if the
        // island fails to load.
        format!(
            "<main class=\"holder-shell\">\n\
  <h1>Sign in</h1>\n\
  {error_banner}\n\
  <fieldset class=\"holder-methods\">\n\
    <legend>Sign-in method</legend>\n\
    <label><input type=\"radio\" name=\"login_method\" value=\"password\" checked> Password</label>\n\
    <label><input type=\"radio\" name=\"login_method\" value=\"login_alpha\"> Sign with your #atproto key</label>\n\
  </fieldset>\n\
{password_form}\n\
  <form method=\"post\" action=\"/oauth/atproto/holder/login\" class=\"holder-form\" \
id=\"login-alpha-form\" data-login-section=\"login_alpha\" hidden>\n\
    <label for=\"la-identifier\">Handle</label>\n\
    <input id=\"la-identifier\" data-la-identifier type=\"text\" autocomplete=\"username\" \
autocapitalize=\"none\" autocorrect=\"off\" spellcheck=\"false\">\n\
    <label for=\"la-privkey\">Your #atproto private key (hex)</label>\n\
    <textarea id=\"la-privkey\" data-la-privkey rows=\"3\" autocomplete=\"off\" \
autocapitalize=\"none\" autocorrect=\"off\" spellcheck=\"false\"></textarea>\n\
    <input type=\"hidden\" name=\"method\" value=\"login_alpha\">\n\
    <input type=\"hidden\" name=\"did\" data-la-did>\n\
    <input type=\"hidden\" name=\"nonce\" data-la-nonce>\n\
    <input type=\"hidden\" name=\"signature\" data-la-signature>\n\
    <button type=\"submit\">Sign in with key</button>\n\
    <p class=\"holder-note\" data-la-status role=\"status\"></p>\n\
  </form>\n\
  <p class=\"holder-note\">Your private key is used in your browser to sign a \
challenge and is never sent to the server.</p>\n\
  <script type=\"module\" src=\"/holder/login-alpha.js\"></script>\n\
</main>",
            error_banner = error_banner,
            password_form = password_form,
        )
    } else {
        format!(
            "<main class=\"holder-shell\">\n\
  <h1>Sign in</h1>\n\
  {error_banner}\n\
{password_form}\n\
  <p class=\"holder-note\">Passkey and security-key sign-in are coming soon.</p>\n\
</main>",
            error_banner = error_banner,
            password_form = password_form,
        )
    };
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

    /// A password-method login form.
    fn pw_form(handle: &str, password: &str) -> LoginForm {
        LoginForm {
            handle: Some(handle.to_string()),
            password: Some(password.to_string()),
            ..Default::default()
        }
    }

    async fn alpha_disabled_ctx() -> AppContext {
        let mut ctx = ctx().await;
        ctx.holder_login_alpha_enabled = false;
        ctx
    }

    #[tokio::test]
    async fn login_page_renders_form() {
        let html = login_page(State(alpha_disabled_ctx().await)).await.0;
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("name=\"handle\""));
        assert!(html.contains("name=\"password\""));
        // The shared shell links the active theme + holder stylesheet.
        assert!(html.contains("href=\"/theme/active.css\""));
        assert!(html.contains("href=\"/holder/holder.css\""));
        // login-α disabled → "coming soon" note, no key-signing form.
        assert!(html.contains("coming soon"));
        assert!(!html.contains("login-alpha.js"));
    }

    #[tokio::test]
    async fn login_page_offers_login_alpha_when_enabled() {
        let mut ctx = ctx().await;
        ctx.holder_login_alpha_enabled = true;
        let html = login_page(State(ctx)).await.0;
        assert!(html.contains("Sign with your #atproto key"));
        assert!(html.contains("/holder/login-alpha.js"));
        assert!(html.contains("name=\"signature\""));
        assert!(!html.contains("coming soon"));
    }

    #[tokio::test]
    async fn correct_password_mints_session_and_redirects() {
        let ctx = ctx().await;
        let did = "did:web:alice.example.com";
        seed_web_holder(&ctx, did, "alice.example.com", "correct horse").await;

        let resp = submit_login(
            State(ctx.clone()),
            axum::http::HeaderMap::new(),
            Form(pw_form("alice.example.com", "correct horse")),
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
            Form(pw_form("bob.example.com", "a-wrong-guess")),
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
            Form(pw_form("ghost.example.com", "anything")),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = body_string(resp).await;
        assert!(body.contains("Invalid handle or password"));
    }

    // ---------- login-α (key-signing) ----------

    use crate::crypto::secp256k1::Secp256k1KeyPair;
    use k256::ecdsa::{signature::Signer, Signature};

    /// publicKeyMultibase for a keypair (did:key form, multicodec-prefixed).
    fn multibase_pubkey(kp: &Secp256k1KeyPair) -> String {
        kp.did().strip_prefix("did:key:").unwrap().to_string()
    }

    /// Sign `msg` the way the JS island does: ES256K over SHA-256(msg), 64-byte
    /// compact R‖S, base64url.
    fn sign_challenge(kp: &Secp256k1KeyPair, msg: &str) -> String {
        let sig: Signature = kp.signing_key().sign(msg.as_bytes());
        base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            sig.to_bytes(),
        )
    }

    /// Seed a did:web holder with a published identity_public_key (no password).
    async fn seed_web_holder_key(ctx: &AppContext, did: &str, handle: &str, pubkey_mb: &str) {
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(did)
            .bind(handle)
            .bind("2026-01-01T00:00:00Z")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO did_web_account (did, domain, slug, identity_public_key, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(did)
        .bind("example.com")
        .bind(handle.split('.').next().unwrap())
        .bind(pubkey_mb)
        .bind("2026-01-01T00:00:00Z")
        .execute(&ctx.account_db)
        .await
        .unwrap();
    }

    async fn alpha_enabled_ctx() -> AppContext {
        let mut ctx = ctx().await;
        ctx.holder_login_alpha_enabled = true;
        ctx
    }

    async fn issue_challenge(ctx: &AppContext, identifier: &str) -> ChallengeResponse {
        let resp = login_challenge(
            State(ctx.clone()),
            Query(ChallengeQuery {
                identifier: identifier.to_string(),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 8 * 1024).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn alpha_form(did: &str, nonce: &str, sig: &str) -> LoginForm {
        LoginForm {
            method: Some("login_alpha".to_string()),
            did: Some(did.to_string()),
            nonce: Some(nonce.to_string()),
            signature: Some(sig.to_string()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn login_alpha_full_flow_mints_session() {
        let ctx = alpha_enabled_ctx().await;
        let kp = Secp256k1KeyPair::generate();
        let did = "did:web:alice.example.com";
        seed_web_holder_key(&ctx, did, "alice.example.com", &multibase_pubkey(&kp)).await;

        // Challenge issued for the handle resolves to the DID.
        let ch = issue_challenge(&ctx, "alice.example.com").await;
        assert_eq!(ch.did, did);
        let sig = sign_challenge(&kp, &ch.challenge);

        let resp = submit_login(
            State(ctx.clone()),
            axum::http::HeaderMap::new(),
            Form(alpha_form(did, &ch.challenge, &sig)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(set_cookie.contains(browser_session::SESSION_COOKIE));
    }

    #[tokio::test]
    async fn login_alpha_bad_signature_is_401() {
        let ctx = alpha_enabled_ctx().await;
        let kp = Secp256k1KeyPair::generate();
        let did = "did:web:bob.example.com";
        seed_web_holder_key(&ctx, did, "bob.example.com", &multibase_pubkey(&kp)).await;
        let ch = issue_challenge(&ctx, "bob.example.com").await;
        // Sign a different message than the issued nonce.
        let wrong = sign_challenge(&kp, "not-the-nonce");
        let resp = submit_login(
            State(ctx.clone()),
            axum::http::HeaderMap::new(),
            Form(alpha_form(did, &ch.challenge, &wrong)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_alpha_wrong_holder_key_is_401() {
        let ctx = alpha_enabled_ctx().await;
        let holder = Secp256k1KeyPair::generate();
        let attacker = Secp256k1KeyPair::generate();
        let did = "did:web:carol.example.com";
        seed_web_holder_key(&ctx, did, "carol.example.com", &multibase_pubkey(&holder)).await;
        let ch = issue_challenge(&ctx, "carol.example.com").await;
        // Valid signature, but by a key that is not the holder's published one.
        let sig = sign_challenge(&attacker, &ch.challenge);
        let resp = submit_login(
            State(ctx.clone()),
            axum::http::HeaderMap::new(),
            Form(alpha_form(did, &ch.challenge, &sig)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_alpha_replayed_nonce_is_401() {
        let ctx = alpha_enabled_ctx().await;
        let kp = Secp256k1KeyPair::generate();
        let did = "did:web:dave.example.com";
        seed_web_holder_key(&ctx, did, "dave.example.com", &multibase_pubkey(&kp)).await;
        let ch = issue_challenge(&ctx, "dave.example.com").await;
        let sig = sign_challenge(&kp, &ch.challenge);
        // First use succeeds.
        let first = submit_login(
            State(ctx.clone()),
            axum::http::HeaderMap::new(),
            Form(alpha_form(did, &ch.challenge, &sig)),
        )
        .await;
        assert_eq!(first.status(), StatusCode::SEE_OTHER);
        // Replay of the same nonce → 401 (consumed).
        let replay = submit_login(
            State(ctx.clone()),
            axum::http::HeaderMap::new(),
            Form(alpha_form(did, &ch.challenge, &sig)),
        )
        .await;
        assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_alpha_disabled_rejects_challenge_and_post() {
        let ctx = alpha_disabled_ctx().await; // explicitly disabled
        let kp = Secp256k1KeyPair::generate();
        let did = "did:web:erin.example.com";
        seed_web_holder_key(&ctx, did, "erin.example.com", &multibase_pubkey(&kp)).await;
        // Challenge endpoint 404s when disabled.
        let ch = login_challenge(
            State(ctx.clone()),
            Query(ChallengeQuery {
                identifier: "erin.example.com".to_string(),
            }),
        )
        .await;
        assert_eq!(ch.status(), StatusCode::NOT_FOUND);
        // POST 404s too (defense in depth).
        let resp = submit_login(
            State(ctx.clone()),
            axum::http::HeaderMap::new(),
            Form(alpha_form(did, "nonce", "AAAA")),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn login_challenge_unknown_handle_is_404() {
        let ctx = alpha_enabled_ctx().await;
        let resp = login_challenge(
            State(ctx.clone()),
            Query(ChallengeQuery {
                identifier: "ghost.example.com".to_string(),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
