//! The OAuth sign-in pages, server-rendered (no scripts but the form-post
//! helper): `/oauth/authorize` opens a request on this device and shows the
//! account picker, sign-in (with email 2FA) or consent; the form posts sign
//! in, pick an account, approve the scopes the user keeps, or deny. As the
//! reference: device cookies with rotation and session-fixation handling,
//! sign-in good for 7 days per device, email 2FA skipped for an account
//! seen on the device in the last 30 days, silent sign-on only where
//! allowed. CSRF: a double-submit token plus `Sec-Fetch-Site`.

use std::sync::Arc;

use askama::Template;
use axum::body::Bytes;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hmac::{Hmac, Mac};
use locus_auth::oauth::OAuthError;
use locus_auth::oauth::client::Client;
use locus_auth::oauth::request::{self as oreq, Parameters};
use locus_store::accounts::{Account, Status};
use serde_json::{Map, Value};
use sha2::Sha256;

use crate::state::AppState;

type St = State<Arc<AppState>>;

/// Sign-in on a device lasts this long before the password is asked again.
const AUTHENTICATION_MAX_AGE_MS: i64 = 7 * 24 * 60 * 60_000;
/// Email 2FA is skipped for an account seen on the device this recently.
const TWO_FACTOR_SKIP_MS: i64 = 30 * 24 * 60 * 60_000;
const DEVICE_ROTATION_MS: i64 = 5 * 60_000;
const SESSION_FIXATION_MS: i64 = 30_000;
const COOKIE_AGE_S: i64 = 10 * 365 * 24 * 60 * 60;

// ---------------------------------------------------------------- templates

struct AccountChoice {
    did: String,
    handle: String,
    login_required: bool,
}

#[derive(Template)]
#[template(path = "oauth/sign_in.html")]
struct SignInPage {
    service_name: String,
    client_name: String,
    client_host: String,
    error: String,
    notice: String,
    accounts: Vec<AccountChoice>,
    code_required: bool,
    csrf: String,
    request_id: String,
    identifier: String,
    remember: bool,
}

struct ScopeLine {
    value: String,
    label: String,
    detail: String,
    required: bool,
}

#[derive(Template)]
#[template(path = "oauth/consent.html")]
struct ConsentPage {
    service_name: String,
    client_name: String,
    client_host: String,
    csrf: String,
    request_id: String,
    did: String,
    handle: String,
    scopes: Vec<ScopeLine>,
}

#[derive(Template)]
#[template(path = "oauth/error.html")]
struct ErrorPage {
    service_name: String,
    message: String,
}

#[derive(Template)]
#[template(path = "oauth/form_post.html")]
struct FormPostPage {
    service_name: String,
    action: String,
    fields: Vec<(String, String)>,
}

fn service_name(s: &AppState) -> String {
    let n = s.snapshot().text("server.service_name").to_owned();
    if n.is_empty() { s.hostname.clone() } else { n }
}

/// The page security headers: strict CSP (`form-action` widened only for a
/// form-post redirect), no framing, no caching.
fn page(html: String, status: StatusCode, form_action: Option<&str>) -> Response {
    let mut res = (status, html).into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    let csp = format!(
        "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
         form-action 'self'{}; frame-ancestors 'none'; base-uri 'none'",
        form_action.map(|a| format!(" {a}")).unwrap_or_default()
    );
    if let Ok(v) = HeaderValue::from_str(&csp) {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    res
}

fn error_page(s: &AppState, message: &str, status: StatusCode) -> Response {
    let html = ErrorPage {
        service_name: service_name(s),
        message: message.to_owned(),
    }
    .render()
    .unwrap_or_default();
    page(html, status, None)
}

// ---------------------------------------------------------------- cookies

fn cookies(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|p| {
            p.trim()
                .split_once('=')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
        })
        .collect()
}

fn cookie<'a>(all: &'a [(String, String)], name: &str) -> Option<&'a str> {
    all.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

fn sign(s: &AppState, value: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(&s.secrets.jwt).expect("any key length");
    mac.update(b"oauth-cookie:");
    mac.update(value.as_bytes());
    locus_auth::oauth::jose::b64(&mac.finalize().into_bytes())
}

fn verified<'a>(
    s: &AppState,
    all: &'a [(String, String)],
    name: &str,
    prefix: &str,
) -> Option<&'a str> {
    let v = cookie(all, name).filter(|v| v.starts_with(prefix) && v.len() == prefix.len() + 32)?;
    let hash = cookie(all, &format!("{name}:hash"))?;
    use subtle::ConstantTimeEq;
    bool::from(sign(s, v).as_bytes().ct_eq(hash.as_bytes())).then_some(v)
}

fn set_cookie(s: &AppState, out: &mut Vec<String>, name: &str, value: &str) {
    let secure = if s.public_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    out.push(format!(
        "{name}={value}; Path=/oauth; Max-Age={COOKIE_AGE_S}; HttpOnly; SameSite=Lax{secure}"
    ));
}

/// This browser's device: its id from the signed cookies (rotated as the
/// reference does), or a new one.
async fn load_device(
    s: &AppState,
    headers: &HeaderMap,
    out: &mut Vec<String>,
) -> Result<String, OAuthError> {
    let all = cookies(headers);
    let ip = crate::ratelimit::client_ip().map_or_else(|| "unknown".to_owned(), |i| i.to_string());
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let store_err = |e: locus_core::CoreError| OAuthError::server_error(e.to_string());
    let device = verified(s, &all, "dev-id", "dev-");
    let session = verified(s, &all, "ses-id", "ses-");
    if let (Some(dev), Some(ses)) = (device, session)
        && let Some((current, old_ua, old_ip, seen)) =
            s.store.oauth_device(dev).await.map_err(store_err)?
    {
        let age = locus_core::time::now_ms() - seen;
        let mut rotate = false;
        if ses != current {
            if age <= SESSION_FIXATION_MS {
                rotate = true;
            } else {
                s.store.oauth_delete_device(dev).await.map_err(store_err)?;
                return create_device(s, out, &ip, ua.as_deref()).await;
            }
        }
        if rotate || old_ip != ip || (ua.is_some() && ua != old_ua) || age > DEVICE_ROTATION_MS {
            let ses = oreq::new_session_id();
            s.store
                .oauth_upsert_device(dev, &ses, ua.as_deref().or(old_ua.as_deref()), &ip)
                .await
                .map_err(store_err)?;
            set_cookie(s, out, "ses-id", &ses);
            set_cookie(s, out, "ses-id:hash", &sign(s, &ses));
        }
        return Ok(dev.to_owned());
    }
    if let Some(dev) = device {
        s.store.oauth_delete_device(dev).await.map_err(store_err)?;
    }
    create_device(s, out, &ip, ua.as_deref()).await
}

async fn create_device(
    s: &AppState,
    out: &mut Vec<String>,
    ip: &str,
    ua: Option<&str>,
) -> Result<String, OAuthError> {
    let (dev, ses) = (oreq::new_device_id(), oreq::new_session_id());
    s.store
        .oauth_upsert_device(&dev, &ses, ua, ip)
        .await
        .map_err(|e| OAuthError::server_error(e.to_string()))?;
    for (n, v) in [("dev-id", dev.clone()), ("ses-id", ses.clone())] {
        set_cookie(s, out, n, &v);
        set_cookie(s, out, &format!("{n}:hash"), &sign(s, &v));
    }
    Ok(dev)
}

/// The CSRF token (the cookie's, or a new one to set).
fn csrf_token(s: &AppState, headers: &HeaderMap, out: &mut Vec<String>) -> String {
    let all = cookies(headers);
    if let Some(t) = cookie(&all, "csrf-token").filter(|t| t.len() == 32) {
        return t.to_owned();
    }
    let t = hex::encode(rand::random::<[u8; 16]>());
    let secure = if s.public_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    out.push(format!(
        "csrf-token={t}; Path=/oauth; HttpOnly; SameSite=Lax{secure}"
    ));
    t
}

fn with_cookies(mut res: Response, set: &[String]) -> Response {
    for c in set {
        if let Ok(v) = HeaderValue::from_str(c) {
            res.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    res
}

/// Refuses a form post without the matching CSRF token, or from another site.
fn check_post(headers: &HeaderMap, form: &[(String, String)]) -> Result<(), String> {
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok())
        && site != "same-origin"
    {
        return Err("Request refused (cross-site)".into());
    }
    let all = cookies(headers);
    let cookie_token = cookie(&all, "csrf-token").unwrap_or("");
    let field = form
        .iter()
        .find(|(k, _)| k == "csrf")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    use subtle::ConstantTimeEq;
    if cookie_token.is_empty() || !bool::from(cookie_token.as_bytes().ct_eq(field.as_bytes())) {
        return Err("Request refused (the form expired; go back and try again)".into());
    }
    Ok(())
}

fn form_value<'a>(form: &'a [(String, String)], k: &str) -> Option<&'a str> {
    form.iter()
        .find(|(key, _)| key == k)
        .map(|(_, v)| v.as_str())
}

// ---------------------------------------------------------------- redirects

/// Sends the result to the client's redirect URI (query, fragment or
/// form_post, with `iss` and `state`).
fn respond_to_client(
    s: &AppState,
    p: &Parameters,
    mut fields: Vec<(String, String)>,
    set: &[String],
) -> Response {
    let Some(redirect) = p.redirect_uri.clone() else {
        return with_cookies(
            error_page(
                s,
                "The request has no redirect URI",
                StatusCode::BAD_REQUEST,
            ),
            set,
        );
    };
    if let Some(st) = &p.state {
        fields.push(("state".into(), st.clone()));
    }
    fields.push(("iss".into(), crate::oauth::issuer(s)));
    let mode = p.response_mode.as_deref().unwrap_or("query");
    if mode == "form_post" {
        let origin = url::Url::parse(&redirect)
            .map(|u| u.origin().ascii_serialization())
            .unwrap_or_default();
        let html = FormPostPage {
            service_name: service_name(s),
            action: redirect,
            fields,
        }
        .render()
        .unwrap_or_default();
        return with_cookies(page(html, StatusCode::OK, Some(&origin)), set);
    }
    let encoded: String = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .finish();
    let sep = if mode == "fragment" {
        '#'
    } else if redirect.contains('?') {
        '&'
    } else {
        '?'
    };
    let mut res = StatusCode::SEE_OTHER.into_response();
    if let Ok(v) = HeaderValue::from_str(&format!("{redirect}{sep}{encoded}")) {
        res.headers_mut().insert(header::LOCATION, v);
    }
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    with_cookies(res, set)
}

fn redirect_error(s: &AppState, p: &Parameters, e: &OAuthError, set: &[String]) -> Response {
    respond_to_client(
        s,
        p,
        vec![
            ("error".into(), e.error.clone()),
            ("error_description".into(), e.description.clone()),
        ],
        set,
    )
}

// ---------------------------------------------------------------- scopes in words

fn describe(scope: &str, sets: &[(locus_auth::scopes::IncludeScope, Value)]) -> (String, String) {
    use locus_auth::scopes::*;
    let s = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    match scope {
        "atproto" => {
            return s(
                "Identify you by your account",
                "Required: lets the app know who you are.",
            );
        }
        "transition:generic" => {
            return s(
                "Read and write your account data",
                "Everything except direct messages and your email address.",
            );
        }
        "transition:chat.bsky" => return s("Read and send your direct messages", ""),
        "transition:email" => return s("See your email address", ""),
        _ => {}
    }
    if let Some(r) = RepoPermission::parse(scope) {
        let what = r
            .collection
            .iter()
            .map(|c| {
                if c == "*" {
                    "any kind of record".to_owned()
                } else {
                    c.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let actions = r.action.join(", ");
        return (
            format!("Change records: {what}"),
            format!("Allowed: {actions}"),
        );
    }
    if let Some(b) = BlobPermission::parse(scope) {
        return (
            format!("Upload files ({})", b.accept.join(", ")),
            String::new(),
        );
    }
    if let Some(r) = RpcPermission::parse(scope) {
        let aud = if r.aud == "*" {
            "any service".to_owned()
        } else {
            r.aud.clone()
        };
        return (format!("Use {} on {aud}", r.lxm.join(", ")), String::new());
    }
    if let Some(a) = AccountPermission::parse(scope) {
        let manage = a.action.iter().any(|x| x == "manage");
        return match (a.attr.as_str(), manage) {
            ("email", false) => s("See your email address", ""),
            ("email", true) => s("Manage your email address", ""),
            ("repo", _) => s(
                "Import a repository into your account",
                "Replaces your records with the imported ones.",
            ),
            ("status", true) => s("Deactivate or reactivate your account", ""),
            ("status", false) => s("See your account status", ""),
            _ => (scope.to_owned(), String::new()),
        };
    }
    if let Some(i) = IdentityPermission::parse(scope) {
        return if i.attr == "handle" {
            s("Change your handle", "")
        } else {
            s(
                "Manage your identity",
                "Including your DID document and signing keys.",
            )
        };
    }
    if let Some(inc) = IncludeScope::parse(scope) {
        let set = sets
            .iter()
            .find(|(i, _)| i.nsid == inc.nsid)
            .map(|(_, v)| v);
        let title = set
            .and_then(|v| v.get("title"))
            .and_then(Value::as_str)
            .unwrap_or(&inc.nsid)
            .to_owned();
        let detail = set
            .and_then(|v| v.get("detail"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        return (title, detail);
    }
    (scope.to_owned(), String::new())
}

// ---------------------------------------------------------------- the flow

fn client_name(c: &Client) -> String {
    c.metadata
        .client_name()
        .map_or_else(|| client_host(c), str::to_owned)
}

fn client_host(c: &Client) -> String {
    url::Url::parse(&c.id)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| c.id.clone())
}

/// The accounts signed in on this device, with whether each must enter its
/// password again.
async fn device_accounts(
    s: &AppState,
    device: &str,
) -> Result<Vec<(Account, bool, i64)>, OAuthError> {
    let rows = s
        .store
        .oauth_device_accounts(device)
        .await
        .map_err(|e| OAuthError::server_error(e.to_string()))?;
    let now = locus_core::time::now_ms();
    let mut out = Vec::new();
    for (did, _, updated) in rows {
        if let Some(a) = s
            .store
            .account_by_did(&did)
            .await
            .ok()
            .flatten()
            .filter(|a| a.status != Status::Deleted)
        {
            out.push((a, now - updated > AUTHENTICATION_MAX_AGE_MS, updated));
        }
    }
    Ok(out)
}

struct Ctx {
    client: Client,
    params: Parameters,
    request_id: String,
    device: String,
    csrf: String,
    set: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
fn sign_in_page(
    s: &AppState,
    c: &Ctx,
    accounts: &[(Account, bool, i64)],
    error: &str,
    notice: &str,
    identifier: &str,
    code_required: bool,
    remember: bool,
) -> Response {
    let html = SignInPage {
        service_name: service_name(s),
        client_name: client_name(&c.client),
        client_host: client_host(&c.client),
        error: error.to_owned(),
        notice: notice.to_owned(),
        accounts: accounts
            .iter()
            .filter(|(a, _, _)| a.status == Status::Active)
            .map(|(a, login, _)| AccountChoice {
                did: a.did.clone(),
                handle: a.handle.clone(),
                login_required: *login,
            })
            .collect(),
        code_required,
        csrf: c.csrf.clone(),
        request_id: c.request_id.clone(),
        identifier: identifier.to_owned(),
        remember,
    }
    .render()
    .unwrap_or_default();
    with_cookies(page(html, StatusCode::OK, None), &c.set)
}

async fn consent_page(s: &AppState, c: &Ctx, account: &Account, remember: bool) -> Response {
    let sets = crate::lexicon::permission_sets(s, c.params.scope.as_deref())
        .await
        .unwrap_or_default();
    let scopes = c
        .params
        .scopes()
        .into_iter()
        .map(|v| {
            let (label, detail) = describe(v, &sets);
            ScopeLine {
                value: v.to_owned(),
                label,
                detail,
                required: v == "atproto",
            }
        })
        .collect();
    let html = ConsentPage {
        service_name: service_name(s),
        client_name: client_name(&c.client),
        client_host: client_host(&c.client),
        csrf: c.csrf.clone(),
        request_id: c.request_id.clone(),
        did: account.did.clone(),
        handle: account.handle.clone(),
        scopes,
    }
    .render()
    .unwrap_or_default();
    let mut res = with_cookies(page(html, StatusCode::OK, None), &c.set);
    if !remember {
        // Carried to the accept step, so the device forgets the account after.
        res.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_static(
                "forget-account=1; Path=/oauth; HttpOnly; SameSite=Lax; Max-Age=900",
            ),
        );
    }
    res
}

/// Approves for `account` and redirects to the client.
async fn finish(
    s: &AppState,
    c: &Ctx,
    did: &str,
    approved: Option<&str>,
    forget: bool,
) -> Response {
    let result = crate::oauth::authorize(s, &c.request_id, &c.device, did, approved).await;
    if forget {
        let _ = s.store.oauth_remove_account_device(did, &c.device).await;
    }
    match result {
        Ok((code, p)) => respond_to_client(s, &p, vec![("code".into(), code)], &c.set),
        Err(e) => redirect_error(s, &c.params, &e, &c.set),
    }
}

/// Consent, or straight on when the account already approved these scopes.
async fn after_sign_in(s: &AppState, c: &Ctx, account: &Account, remember: bool) -> Response {
    match crate::oauth::consent_required(s, &account.did, &c.params).await {
        Ok(false) => finish(s, c, &account.did, None, !remember).await,
        Ok(true) => consent_page(s, c, account, remember).await,
        Err(e) => redirect_error(s, &c.params, &e, &c.set),
    }
}

/// `GET /oauth/authorize`.
pub async fn authorize_page(State(s): St, headers: HeaderMap, RawQuery(q): RawQuery) -> Response {
    let mut set = Vec::new();
    let query: Map<String, Value> = url::form_urlencoded::parse(q.unwrap_or_default().as_bytes())
        .map(|(k, v)| (k.into_owned(), Value::String(v.into_owned())))
        .collect();
    let Some(client_id) = query
        .get("client_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return error_page(&s, "Missing \"client_id\"", StatusCode::BAD_REQUEST);
    };
    let device = match load_device(&s, &headers, &mut set).await {
        Ok(d) => d,
        Err(e) => return error_page(&s, &e.description, StatusCode::INTERNAL_SERVER_ERROR),
    };
    let csrf = csrf_token(&s, &headers, &mut set);
    let client = match crate::oauth::get_client(&s, &client_id).await {
        Ok(c) => c,
        Err(e) => {
            return with_cookies(
                error_page(&s, &e.description, StatusCode::BAD_REQUEST),
                &set,
            );
        }
    };
    let opened = match query.get("request_uri").and_then(Value::as_str) {
        Some(uri) => match oreq::request_id_from_uri(uri) {
            Some(id) => crate::oauth::open_request(&s, id, &device, &client.id).await,
            None => Err(OAuthError::invalid_request("Invalid request_uri")),
        },
        None => crate::oauth::create_direct_request(&s, &client, &query, &device).await,
    };
    let (row, params) = match opened {
        Ok(v) => v,
        Err(e) => {
            return with_cookies(
                error_page(&s, &e.description, StatusCode::BAD_REQUEST),
                &set,
            );
        }
    };
    let c = Ctx {
        client,
        params,
        request_id: row.id,
        device,
        csrf,
        set,
    };
    let accounts = match device_accounts(&s, &c.device).await {
        Ok(a) => a,
        Err(e) => return redirect_error(&s, &c.params, &e, &c.set),
    };
    let p = &c.params;
    let hint = p.login_hint.as_deref();
    let matches_hint = |a: &Account| hint.is_some_and(|h| a.did == h || a.handle == h);
    let prompt = p.prompt.as_deref();
    if prompt == Some("select_account") && accounts.is_empty() {
        return redirect_error(
            &s,
            p,
            &OAuthError::new(
                "account_selection_required",
                "Account selection required",
                400,
            ),
            &c.set,
        );
    }
    let sso: Vec<&(Account, bool, i64)> = accounts
        .iter()
        .filter(|(a, _, _)| a.status == Status::Active && matches_hint(a))
        .collect();
    if prompt == Some("none") || (prompt.is_none() && hint.is_some()) {
        let none = prompt == Some("none");
        let only = (sso.len() == 1).then(|| sso[0]);
        match only {
            Some((a, login_required, _)) if !login_required && prompt != Some("login") => {
                match crate::oauth::consent_required(&s, &a.did, p).await {
                    Ok(false) => return finish(&s, &c, &a.did, None, false).await,
                    Ok(true) if none => {
                        return redirect_error(
                            &s,
                            p,
                            &OAuthError::new("consent_required", "Consent required", 400),
                            &c.set,
                        );
                    }
                    _ => {}
                }
            }
            Some(_) if none => {
                return redirect_error(
                    &s,
                    p,
                    &OAuthError::new("login_required", "Login required", 400),
                    &c.set,
                );
            }
            None if none => {
                let e = if sso.len() > 1 {
                    OAuthError::new(
                        "account_selection_required",
                        "Account selection required",
                        400,
                    )
                } else {
                    OAuthError::new("login_required", "Login required", 400)
                };
                return redirect_error(&s, p, &e, &c.set);
            }
            _ => {}
        }
    }
    let identifier = hint.unwrap_or("").to_owned();
    sign_in_page(&s, &c, &accounts, "", "", &identifier, false, true)
}

/// Loads the context a form post continues: its request (on this device,
/// open) and client.
async fn post_ctx(
    s: &AppState,
    headers: &HeaderMap,
    form: &[(String, String)],
) -> Result<Ctx, Response> {
    check_post(headers, form).map_err(|e| error_page(s, &e, StatusCode::FORBIDDEN))?;
    let mut set = Vec::new();
    let device = load_device(s, headers, &mut set)
        .await
        .map_err(|e| error_page(s, &e.description, StatusCode::INTERNAL_SERVER_ERROR))?;
    let csrf = csrf_token(s, headers, &mut set);
    let id = form_value(form, "request_id").unwrap_or("").to_owned();
    let row = s
        .store
        .oauth_read_request(&id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| {
            error_page(
                s,
                "This sign-in request has expired. Go back to the app and try again.",
                StatusCode::BAD_REQUEST,
            )
        })?;
    if row.device_id.as_deref() != Some(device.as_str())
        || row.did.is_some()
        || row.expires_at < locus_core::time::now_ms()
    {
        return Err(error_page(
            s,
            "This sign-in request is no longer valid. Go back to the app and try again.",
            StatusCode::BAD_REQUEST,
        ));
    }
    let params: Parameters = serde_json::from_str(&row.parameters).unwrap_or_default();
    let client = crate::oauth::get_client(s, &row.client_id)
        .await
        .map_err(|e| error_page(s, &e.description, StatusCode::BAD_REQUEST))?;
    Ok(Ctx {
        client,
        params,
        request_id: id,
        device,
        csrf,
        set,
    })
}

fn parse_form(body: &[u8]) -> Vec<(String, String)> {
    url::form_urlencoded::parse(body)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// `POST /oauth/authorize/sign-in`: password (and the email code when the
/// account needs one), padded to a constant time.
pub async fn sign_in(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let started = tokio::time::Instant::now();
    let res = sign_in_inner(&s, &headers, &parse_form(&body)).await;
    tokio::time::sleep_until(started + crate::server::LOGIN_FLOOR).await;
    res
}

async fn sign_in_inner(s: &AppState, headers: &HeaderMap, form: &[(String, String)]) -> Response {
    let c = match post_ctx(s, headers, form).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let identifier = form_value(form, "identifier")
        .unwrap_or("")
        .trim()
        .to_owned();
    let password = form_value(form, "password").unwrap_or("");
    let code = form_value(form, "code").filter(|v| !v.is_empty());
    let remember = form_value(form, "remember").is_some();
    let accounts = device_accounts(s, &c.device).await.unwrap_or_default();
    let again = |error: &str, notice: &str, code_required: bool| {
        sign_in_page(
            s,
            &c,
            &accounts,
            error,
            notice,
            &identifier,
            code_required,
            remember,
        )
    };
    {
        use crate::ratelimit::buckets::{CREATE_SESSION_5M, CREATE_SESSION_DAY};
        let ip =
            crate::ratelimit::client_ip().map_or_else(|| "unknown".to_owned(), |i| i.to_string());
        let key = format!("{identifier}-{ip}");
        if crate::ratelimit::limit(
            s,
            &[(CREATE_SESSION_DAY, &key, 1), (CREATE_SESSION_5M, &key, 1)],
        )
        .await
        .is_err()
        {
            return again(
                "Too many attempts; wait a few minutes and try again",
                "",
                false,
            );
        }
    }
    let invalid = "Invalid identifier or password";
    let account = match crate::server::find_account(s, &identifier).await {
        Ok(Some(a)) if a.status != Status::Deleted => a,
        _ => return again(invalid, "", false),
    };
    match crate::server::check_password(s, &account, password).await {
        Ok(Some(crate::server::Credential::Account)) => {}
        _ => return again(invalid, "", false),
    }
    if matches!(account.status, Status::Takendown | Status::Suspended) {
        return again("This account has been taken down", "", false);
    }
    // Email 2FA, unless the account was on this device in the last 30 days.
    if let (Some(email), Some(_)) = (&account.email, account.email_auth_factor_at) {
        let recent = accounts.iter().any(|(a, _, updated)| {
            a.did == account.did && locus_core::time::now_ms() - updated < TWO_FACTOR_SKIP_MS
        });
        if !recent {
            match code {
                Some(code) => {
                    if crate::email::check(
                        s,
                        &account.did,
                        locus_store::email::TokenPurpose::AuthFactor,
                        code,
                    )
                    .await
                    .is_err()
                    {
                        return again("Invalid or expired sign-in code", "", true);
                    }
                }
                None => {
                    let sent = async {
                        crate::email::require_email(s).await?;
                        let token = crate::email::issue(
                            s,
                            &account.did,
                            locus_store::email::TokenPurpose::AuthFactor,
                        )
                        .await?;
                        crate::email::send(
                            s,
                            locus_mail::Template::SignInCode,
                            email,
                            &token,
                            &account.handle,
                        )
                        .await
                    }
                    .await;
                    return match sent {
                        Ok(()) => again(
                            "",
                            "A sign-in code has been sent to your email address",
                            true,
                        ),
                        Err(_) => again(
                            "A sign-in code is required, but email is not available on this server; contact its operator",
                            "",
                            false,
                        ),
                    };
                }
            }
        }
    }
    if s.store
        .oauth_touch_account_device(&account.did, &c.device)
        .await
        .is_err()
    {
        return error_page(
            s,
            "Internal Server Error",
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }
    after_sign_in(s, &c, &account, remember).await
}

/// `POST /oauth/authorize/select`: continue as an account signed in on
/// this device.
pub async fn select(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let form = parse_form(&body);
    let c = match post_ctx(&s, &headers, &form).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let did = form_value(&form, "did").unwrap_or("");
    let accounts = device_accounts(&s, &c.device).await.unwrap_or_default();
    match accounts.iter().find(|(a, _, _)| a.did == did) {
        Some((a, false, _)) if a.status == Status::Active => {
            let a = a.clone();
            after_sign_in(&s, &c, &a, true).await
        }
        Some((a, _, _)) => {
            let handle = a.handle.clone();
            sign_in_page(
                &s,
                &c,
                &accounts,
                "Enter your password to continue",
                "",
                &handle,
                false,
                true,
            )
        }
        None => sign_in_page(
            &s,
            &c,
            &accounts,
            "That account isn't signed in on this device",
            "",
            "",
            false,
            true,
        ),
    }
}

/// `POST /oauth/authorize/accept`: the scopes the user kept are approved.
pub async fn accept(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let form = parse_form(&body);
    let c = match post_ctx(&s, &headers, &form).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let did = form_value(&form, "did").unwrap_or("").to_owned();
    let accounts = device_accounts(&s, &c.device).await.unwrap_or_default();
    if !accounts.iter().any(|(a, login, _)| a.did == did && !login) {
        return sign_in_page(
            &s,
            &c,
            &accounts,
            "Sign in again to continue",
            "",
            "",
            false,
            true,
        );
    }
    let requested: Vec<&str> = c.params.scopes();
    let kept: Vec<&str> = form
        .iter()
        .filter(|(k, v)| k == "scope" && requested.contains(&v.as_str()))
        .map(|(_, v)| v.as_str())
        .collect();
    let approved = kept.join(" ");
    let forget = cookie(&cookies(&headers), "forget-account") == Some("1");
    let mut res = finish(&s, &c, &did, Some(&approved), forget).await;
    if forget {
        res.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_static(
                "forget-account=; Path=/oauth; HttpOnly; SameSite=Lax; Max-Age=0",
            ),
        );
    }
    res
}

/// `POST /oauth/authorize/reject`.
pub async fn reject(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let form = parse_form(&body);
    let c = match post_ctx(&s, &headers, &form).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let _ = s.store.oauth_delete_request(&c.request_id).await;
    redirect_error(
        &s,
        &c.params,
        &OAuthError::access_denied("Access denied"),
        &c.set,
    )
}

pub async fn style() -> Response {
    let mut res = include_str!("../assets/oauth.css").into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/css; charset=utf-8"),
    );
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("max-age=3600"),
    );
    res
}

pub async fn form_post_js() -> Response {
    let mut res = include_str!("../assets/form-post.js").into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/javascript; charset=utf-8"),
    );
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("max-age=3600"),
    );
    res
}
