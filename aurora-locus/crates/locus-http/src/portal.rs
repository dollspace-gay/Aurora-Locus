//! The account portal at `/account`, while `portal.enabled` is on: where
//! people manage their own account in a browser. A portal sign-in is an
//! ordinary account session (made by `createSession`, so email sign-in
//! codes and takedowns apply as they do to apps); each page calls the same
//! XRPC methods apps use, as that session, with an access token minted for
//! the call.

#![allow(clippy::result_large_err)]

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use locus_auth::session::SCOPE_ACCESS;
use locus_store::accounts::{Account, SessionFilter, Status};
use locus_web::console_pages::{Card, Cell, Fact, Form, Input, SectionPage, Table};
use locus_web::portal::{self as menu, PageId, SignInPage};
use locus_web::setup::Choice;
use locus_web::theme::{Font, Mode, Scheme, Theme};
use locus_web::view::{Link, MessagePage, Shell};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::state::AppState;
use crate::web;

type St = State<Arc<AppState>>;

const COOKIE: &str = "locus-account";
const PRE_COOKIE: &str = "locus-account-pre";
/// A portal sign-in ends after a week, or two days unused.
const TTL_MS: i64 = 7 * 86_400_000;
const IDLE_MS: i64 = 2 * 86_400_000;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/account", get(overview))
        .route("/account/sign-in", get(sign_in_page).post(sign_in))
        .route("/account/sign-up", get(sign_up_page).post(sign_up))
        .route("/account/sign-out", post(sign_out))
        .route("/account/handle", get(handle_page).post(handle_save))
        .route("/account/email", get(email_page))
        .route("/account/email/{action}", post(email_action))
        .route("/account/password", get(password_page))
        .route("/account/password/{action}", post(password_action))
        .route("/account/app-passwords", get(app_passwords_page))
        .route("/account/app-passwords/{action}", post(app_password_action))
        .route("/account/apps", get(apps_page))
        .route("/account/apps/revoke", post(app_revoke))
        .route("/account/export", get(export_page))
        .route("/account/export/repo.car", get(export_repo))
        .route("/account/identity", get(identity_page))
        .route("/account/identity/{action}", post(identity_action))
        .route("/account/moving", get(moving_page))
        .route("/account/deactivate", get(deactivate_page).post(deactivate))
        .route("/account/delete", get(delete_page))
        .route("/account/delete/{action}", post(delete_action))
        .route(
            "/account/appearance",
            get(appearance_page).post(appearance_save),
        )
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "Not found").into_response()
}

/// The portal answers only while it's on.
fn enabled(s: &AppState) -> Result<(), Response> {
    if s.snapshot().bool("portal.enabled") {
        Ok(())
    } else {
        Err(not_found())
    }
}

fn id_hash(id: &str) -> String {
    hex::encode(Sha256::digest(
        format!("locus-portal-session:{id}").as_bytes(),
    ))
}

fn csrf_for(id: &str) -> String {
    hex::encode(&Sha256::digest(format!("locus-portal-csrf:{id}").as_bytes())[..16])
}

fn cookie(s: &AppState, name: &str, value: &str, max_age_s: i64) -> String {
    let secure = if s.public_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    format!("{name}={value}; Path=/account; Max-Age={max_age_s}; HttpOnly; SameSite=Lax{secure}")
}

fn brand(s: &AppState) -> String {
    crate::branding::service_name_cached(s)
}

/// A signed-in account holder.
pub struct User {
    pub account: Account,
    pub session_id: String,
    pub id: String,
    pub csrf: String,
}

impl User {
    /// Headers that call XRPC methods as this sign-in.
    fn bearer(&self, s: &AppState) -> HeaderMap {
        let token = s
            .sessions
            .mint_access(&self.account.did, SCOPE_ACCESS, &self.session_id);
        let mut h = HeaderMap::new();
        if let Ok(v) = format!("Bearer {token}").parse() {
            h.insert(header::AUTHORIZATION, v);
        }
        h
    }
}

async fn signed_in(s: &AppState, headers: &HeaderMap) -> Option<User> {
    let all = web::cookies(headers);
    let id = web::cookie(&all, COOKIE)?.to_owned();
    if id.len() != 64 {
        return None;
    }
    let (did, session_id) = s
        .store
        .portal_session(&id_hash(&id), IDLE_MS)
        .await
        .ok()??;
    // The account session must still stand (a password change revokes it).
    let token = s.sessions.mint_access(&did, SCOPE_ACCESS, &session_id);
    s.sessions.verify_access(&token).await.ok()?;
    let account = s.store.account_by_did(&did).await.ok()??;
    if account.status == Status::Deleted {
        return None;
    }
    Some(User {
        account,
        session_id,
        csrf: csrf_for(&id),
        id,
    })
}

async fn require_user(s: &AppState, headers: &HeaderMap, here: &str) -> Result<User, Response> {
    let _ = crate::branding::published(s).await;
    enabled(s)?;
    signed_in(s, headers).await.ok_or_else(|| {
        web::redirect(&format!(
            "/account/sign-in?next={}",
            url::form_urlencoded::byte_serialize(here.as_bytes()).collect::<String>()
        ))
    })
}

async fn form_user(
    s: &AppState,
    headers: &HeaderMap,
    body: &Bytes,
    here: &str,
) -> Result<(User, Vec<(String, String)>), Response> {
    let u = require_user(s, headers, here).await?;
    let form = web::parse_form(body);
    if !web::same_site(headers) || !web::ct_eq(web::field(&form, "csrf"), &u.csrf) {
        return Err((
            StatusCode::FORBIDDEN,
            "Request refused: the form expired. Go back and try again.",
        )
            .into_response());
    }
    Ok((u, form))
}

fn raw<'a>(form: &'a [(String, String)], k: &str) -> &'a str {
    form.iter()
        .find(|(n, _)| n == k)
        .map_or("", |(_, v)| v.as_str())
}

fn input<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).expect("the portal builds valid inputs")
}

fn said<T>(r: crate::error::XrpcResult<T>) -> Result<T, String> {
    r.map_err(|e| e.message)
}

async fn theme_of(s: &AppState, did: &str) -> Theme {
    match s.store.account_theme(did).await {
        Ok(Some(t)) => Theme::from_stored(&t.theme_mode, &t.theme_scheme, &t.theme_font),
        _ => Theme::default(),
    }
}

async fn page(s: &AppState, u: &User, id: PageId, intro: &str) -> SectionPage {
    let mut banners = Vec::new();
    if u.account.email_confirmed_at.is_none() && u.account.email.is_some() {
        banners.push(locus_web::view::Banner {
            bad: false,
            text: "Your email address isn't confirmed yet.".into(),
            link: Some(Link {
                href: PageId::Email.href().into(),
                label: "Confirm it".into(),
            }),
        });
    }
    if u.account.status == Status::Deactivated {
        banners.push(locus_web::view::Banner {
            bad: true,
            text: "Your account is deactivated: apps can't see it.".into(),
            link: Some(Link {
                href: PageId::Deactivate.href().into(),
                label: "Reactivate it".into(),
            }),
        });
    }
    let nav = menu::nav(
        &format!("@{}", u.account.handle),
        &u.csrf,
        theme_of(s, &u.account.did).await,
        id,
        banners,
    );
    SectionPage {
        shell: Shell {
            dress: crate::branding::dress_cached(s, crate::branding::Blurb::None),
            ..Shell::signed_in(id.name(), brand(s), nav)
        },
        csrf: u.csrf.clone(),
        intro: intro.into(),
        notice: None,
        errors: Vec::new(),
        cards: Vec::new(),
        forms: Vec::new(),
        tables: Vec::new(),
        fields: Vec::new(),
        advanced: Vec::new(),
        pre: None,
        frames: Vec::new(),
        can_edit: false,
        back: id.href().into(),
    }
}

fn form(title: &str, text: &str, action: &str, inputs: Vec<Input>, button: &str) -> Form {
    Form {
        title: title.into(),
        text: text.into(),
        action: action.into(),
        get: false,
        inputs,
        hidden: Vec::new(),
        button: button.into(),
        danger: false,
    }
}

fn when(ms: i64) -> String {
    crate::console::when(ms)
}

#[derive(Deserialize, Default)]
pub struct Q {
    #[serde(default)]
    done: Option<String>,
    #[serde(default)]
    next: Option<String>,
}

// ---------------------------------------------------------------- sign-in

fn sign_in_render(
    s: &AppState,
    csrf: &str,
    error: Option<String>,
    notice: Option<String>,
    identifier: &str,
    code_stage: bool,
    status: StatusCode,
) -> Response {
    web::render(
        &SignInPage {
            shell: Shell {
                dress: crate::branding::dress_cached(s, crate::branding::Blurb::SignIn),
                ..Shell::signed_out("Sign in", brand(s))
            },
            heading: crate::branding::sign_in_heading(s),
            csrf: csrf.into(),
            error,
            notice,
            identifier: identifier.into(),
            code_stage,
            sign_up: false,
            domains: Vec::new(),
            invite_required: false,
            handle: String::new(),
            email: String::new(),
        },
        status,
    )
}

fn pre_cookie(headers: &HeaderMap) -> String {
    let all = web::cookies(headers);
    web::cookie(&all, PRE_COOKIE).unwrap_or("").to_owned()
}

fn new_pre(s: &AppState, res: Response, pre: &str) -> Response {
    web::with_cookie(res, &cookie(s, PRE_COOKIE, pre, 3600))
}

/// `GET /account/sign-in`.
pub async fn sign_in_page(State(s): St, headers: HeaderMap, Query(q): Query<Q>) -> Response {
    let _ = crate::branding::published(&s).await;
    if let Err(r) = enabled(&s) {
        return r;
    }
    if signed_in(&s, &headers).await.is_some() {
        return web::redirect("/account");
    }
    let pre = hex::encode(rand::random::<[u8; 16]>());
    let notice = q.done.map(|d| match d.as_str() {
        "password" => "Your password changed. Sign in with the new one.".to_owned(),
        "deleted" => "Your account is being deleted.".to_owned(),
        _ => "Signed out.".to_owned(),
    });
    let mut res = sign_in_render(&s, &pre, None, notice, "", false, StatusCode::OK);
    if let Some(next) = q.next.filter(|n| menu::safe_next(n)) {
        res = web::with_cookie(
            res,
            &cookie(
                &s,
                "locus-account-next",
                &url::form_urlencoded::byte_serialize(next.as_bytes()).collect::<String>(),
                3600,
            ),
        );
    }
    new_pre(&s, res, &pre)
}

/// Starts a portal sign-in on an account session.
async fn start(s: &AppState, did: &str, session_id: &str, headers: &HeaderMap) -> Response {
    let id = hex::encode(rand::random::<[u8; 32]>());
    if let Err(e) = s
        .store
        .create_portal_session(&id_hash(&id), did, session_id, TTL_MS)
        .await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    let all = web::cookies(headers);
    let next = web::cookie(&all, "locus-account-next")
        .and_then(|n| {
            url::form_urlencoded::parse(format!("n={n}").as_bytes())
                .next()
                .map(|(_, v)| v.into_owned())
        })
        .filter(|n| menu::safe_next(n))
        .unwrap_or_else(|| "/account".into());
    let res = web::with_cookie(web::redirect(&next), &cookie(s, COOKIE, &id, TTL_MS / 1000));
    web::with_cookie(res, &cookie(s, "locus-account-next", "", 0))
}

/// The session id an access token was issued under.
fn session_of(s: &AppState, access: &str) -> Option<(String, String, String)> {
    let c = locus_auth::jwt::verify(
        &s.secrets.jwt,
        locus_auth::jwt::TYP_ACCESS,
        access,
        locus_core::time::now_ms() / 1000,
    )
    .ok()?;
    Some((c.sub, c.jti?, c.scope))
}

/// `POST /account/sign-in`: through `createSession`, so the email code
/// (when the account has sign-in codes on) and takedowns apply.
pub async fn sign_in(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let _ = crate::branding::published(&s).await;
    if let Err(r) = enabled(&s) {
        return r;
    }
    let form = web::parse_form(&body);
    let pre = pre_cookie(&headers);
    let identifier = web::field(&form, "identifier").to_owned();
    if !web::same_site(&headers) || !web::ct_eq(web::field(&form, "csrf"), &pre) {
        return sign_in_render(
            &s,
            &pre,
            Some("The form expired; try again.".into()),
            None,
            &identifier,
            false,
            StatusCode::FORBIDDEN,
        );
    }
    let code = web::field(&form, "code");
    let mut body = json!({"identifier": identifier, "password": raw(&form, "password")});
    if !code.is_empty() {
        body["authFactorToken"] = json!(code);
    }
    match crate::server::create_session(State(Arc::clone(&s)), Json(input(body))).await {
        Ok(Json(v)) => {
            let access = v["accessJwt"].as_str().unwrap_or_default();
            let Some((did, session_id, scope)) = session_of(&s, access) else {
                return sign_in_render(
                    &s,
                    &pre,
                    Some("Signing in failed.".into()),
                    None,
                    &identifier,
                    false,
                    StatusCode::INTERNAL_SERVER_ERROR,
                );
            };
            if scope != SCOPE_ACCESS {
                // An app password: apps only, not the portal.
                let _ = revoke(&s, &did, &session_id).await;
                return sign_in_render(
                    &s,
                    &pre,
                    Some("Use your account password here, not an app password.".into()),
                    None,
                    &identifier,
                    false,
                    StatusCode::UNAUTHORIZED,
                );
            }
            start(&s, &did, &session_id, &headers).await
        }
        Err(e) if e.error == "AuthFactorTokenRequired" => sign_in_render(
            &s,
            &pre,
            None,
            Some("We emailed you a sign-in code. Enter it with your password.".into()),
            &identifier,
            true,
            StatusCode::OK,
        ),
        Err(e) => sign_in_render(
            &s,
            &pre,
            Some(e.message),
            None,
            &identifier,
            !code.is_empty(),
            e.status,
        ),
    }
}

async fn revoke(s: &AppState, did: &str, session_id: &str) -> locus_core::CoreResult<()> {
    let mut tx = s.store.begin().await?;
    tx.revoke_sessions(did, SessionFilter::One(session_id))
        .await?;
    tx.commit().await
}

/// `POST /account/sign-out`: ends the portal sign-in and its session.
pub async fn sign_out(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    if let Ok((u, _)) = form_user(&s, &headers, &body, "/account").await {
        let _ = s.store.delete_portal_session(&id_hash(&u.id)).await;
        let _ = revoke(&s, &u.account.did, &u.session_id).await;
    }
    web::with_cookie(
        web::redirect("/account/sign-in?done=out"),
        &cookie(&s, COOKIE, "", 0),
    )
}

fn sign_up_render(
    s: &AppState,
    csrf: &str,
    error: Option<String>,
    handle: &str,
    email: &str,
    status: StatusCode,
) -> Response {
    web::render(
        &SignInPage {
            shell: Shell {
                dress: crate::branding::dress_cached(s, crate::branding::Blurb::SignUp),
                ..Shell::signed_out("Create an account", brand(s))
            },
            heading: String::new(),
            csrf: csrf.into(),
            error,
            notice: None,
            identifier: String::new(),
            code_stage: false,
            sign_up: true,
            domains: s.handle_domains(),
            invite_required: s.snapshot().bool("accounts.invite_required"),
            handle: handle.into(),
            email: email.into(),
        },
        status,
    )
}

/// `GET /account/sign-up`.
pub async fn sign_up_page(State(s): St) -> Response {
    let _ = crate::branding::published(&s).await;
    if let Err(r) = enabled(&s) {
        return r;
    }
    let pre = hex::encode(rand::random::<[u8; 16]>());
    new_pre(
        &s,
        sign_up_render(&s, &pre, None, "", "", StatusCode::OK),
        &pre,
    )
}

/// A handle typed without its domain gets the first handle domain.
#[must_use]
pub fn full_handle(s: &AppState, typed: &str) -> String {
    let h = typed.trim().trim_start_matches('@').to_ascii_lowercase();
    if h.contains('.') {
        h
    } else {
        format!(
            "{h}{}",
            s.handle_domains().first().map_or("", String::as_str)
        )
    }
}

/// `POST /account/sign-up`: `createAccount`, then signed in.
pub async fn sign_up(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let _ = crate::branding::published(&s).await;
    if let Err(r) = enabled(&s) {
        return r;
    }
    let form = web::parse_form(&body);
    let pre = pre_cookie(&headers);
    let (handle, email) = (
        full_handle(&s, web::field(&form, "handle")),
        web::field(&form, "email").to_owned(),
    );
    if !web::same_site(&headers) || !web::ct_eq(web::field(&form, "csrf"), &pre) {
        return sign_up_render(
            &s,
            &pre,
            Some("The form expired; try again.".into()),
            &handle,
            &email,
            StatusCode::FORBIDDEN,
        );
    }
    let mut body = json!({"handle": handle, "email": email, "password": raw(&form, "password")});
    let invite = web::field(&form, "invite");
    if !invite.is_empty() {
        body["inviteCode"] = json!(invite);
    }
    match crate::server::create_account(State(Arc::clone(&s)), HeaderMap::new(), Json(input(body)))
        .await
    {
        Ok(Json(v)) => match session_of(&s, v["accessJwt"].as_str().unwrap_or_default()) {
            Some((did, session_id, _)) => start(&s, &did, &session_id, &headers).await,
            None => web::redirect("/account/sign-in"),
        },
        Err(e) => sign_up_render(&s, &pre, Some(e.message), &handle, &email, e.status),
    }
}

// ---------------------------------------------------------------- overview

/// `GET /account`.
pub async fn overview(State(s): St, headers: HeaderMap) -> Response {
    let u = match require_user(&s, &headers, "/account").await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let a = &u.account;
    let mut p = page(&s, &u, PageId::Overview, "").await;
    p.shell.title = format!("@{}", a.handle);
    p.shell.dress = crate::branding::dress_cached(&s, crate::branding::Blurb::Portal);
    p.cards.push(Card {
        title: "Your account".into(),
        facts: vec![
            Fact::new("Handle", format!("@{}", a.handle)),
            Fact::new("DID", a.did.clone()),
            Fact::new("Email", a.email.clone().unwrap_or_else(|| "none".into())),
            Fact::new(
                "Email confirmed",
                if a.email_confirmed_at.is_some() {
                    "yes"
                } else {
                    "no"
                },
            )
            .class(if a.email_confirmed_at.is_some() {
                "ok"
            } else {
                "warn"
            }),
            Fact::new(
                "Sign-in codes by email",
                if a.email_auth_factor_at.is_some() {
                    "on"
                } else {
                    "off"
                },
            ),
            Fact::new("Status", a.status.as_str()).class(if a.status == Status::Active {
                "ok"
            } else {
                "warn"
            }),
            Fact::new("Created", when(a.created_at)),
        ],
    });
    let (records, blobs, bytes) = s.store.account_stats(&a.did).await.unwrap_or_default();
    p.cards.push(Card {
        title: "Your data here".into(),
        facts: vec![
            Fact::new("Records", records.to_string()),
            Fact::new(
                "Media files",
                format!("{blobs} ({})", crate::console::bytes(bytes)),
            ),
        ],
    });
    web::render(&p, StatusCode::OK)
}

// ---------------------------------------------------------------- handle

async fn handle_render(
    s: &AppState,
    u: &User,
    notice: Option<String>,
    errors: Vec<String>,
    status: StatusCode,
) -> Response {
    let mut p = page(s, u, PageId::Handle, "Your handle is your name on the network. Change it to another one here, or to a domain you own.").await;
    p.notice = notice;
    p.errors = errors;
    p.forms.push(form(
        "Change your handle",
        &format!("Here, handles end in {}.", s.handle_domains().join(" or ")),
        "/account/handle",
        vec![Input::new("handle", "New handle", "text").value(u.account.handle.clone())],
        "Change handle",
    ));
    p.cards.push(Card {
        title: "Using a domain you own".into(),
        facts: vec![
            Fact::new(
                "Add a DNS TXT record",
                format!("_atproto.<your domain> = did={}", u.account.did),
            ),
            Fact::new(
                "Or serve",
                format!(
                    "https://<your domain>/.well-known/atproto-did with {}",
                    u.account.did
                ),
            ),
            Fact::new(
                "Then",
                "enter the domain above; it's checked before it's used",
            ),
        ],
    });
    web::render(&p, status)
}

/// `GET /account/handle`.
pub async fn handle_page(State(s): St, headers: HeaderMap, Query(q): Query<Q>) -> Response {
    match require_user(&s, &headers, "/account/handle").await {
        Ok(u) => {
            handle_render(
                &s,
                &u,
                q.done.map(|_| "Your handle changed.".into()),
                Vec::new(),
                StatusCode::OK,
            )
            .await
        }
        Err(r) => r,
    }
}

/// `POST /account/handle`.
pub async fn handle_save(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (u, f) = match form_user(&s, &headers, &body, "/account/handle").await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let handle = full_handle(&s, web::field(&f, "handle"));
    match said(
        crate::identity::update_handle(
            State(Arc::clone(&s)),
            u.bearer(&s),
            Json(input(json!({"handle": handle}))),
        )
        .await,
    ) {
        Ok(()) => web::redirect("/account/handle?done=1"),
        Err(e) => handle_render(&s, &u, None, vec![e], StatusCode::BAD_REQUEST).await,
    }
}

// ---------------------------------------------------------------- email

async fn email_render(
    s: &AppState,
    u: &User,
    notice: Option<String>,
    errors: Vec<String>,
    status: StatusCode,
) -> Response {
    let a = &u.account;
    let mut p = page(s, u, PageId::Email, "Your email is how this server reaches you: to confirm it's you, and to reset a forgotten password.").await;
    p.notice = notice;
    p.errors = errors;
    let current = a.email.clone().unwrap_or_default();
    if a.email_confirmed_at.is_none() {
        p.forms.push(form(
            "Confirm your email",
            &format!("We'll send a code to {current}."),
            "/account/email/send-confirm",
            Vec::new(),
            "Send a code",
        ));
        p.forms.push(form(
            "Enter the code",
            "",
            "/account/email/confirm",
            vec![Input::new("code", "Code", "text")],
            "Confirm",
        ));
    }
    p.forms.push(form(
        "Get a code to make changes",
        "Changing your email or sign-in codes needs a code sent to your current address (once it's confirmed).",
        "/account/email/send-update",
        Vec::new(),
        "Send a code",
    ));
    p.forms.push(form(
        "Change your email",
        "",
        "/account/email/update",
        vec![
            Input::new("email", "New email", "email").value(current.clone()),
            Input::new("code", "Code (if one was needed)", "text"),
        ],
        "Change email",
    ));
    let on = a.email_auth_factor_at.is_some();
    let toggle = Input {
        options: vec![
            Choice {
                value: "on".into(),
                label: "On: a code by email at every sign-in".into(),
                checked: on,
            },
            Choice {
                value: "off".into(),
                label: "Off".into(),
                checked: !on,
            },
        ],
        ..Input::new("factor", "Sign-in codes", "select")
    };
    p.forms.push(form(
        "Sign-in codes by email",
        "With this on, signing in with your password also needs a code we email you.",
        "/account/email/factor",
        vec![
            toggle,
            Input::new("code", "Code (if one was needed)", "text"),
        ],
        "Save",
    ));
    web::render(&p, status)
}

/// `GET /account/email`.
pub async fn email_page(State(s): St, headers: HeaderMap, Query(q): Query<Q>) -> Response {
    match require_user(&s, &headers, "/account/email").await {
        Ok(u) => {
            let notice = q.done.map(|d| match d.as_str() {
                "confirmed" => "Your email is confirmed.".into(),
                "sent" => "Sent. Check your email for the code.".into(),
                _ => "Saved.".into(),
            });
            email_render(&s, &u, notice, Vec::new(), StatusCode::OK).await
        }
        Err(r) => r,
    }
}

/// `POST /account/email/{action}`.
pub async fn email_action(
    State(s): St,
    headers: HeaderMap,
    axum::extract::Path(action): axum::extract::Path<String>,
    body: Bytes,
) -> Response {
    let (u, f) = match form_user(&s, &headers, &body, "/account/email").await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let st = || State(Arc::clone(&s));
    let code = web::field(&f, "code");
    let current = u.account.email.clone().unwrap_or_default();
    let with_code = |mut v: Value| {
        if !code.is_empty() {
            v["token"] = json!(code);
        }
        v
    };
    let result = match action.as_str() {
        "send-confirm" => said(
            crate::email::request_email_confirmation(st(), u.bearer(&s), Default::default()).await,
        )
        .map(|()| "sent"),
        "confirm" => said(
            crate::email::confirm_email(
                st(),
                u.bearer(&s),
                Json(input(json!({"email": current, "token": code}))),
            )
            .await,
        )
        .map(|()| "confirmed"),
        "send-update" => match said(
            crate::email::request_email_update(st(), u.bearer(&s), Default::default()).await,
        ) {
            Ok(Json(v)) if v["tokenRequired"] == json!(false) => Ok("not-needed"),
            Ok(_) => Ok("sent"),
            Err(e) => Err(e),
        },
        "update" => said(
            crate::email::update_email(
                st(),
                u.bearer(&s),
                Json(input(with_code(json!({"email": web::field(&f, "email")})))),
            )
            .await,
        )
        .map(|()| "saved"),
        "factor" => said(
            crate::email::update_email(
                st(),
                u.bearer(&s),
                Json(input(with_code(
                    json!({"email": current, "emailAuthFactor": web::field(&f, "factor") == "on"}),
                ))),
            )
            .await,
        )
        .map(|()| "saved"),
        _ => return not_found(),
    };
    match result {
        Ok("not-needed") => email_render(
            &s,
            &u,
            Some(
                "No code is needed: your email isn't confirmed yet, so you can change it directly."
                    .into(),
            ),
            Vec::new(),
            StatusCode::OK,
        )
        .await,
        Ok(done) => web::redirect(&format!("/account/email?done={done}")),
        Err(e) => email_render(&s, &u, None, vec![e], StatusCode::BAD_REQUEST).await,
    }
}

// ---------------------------------------------------------------- password

async fn password_render(
    s: &AppState,
    u: &User,
    notice: Option<String>,
    errors: Vec<String>,
    status: StatusCode,
) -> Response {
    let mut p = page(s, u, PageId::Password, "To change your password, we email you a code first. Changing it signs you out everywhere, apps included.").await;
    p.notice = notice;
    p.errors = errors;
    p.forms.push(form(
        "Get a code",
        &format!(
            "We'll send it to {}.",
            u.account.email.clone().unwrap_or_default()
        ),
        "/account/password/send",
        Vec::new(),
        "Send a code",
    ));
    p.forms.push(form(
        "Choose a new password",
        "",
        "/account/password/reset",
        vec![
            Input::new("code", "Code", "text"),
            Input::new("password", "New password", "password"),
        ],
        "Change password",
    ));
    web::render(&p, status)
}

/// `GET /account/password`.
pub async fn password_page(State(s): St, headers: HeaderMap, Query(q): Query<Q>) -> Response {
    match require_user(&s, &headers, "/account/password").await {
        Ok(u) => {
            password_render(
                &s,
                &u,
                q.done
                    .map(|_| "Sent. Check your email for the code.".into()),
                Vec::new(),
                StatusCode::OK,
            )
            .await
        }
        Err(r) => r,
    }
}

/// `POST /account/password/{send|reset}`.
pub async fn password_action(
    State(s): St,
    headers: HeaderMap,
    axum::extract::Path(action): axum::extract::Path<String>,
    body: Bytes,
) -> Response {
    let (u, f) = match form_user(&s, &headers, &body, "/account/password").await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let st = || State(Arc::clone(&s));
    match action.as_str() {
        "send" => match said(
            crate::email::request_password_reset(
                st(),
                Json(input(
                    json!({"email": u.account.email.clone().unwrap_or_default()}),
                )),
            )
            .await,
        ) {
            Ok(()) => web::redirect("/account/password?done=sent"),
            Err(e) => password_render(&s, &u, None, vec![e], StatusCode::BAD_REQUEST).await,
        },
        "reset" => match said(
            crate::email::reset_password(
                st(),
                Json(input(
                    json!({"token": web::field(&f, "code"), "password": raw(&f, "password")}),
                )),
            )
            .await,
        ) {
            Ok(()) => {
                let _ = s.store.delete_portal_session(&id_hash(&u.id)).await;
                web::with_cookie(
                    web::redirect("/account/sign-in?done=password"),
                    &cookie(&s, COOKIE, "", 0),
                )
            }
            Err(e) => password_render(&s, &u, None, vec![e], StatusCode::BAD_REQUEST).await,
        },
        _ => not_found(),
    }
}

// ---------------------------------------------------------------- app passwords

async fn app_passwords_render(
    s: &Arc<AppState>,
    u: &User,
    new: Option<(String, String)>,
    errors: Vec<String>,
    status: StatusCode,
) -> Response {
    let mut p = page(s, u, PageId::AppPasswords, "App passwords let an app sign in as you without your real password. Revoke one and that app is signed out.").await;
    p.errors = errors;
    if let Some((name, pw)) = new {
        p.cards.push(Card {
            title: format!("New app password: {name}"),
            facts: vec![Fact::new("Copy it now; it isn't shown again", pw)],
        });
    }
    let list = said(crate::server::list_app_passwords(State(Arc::clone(s)), u.bearer(s)).await);
    let rows = match list {
        Ok(Json(v)) => v["passwords"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|p| {
                let name = p["name"].as_str().unwrap_or_default().to_owned();
                vec![
                    Cell::new("name", name.clone()),
                    Cell::new(
                        "when",
                        p["createdAt"]
                            .as_str()
                            .unwrap_or_default()
                            .replace('T', " ")
                            .chars()
                            .take(16)
                            .collect::<String>(),
                    ),
                    Cell::new(
                        "state",
                        if p["privileged"].as_bool().unwrap_or(false) {
                            "privileged"
                        } else {
                            "normal"
                        },
                    )
                    .pill(if p["privileged"].as_bool().unwrap_or(false) {
                        "amber"
                    } else {
                        ""
                    }),
                    Cell {
                        class: "actions",
                        button: Some(locus_web::console_pages::Button {
                            action: "/account/app-passwords/revoke".into(),
                            label: "Revoke".into(),
                            hidden: vec![("name".into(), name)],
                            danger: false,
                        }),
                        ..Cell::default()
                    },
                ]
            })
            .collect(),
        Err(e) => {
            p.errors.push(e);
            Vec::new()
        }
    };
    p.tables.push(Table {
        title: "Your app passwords".into(),
        cols: vec![
            ("name", "Name".into()),
            ("when", "Created".into()),
            ("state", "Kind".into()),
            ("actions", String::new()),
        ],
        rows,
        empty: "None yet.".into(),
    });
    let kind = Input {
        options: vec![
            Choice {
                value: "no".into(),
                label: "Normal".into(),
                checked: true,
            },
            Choice {
                value: "yes".into(),
                label: "Privileged: can also read your direct messages".into(),
                checked: false,
            },
        ],
        ..Input::new("privileged", "Kind", "select")
    };
    p.forms.push(form(
        "Make an app password",
        "",
        "/account/app-passwords/create",
        vec![
            Input::new("name", "Name (which app it's for)", "text"),
            kind,
        ],
        "Make it",
    ));
    web::render(&p, status)
}

/// `GET /account/app-passwords`.
pub async fn app_passwords_page(State(s): St, headers: HeaderMap) -> Response {
    match require_user(&s, &headers, "/account/app-passwords").await {
        Ok(u) => app_passwords_render(&s, &u, None, Vec::new(), StatusCode::OK).await,
        Err(r) => r,
    }
}

/// `POST /account/app-passwords/{create|revoke}`.
pub async fn app_password_action(
    State(s): St,
    headers: HeaderMap,
    axum::extract::Path(action): axum::extract::Path<String>,
    body: Bytes,
) -> Response {
    let (u, f) = match form_user(&s, &headers, &body, "/account/app-passwords").await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let st = || State(Arc::clone(&s));
    let name = web::field(&f, "name").to_owned();
    match action.as_str() {
        "create" => match said(
            crate::server::create_app_password(
                st(),
                u.bearer(&s),
                Json(input(
                    json!({"name": name, "privileged": web::field(&f, "privileged") == "yes"}),
                )),
            )
            .await,
        ) {
            Ok(Json(v)) => {
                app_passwords_render(
                    &s,
                    &u,
                    Some((name, v["password"].as_str().unwrap_or_default().to_owned())),
                    Vec::new(),
                    StatusCode::OK,
                )
                .await
            }
            Err(e) => app_passwords_render(&s, &u, None, vec![e], StatusCode::BAD_REQUEST).await,
        },
        "revoke" => match said(
            crate::server::revoke_app_password(
                st(),
                u.bearer(&s),
                Json(input(json!({"name": name}))),
            )
            .await,
        ) {
            Ok(_) => web::redirect("/account/app-passwords"),
            Err(e) => app_passwords_render(&s, &u, None, vec![e], StatusCode::BAD_REQUEST).await,
        },
        _ => not_found(),
    }
}

// ---------------------------------------------------------------- connected apps

/// `GET /account/apps`: apps signed in with OAuth, and where.
pub async fn apps_page(State(s): St, headers: HeaderMap) -> Response {
    let u = match require_user(&s, &headers, "/account/apps").await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let mut p = page(
        &s,
        &u,
        PageId::Apps,
        "Apps you've signed in to with this account. Revoke one to sign it out.",
    )
    .await;
    let tokens = s
        .store
        .oauth_account_tokens(&u.account.did)
        .await
        .unwrap_or_default();
    p.tables.push(Table {
        title: "Connected apps".into(),
        cols: vec![
            ("text", "App".into()),
            ("when", "Signed in".into()),
            ("when", "Last used".into()),
            ("text", "What it may do".into()),
            ("actions", String::new()),
        ],
        rows: tokens
            .into_iter()
            .map(|t| {
                vec![
                    Cell::new("text", t.client_id.clone()).code(),
                    Cell::new("when", when(t.created_at)),
                    Cell::new("when", when(t.updated_at)),
                    Cell::new("text", t.scope.clone().unwrap_or_default()),
                    Cell {
                        class: "actions",
                        button: Some(locus_web::console_pages::Button {
                            action: "/account/apps/revoke".into(),
                            label: "Revoke".into(),
                            hidden: vec![("id".into(), t.id.to_string())],
                            danger: false,
                        }),
                        ..Cell::default()
                    },
                ]
            })
            .collect(),
        empty: "No app is signed in with OAuth.".into(),
    });
    web::render(&p, StatusCode::OK)
}

/// `POST /account/apps/revoke`: only one of this account's own.
pub async fn app_revoke(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (u, f) = match form_user(&s, &headers, &body, "/account/apps").await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let id: i64 = web::field(&f, "id").parse().unwrap_or(-1);
    let mine = s
        .store
        .oauth_account_tokens(&u.account.did)
        .await
        .unwrap_or_default()
        .iter()
        .any(|t| t.id == id);
    if mine {
        let _ = s.store.oauth_delete_token(id).await;
    }
    web::redirect("/account/apps")
}

// ---------------------------------------------------------------- export

/// `GET /account/export`.
pub async fn export_page(State(s): St, headers: HeaderMap) -> Response {
    let u = match require_user(&s, &headers, "/account/export").await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let mut p = page(&s, &u, PageId::Export, "Your repository holds everything you've posted, liked and followed, signed so anyone can check it's yours. Media files are stored separately, by their content id.").await;
    p.forms.push(Form {
        title: "Your repository".into(),
        text: "A CAR file, the format servers use to move accounts.".into(),
        action: "/account/export/repo.car".into(),
        get: true,
        inputs: Vec::new(),
        hidden: Vec::new(),
        button: "Download".into(),
        danger: false,
    });
    web::render(&p, StatusCode::OK)
}

/// `GET /account/export/repo.car`.
pub async fn export_repo(State(s): St, headers: HeaderMap) -> Response {
    let u = match require_user(&s, &headers, "/account/export").await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let uri: axum::http::Uri = match format!(
        "/?did={}",
        url::form_urlencoded::byte_serialize(u.account.did.as_bytes()).collect::<String>()
    )
    .parse()
    {
        Ok(u) => u,
        Err(_) => return not_found(),
    };
    let Ok(params) = Query::try_from_uri(&uri) else {
        return not_found();
    };
    match crate::sync::get_repo(State(Arc::clone(&s)), u.bearer(&s), params).await {
        Ok(mut res) => {
            let name = format!("attachment; filename=\"{}.car\"", u.account.handle);
            if let Ok(v) = HeaderValue::from_str(&name) {
                res.headers_mut().insert(header::CONTENT_DISPOSITION, v);
            }
            res
        }
        Err(e) => e.into_response(),
    }
}

// ---------------------------------------------------------------- identity

async fn identity_render(
    s: &AppState,
    u: &User,
    notice: Option<String>,
    errors: Vec<String>,
    status: StatusCode,
) -> Response {
    let mut p = page(s, u, PageId::Identity, "Your DID is your permanent identity; your handle and this server are what it points to now.").await;
    p.notice = notice;
    p.errors = errors;
    let did = &u.account.did;
    match did_document(s, did).await {
        Some(doc) => {
            p.pre = Some((
                "Your DID document".into(),
                serde_json::to_string_pretty(&doc).unwrap_or_default(),
            ))
        }
        None => p.cards.push(Card {
            title: "Your DID document".into(),
            facts: vec![Fact::new(
                if did.starts_with("did:web:") {
                    "Served by your domain"
                } else {
                    "Couldn't be fetched just now"
                },
                did.clone(),
            )],
        }),
    }
    if did.starts_with("did:plc:") {
        p.forms.push(form(
            "Add a recovery key",
            "A recovery key is a did:key you keep somewhere safe, off this server. It can take your identity back if this server ever can't (or won't). Adding one takes a code we email you.",
            "/account/identity/send",
            Vec::new(),
            "Send a code",
        ));
        p.forms.push(form(
            "Then",
            "",
            "/account/identity/recovery",
            vec![
                Input::new("code", "Code", "text"),
                Input::new("key", "Your recovery key (did:key:…)", "text"),
            ],
            "Add the key",
        ));
    }
    web::render(&p, status)
}

/// The account's DID document as JSON: a did:web hosted here from the
/// store, a did:plc from the PLC directory.
async fn did_document(s: &AppState, did: &str) -> Option<Value> {
    if let Ok(Some(web)) = s.store.hosted_did_web(did).await {
        return web.doc.and_then(|d| serde_json::from_str(&d).ok());
    }
    if !did.starts_with("did:plc:") {
        return None;
    }
    let url = format!(
        "{}/{did}",
        s.snapshot().text("identity.plc_url").trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;
    client
        .get(url)
        .send()
        .await
        .ok()?
        .json::<Value>()
        .await
        .ok()
}

/// `GET /account/identity`.
pub async fn identity_page(State(s): St, headers: HeaderMap, Query(q): Query<Q>) -> Response {
    match require_user(&s, &headers, "/account/identity").await {
        Ok(u) => {
            let notice = q.done.map(|d| {
                if d == "sent" {
                    "Sent. Check your email for the code.".into()
                } else {
                    "Your recovery key is added.".into()
                }
            });
            identity_render(&s, &u, notice, Vec::new(), StatusCode::OK).await
        }
        Err(r) => r,
    }
}

/// `POST /account/identity/{send|recovery}`: a PLC operation adding the
/// account's own recovery key ahead of this server's keys.
pub async fn identity_action(
    State(s): St,
    headers: HeaderMap,
    axum::extract::Path(action): axum::extract::Path<String>,
    body: Bytes,
) -> Response {
    let (u, f) = match form_user(&s, &headers, &body, "/account/identity").await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let st = || State(Arc::clone(&s));
    match action.as_str() {
        "send" => {
            match said(
                crate::identity::request_plc_operation_signature(
                    st(),
                    u.bearer(&s),
                    Default::default(),
                )
                .await,
            ) {
                Ok(()) => web::redirect("/account/identity?done=sent"),
                Err(e) => identity_render(&s, &u, None, vec![e], StatusCode::BAD_REQUEST).await,
            }
        }
        "recovery" => {
            let key = web::field(&f, "key").to_owned();
            if !key.starts_with("did:key:") {
                return identity_render(
                    &s,
                    &u,
                    None,
                    vec!["A recovery key is a did:key.".into()],
                    StatusCode::BAD_REQUEST,
                )
                .await;
            }
            let result = async {
                let rec = said(
                    crate::identity::get_recommended_did_credentials(st(), u.bearer(&s)).await,
                )?
                .0;
                let mut keys = vec![json!(key)];
                for k in rec["rotationKeys"].as_array().cloned().unwrap_or_default() {
                    if k != json!(key) {
                        keys.push(k);
                    }
                }
                let op = said(
                    crate::identity::sign_plc_operation(
                        st(),
                        u.bearer(&s),
                        Json(input(
                            json!({"token": web::field(&f, "code"), "rotationKeys": keys}),
                        )),
                    )
                    .await,
                )?
                .0;
                said(
                    crate::identity::submit_plc_operation(
                        st(),
                        u.bearer(&s),
                        Json(input(json!({"operation": op["operation"]}))),
                    )
                    .await,
                )
            }
            .await;
            match result {
                Ok(()) => web::redirect("/account/identity?done=added"),
                Err(e) => identity_render(&s, &u, None, vec![e], StatusCode::BAD_REQUEST).await,
            }
        }
        _ => not_found(),
    }
}

/// `GET /account/moving`: how to move an account here.
pub async fn moving_page(State(s): St, headers: HeaderMap) -> Response {
    let u = match require_user(&s, &headers, "/account/moving").await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let mut p = page(&s, &u, PageId::Moving, "An account elsewhere can move here and keep its DID, followers and posts. The move is run by an app or tool that signs in to both servers.").await;
    p.cards.push(Card {
        title: "The steps".into(),
        facts: vec![
            Fact::new(
                "1",
                format!("Make an account here for your DID, at {}", s.public_url),
            ),
            Fact::new(
                "2",
                "Copy your repository and preferences from the old server, then your media files",
            ),
            Fact::new(
                "3",
                "Get your old server to sign an identity update pointing your DID here",
            ),
            Fact::new("4", "Activate the account here, and deactivate the old one"),
        ],
    });
    p.cards.push(Card {
        title: "Tools that do this".into(),
        facts: vec![
            Fact::new("Apps", "Some apps have a \"move account\" option"),
            Fact::new(
                "Command line",
                "goat account migrate (from the atproto tools)",
            ),
        ],
    });
    web::render(&p, StatusCode::OK)
}

// ---------------------------------------------------------------- leaving

/// `GET /account/deactivate`.
pub async fn deactivate_page(State(s): St, headers: HeaderMap) -> Response {
    let u = match require_user(&s, &headers, "/account/deactivate").await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let mut p = page(&s, &u, PageId::Deactivate, "Deactivating hides your account from the network until you reactivate it. Nothing is deleted.").await;
    let off = u.account.status == Status::Deactivated;
    p.forms.push(Form {
        hidden: vec![("to".into(), if off { "on" } else { "off" }.into())],
        danger: !off,
        ..form(
            if off { "Reactivate" } else { "Deactivate" },
            "",
            "/account/deactivate",
            Vec::new(),
            if off {
                "Reactivate my account"
            } else {
                "Deactivate my account"
            },
        )
    });
    web::render(&p, StatusCode::OK)
}

/// `POST /account/deactivate`.
pub async fn deactivate(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (u, f) = match form_user(&s, &headers, &body, "/account/deactivate").await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let st = || State(Arc::clone(&s));
    let r = if web::field(&f, "to") == "on" {
        said(crate::account::activate_account(st(), u.bearer(&s), Default::default()).await)
    } else {
        said(crate::account::deactivate_account(st(), u.bearer(&s), Json(input(json!({})))).await)
    };
    match r {
        Ok(()) => web::redirect("/account/deactivate"),
        Err(e) => {
            let mut p = page(&s, &u, PageId::Deactivate, "").await;
            p.errors.push(e);
            web::render(&p, StatusCode::BAD_REQUEST)
        }
    }
}

async fn delete_render(
    s: &AppState,
    u: &User,
    notice: Option<String>,
    errors: Vec<String>,
    status: StatusCode,
) -> Response {
    let mut p = page(s, u, PageId::Delete, "Deleting removes your account, your posts and your media from this server for good. It can't be undone.").await;
    p.notice = notice;
    p.errors = errors;
    p.forms.push(form(
        "Get a code",
        "Deleting needs a code we email you.",
        "/account/delete/send",
        Vec::new(),
        "Send a code",
    ));
    p.forms.push(Form {
        danger: true,
        ..form(
            "Delete your account",
            "",
            "/account/delete/confirm",
            vec![
                Input::new("code", "Code", "text"),
                Input::new("password", "Your password", "password"),
            ],
            "Delete my account",
        )
    });
    web::render(&p, status)
}

/// `GET /account/delete`.
pub async fn delete_page(State(s): St, headers: HeaderMap, Query(q): Query<Q>) -> Response {
    match require_user(&s, &headers, "/account/delete").await {
        Ok(u) => {
            delete_render(
                &s,
                &u,
                q.done
                    .map(|_| "Sent. Check your email for the code.".into()),
                Vec::new(),
                StatusCode::OK,
            )
            .await
        }
        Err(r) => r,
    }
}

/// `POST /account/delete/{send|confirm}`.
pub async fn delete_action(
    State(s): St,
    headers: HeaderMap,
    axum::extract::Path(action): axum::extract::Path<String>,
    body: Bytes,
) -> Response {
    let (u, f) = match form_user(&s, &headers, &body, "/account/delete").await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let st = || State(Arc::clone(&s));
    match action.as_str() {
        "send" => match said(crate::account::request_account_delete(st(), u.bearer(&s), Default::default()).await) {
            Ok(()) => web::redirect("/account/delete?done=sent"),
            Err(e) => delete_render(&s, &u, None, vec![e], StatusCode::BAD_REQUEST).await,
        },
        "confirm" => match said(crate::account::delete_account(st(), Json(input(json!({"did": u.account.did, "password": raw(&f, "password"), "token": web::field(&f, "code")})))).await) {
            Ok(()) => {
                let _ = s.store.delete_portal_session(&id_hash(&u.id)).await;
                web::with_cookie(
                    web::render(
                        &MessagePage {
                            shell: Shell::signed_out("Account deleted", brand(&s)),
                            message: "Your account is being deleted. Its posts and media are being removed from this server.".into(),
                            link: None,
                        },
                        StatusCode::OK,
                    ),
                    &cookie(&s, COOKIE, "", 0),
                )
            }
            Err(e) => delete_render(&s, &u, None, vec![e], StatusCode::BAD_REQUEST).await,
        },
        _ => not_found(),
    }
}

// ---------------------------------------------------------------- appearance

/// `GET /account/appearance`.
pub async fn appearance_page(State(s): St, headers: HeaderMap, Query(q): Query<Q>) -> Response {
    let u = match require_user(&s, &headers, "/account/appearance").await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let t = theme_of(&s, &u.account.did).await;
    let mut p = page(&s, &u, PageId::Appearance, "How these pages look to you.").await;
    p.notice = q.done.map(|_| "Saved.".into());
    let pick = |name: &str, label: &str, opts: Vec<Choice>| Input {
        options: opts,
        ..Input::new(name, label, "select")
    };
    p.forms.push(form(
        "Appearance",
        "",
        "/account/appearance",
        vec![
            pick(
                "mode",
                "Light or dark",
                Mode::ALL
                    .into_iter()
                    .map(|m| Choice {
                        value: m.as_str().into(),
                        label: m.label().into(),
                        checked: m == t.mode,
                    })
                    .collect(),
            ),
            pick(
                "scheme",
                "Colours",
                Scheme::ALL
                    .into_iter()
                    .map(|m| Choice {
                        value: m.as_str().into(),
                        label: m.label().into(),
                        checked: m == t.scheme,
                    })
                    .collect(),
            ),
            pick(
                "font",
                "Font",
                Font::ALL
                    .into_iter()
                    .map(|m| Choice {
                        value: m.as_str().into(),
                        label: m.label().into(),
                        checked: m == t.font,
                    })
                    .collect(),
            ),
        ],
        "Save",
    ));
    web::render(&p, StatusCode::OK)
}

/// `POST /account/appearance`.
pub async fn appearance_save(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (u, f) = match form_user(&s, &headers, &body, "/account/appearance").await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let t = Theme::from_stored(
        web::field(&f, "mode"),
        web::field(&f, "scheme"),
        web::field(&f, "font"),
    );
    let _ = s
        .store
        .set_account_theme(
            &u.account.did,
            t.mode.as_str(),
            t.scheme.as_str(),
            t.font.as_str(),
        )
        .await;
    web::redirect("/account/appearance?done=1")
}

/// Sign-up or the portal's overview with sample data, dressed in `dress`,
/// for the branding editor's previews.
pub fn preview_page(
    s: &AppState,
    sign_up: bool,
    name: &str,
    mut dress: locus_web::view::Dress,
    dark: bool,
) -> String {
    use askama::Template;
    dress.html_class = if dark {
        "mode-dark".into()
    } else {
        "mode-light".into()
    };
    if sign_up {
        return SignInPage {
            shell: Shell {
                dress: Some(dress),
                ..Shell::signed_out("Create an account", name)
            },
            heading: String::new(),
            csrf: "preview".into(),
            error: None,
            notice: None,
            identifier: String::new(),
            code_stage: false,
            sign_up: true,
            domains: s.handle_domains(),
            invite_required: s.snapshot().bool("accounts.invite_required"),
            handle: String::new(),
            email: String::new(),
        }
        .render()
        .unwrap_or_default();
    }
    let theme = Theme {
        mode: if dark { Mode::Dark } else { Mode::Light },
        ..Theme::default()
    };
    let nav = menu::nav(
        "@alice.example.com",
        "preview",
        theme,
        PageId::Overview,
        Vec::new(),
    );
    let page = SectionPage {
        shell: Shell {
            dress: Some(dress),
            ..Shell::signed_in("@alice.example.com", name, nav)
        },
        csrf: "preview".into(),
        intro: String::new(),
        notice: None,
        errors: Vec::new(),
        cards: vec![Card {
            title: "Your account".into(),
            facts: vec![
                Fact::new("Handle", "@alice.example.com"),
                Fact::new("Email", "alice@example.com"),
                Fact::new("Email confirmed", "yes").class("ok"),
                Fact::new("Status", "active").class("ok"),
            ],
        }],
        forms: vec![form(
            "Change your handle",
            "",
            "/account/handle",
            vec![Input::new("handle", "New handle", "text").value("alice.example.com")],
            "Change handle",
        )],
        tables: Vec::new(),
        fields: Vec::new(),
        advanced: Vec::new(),
        pre: None,
        frames: Vec::new(),
        can_edit: false,
        back: "/account".into(),
    };
    page.render().unwrap_or_default()
}
