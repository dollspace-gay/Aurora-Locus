//! The operator console at `/admin`: sign-in (password, then the sign-in
//! code when the operator has a code app), sessions, re-confirmation for
//! sensitive actions, and its pages. Forms that change the PDS go through
//! the same admin API the CLI uses, authenticated as the session
//! (`Authorization: Console <session>`, see [`crate::admin_auth`]).

// Handlers return a ready response as their error.
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use locus_auth::{password, totp};
use locus_core::settings::{self, Kind, REGISTRY};
use locus_store::console::SessionRow;
use locus_store::email::Sender;
use locus_store::operators::OperatorRow;
use locus_web::console::{self as menu, PageId};
use locus_web::console_pages::{
    AppearancePage, Enrolling, Fact, HomePage, JobRow, PortalPage, ReauthPage, SecurityPage,
    SessionView, SettingField, SettingGroup, SettingsPage, SignInPage,
};
use locus_web::setup::Choice;
use locus_web::theme::{Font, Mode, Scheme, Theme};
use locus_web::view::{Banner, Link, Shell};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::admin_auth::Role;
use crate::state::AppState;
use crate::web;

type St = State<Arc<AppState>>;

const COOKIE: &str = "locus-console";
/// The sign-in form's CSRF cookie, before there's a session.
const PRE_COOKIE: &str = "locus-console-pre";
/// Every sign-in ends after this, used or not.
pub const SESSION_TTL_MS: i64 = 12 * 3_600_000;
/// A sign-in waiting for its code lasts this long.
const CODE_STAGE_TTL_MS: i64 = 5 * 60_000;
/// Sensitive actions need the password (and code) confirmed this recently.
pub const CONFIRM_WINDOW_MS: i64 = 10 * 60_000;

/// The console's routes.
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin", get(home))
        .route("/admin/sign-in", get(sign_in_page).post(sign_in))
        .route("/admin/sign-in/code", get(code_page).post(sign_in_code))
        .route("/admin/sign-out", post(sign_out))
        .route("/admin/confirm-identity", get(confirm_page).post(confirm))
        .route("/admin/settings", get(settings_page))
        .route("/admin/settings/save", post(save_setting))
        .route(
            "/admin/appearance",
            get(appearance_page).post(save_appearance),
        )
        .route("/admin/security", get(security_page))
        .route("/admin/security/password", post(change_password))
        .route("/admin/security/totp/start", post(totp_start))
        .route("/admin/security/totp/confirm", post(totp_confirm))
        .route("/admin/security/totp/off", post(totp_off))
        .route("/admin/security/sign-out", post(end_session))
        .route("/admin/portal", get(portal_page))
}

// ---------------------------------------------------------------- sessions

#[must_use]
pub fn session_hash(id: &str) -> String {
    hex::encode(Sha256::digest(
        format!("locus-console-session:{id}").as_bytes(),
    ))
}

fn csrf_for(id: &str) -> String {
    hex::encode(&Sha256::digest(format!("locus-console-csrf:{id}").as_bytes())[..16])
}

fn idle_ms(s: &AppState) -> i64 {
    s.snapshot().int("admin.session_idle_minutes") * 60_000
}

fn cookie_header(s: &AppState, name: &str, value: &str, max_age_s: i64) -> String {
    let secure = if s.public_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    format!("{name}={value}; Path=/admin; Max-Age={max_age_s}; HttpOnly; SameSite=Strict{secure}")
}

/// A signed-in operator.
pub struct Op {
    pub row: OperatorRow,
    pub session: SessionRow,
    /// The raw session id (the `Console` credential).
    pub id: String,
    pub csrf: String,
    pub role: Role,
}

impl Op {
    pub fn actor(&self) -> String {
        format!("operator:{}", self.row.username)
    }

    /// Whether the password (and code) were confirmed recently.
    pub fn confirmed_recently(&self) -> bool {
        locus_core::time::now_ms() - self.session.confirmed_at < CONFIRM_WINDOW_MS
    }

    /// Headers that call the admin API as this session.
    pub fn api_headers(&self) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Ok(v) = format!("Console {}", self.id).parse() {
            h.insert(header::AUTHORIZATION, v);
        }
        h
    }
}

async fn session_at(
    s: &AppState,
    headers: &HeaderMap,
) -> Option<(String, SessionRow, OperatorRow)> {
    let all = web::cookies(headers);
    let id = web::cookie(&all, COOKIE)?.to_owned();
    if id.len() != 64 {
        return None;
    }
    let (row, op) = s
        .store
        .operator_session(&session_hash(&id), idle_ms(s))
        .await
        .ok()??;
    Some((id, row, op))
}

/// The signed-in operator (a full session), if any.
pub async fn signed_in(s: &AppState, headers: &HeaderMap) -> Option<Op> {
    let (id, session, row) = session_at(s, headers).await?;
    if session.stage != "full" {
        return None;
    }
    Some(Op {
        role: Role::parse(&row.role).unwrap_or(Role::Viewer),
        csrf: csrf_for(&id),
        row,
        session,
        id,
    })
}

/// The operator, or a redirect to sign in.
pub(crate) async fn require_op(s: &AppState, headers: &HeaderMap) -> Result<Op, Response> {
    signed_in(s, headers)
        .await
        .ok_or_else(|| web::redirect("/admin/sign-in"))
}

/// A form post from this operator: same site, with the session's CSRF
/// token.
pub(crate) fn check_form(
    op: &Op,
    headers: &HeaderMap,
    form: &[(String, String)],
) -> Result<(), Response> {
    if web::same_site(headers) && web::ct_eq(web::field(form, "csrf"), &op.csrf) {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            "Request refused: the form expired. Go back and try again.",
        )
            .into_response())
    }
}

pub(crate) fn brand(s: &AppState) -> String {
    let n = s.snapshot().text("server.service_name").to_owned();
    if n.is_empty() { s.hostname.clone() } else { n }
}

async fn theme_of(s: &AppState, op: &OperatorRow) -> Theme {
    match s.store.operator_theme(&op.username).await {
        Ok(Some(t)) => Theme::from_stored(&t.theme_mode, &t.theme_scheme, &t.theme_font),
        _ => Theme::default(),
    }
}

/// Lines across the top of every console page until their cause is gone.
async fn banners(s: &AppState, op: &Op) -> Vec<Banner> {
    let mut out = Vec::new();
    if op.row.totp_secret.is_none() {
        out.push(Banner {
            bad: false,
            text: "Your operator account has no sign-in code app yet.".into(),
            link: Some(Link {
                href: PageId::Security.href().into(),
                label: "Set one up".into(),
            }),
        });
    }
    if !s.mailer.configured(Sender::Main).await.unwrap_or(false) {
        out.push(Banner {
            bad: false,
            text: "Email isn't set up, so people can't confirm addresses, reset passwords by email or get sign-in codes.".into(),
            link: None,
        });
    }
    out.extend(node_banners(s).await);
    out
}

/// Warnings from the bootstrap values live nodes report: SSRF protection
/// off, dev mode ignored, and values nodes disagree on.
async fn node_banners(s: &AppState) -> Vec<Banner> {
    let Ok(reports) = s.store.live_bootstrap_reports().await else {
        return Vec::new();
    };
    let nodes_with = |name: &str, value: &str| -> Vec<String> {
        let hash = locus_store::env::bootstrap_hash(value);
        reports
            .get(name)
            .into_iter()
            .flatten()
            .filter(|(_, h)| *h == hash)
            .map(|(n, _)| n.clone())
            .collect()
    };
    let mut out = Vec::new();
    let off = nodes_with("ssrf_protection", "off");
    if !off.is_empty() {
        out.push(Banner {
            bad: true,
            text: format!(
                "SSRF protection is off on {} (PDS_DISABLE_SSRF_PROTECTION or dev mode): requests users can influence may reach loopback and private addresses.",
                off.join(", ")
            ),
            link: None,
        });
    }
    let ignored = nodes_with("dev_mode", "ignored");
    if !ignored.is_empty() {
        out.push(Banner {
            bad: false,
            text: format!(
                "PDS_DEV_MODE is ignored on {}: the hostname isn't local.",
                ignored.join(", ")
            ),
            link: None,
        });
    }
    let refused = nodes_with("service_did_env", "refused");
    if !refused.is_empty() {
        out.push(Banner {
            bad: false,
            text: format!(
                "PDS_SERVICE_DID on {} differs from this install's service DID ({}) and is ignored: an explicit service DID can't change in 0.1.",
                refused.join(", "),
                s.service_did
            ),
            link: None,
        });
    }
    let snap = s.snapshot();
    for key in snap.list("http.allow_insecure_http") {
        if locus_core::settings::plain_http(snap.get(key)) {
            out.push(Banner {
                bad: false,
                text: format!(
                    "{key} uses plain http://: requests to it travel unencrypted. Use https:// unless it's on a private network."
                ),
                link: None,
            });
        }
    }
    for (name, rows) in &reports {
        if rows.iter().any(|(_, h)| *h != rows[0].1) {
            let nodes: Vec<&str> = rows.iter().map(|(n, _)| n.as_str()).collect();
            out.push(Banner {
                bad: true,
                text: format!(
                    "The nodes {} disagree on {name}: every node must use the same value.",
                    nodes.join(", ")
                ),
                link: None,
            });
        }
    }
    out
}

/// The frame of a console page.
pub(crate) async fn shell(s: &AppState, op: &Op, page: PageId) -> Shell {
    let theme = theme_of(s, &op.row).await;
    let nav = menu::nav(
        &op.row.username,
        &op.row.role,
        &op.csrf,
        theme,
        page,
        banners(s, op).await,
    );
    Shell::signed_in(page.title(), brand(s), nav)
}

pub(crate) fn refused(why: &str) -> Response {
    (StatusCode::FORBIDDEN, why.to_owned()).into_response()
}

/// When `ms` was, for a table: `2026-11-05 20:11 UTC`.
#[must_use]
pub fn when(ms: i64) -> String {
    let iso = locus_core::time::iso8601(ms);
    format!("{} UTC", iso[..16].replace('T', " "))
}

pub(crate) fn back_or(form: &[(String, String)], default: &str) -> String {
    let b = web::field(form, "back");
    if menu::safe_back(b) {
        b.to_owned()
    } else {
        default.to_owned()
    }
}

// ---------------------------------------------------------------- sign-in

fn sign_in_render(
    s: &AppState,
    csrf: &str,
    error: Option<String>,
    username: &str,
    code_stage: bool,
    status: StatusCode,
) -> Response {
    web::render(
        &SignInPage {
            shell: Shell::signed_out("Sign in", brand(s)),
            csrf: csrf.to_owned(),
            error,
            notice: None,
            username: username.to_owned(),
            code_stage,
        },
        status,
    )
}

/// `GET /admin/sign-in`.
pub async fn sign_in_page(State(s): St, headers: HeaderMap) -> Response {
    if signed_in(&s, &headers).await.is_some() {
        return web::redirect("/admin");
    }
    let pre = hex::encode(rand::random::<[u8; 16]>());
    web::with_cookie(
        sign_in_render(&s, &pre, None, "", false, StatusCode::OK),
        &cookie_header(&s, PRE_COOKIE, &pre, 3600),
    )
}

fn pre_ok(headers: &HeaderMap, form: &[(String, String)]) -> bool {
    let all = web::cookies(headers);
    let pre = web::cookie(&all, PRE_COOKIE).unwrap_or("");
    web::same_site(headers) && web::ct_eq(web::field(form, "csrf"), pre)
}

/// `POST /admin/sign-in`: the password. With a code app, the session
/// waits for the code.
pub async fn sign_in(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let form = web::parse_form(&body);
    let csrf = web::field(&form, "csrf").to_owned();
    if !pre_ok(&headers, &form) {
        return sign_in_render(
            &s,
            &csrf,
            Some("The form expired; try again.".into()),
            "",
            false,
            StatusCode::FORBIDDEN,
        );
    }
    let username = web::field(&form, "username").to_owned();
    let pw = form
        .iter()
        .find(|(k, _)| k == "password")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    let limited = crate::ratelimit::limit_ip(&s, &[crate::ratelimit::buckets::CONSOLE_SIGN_IN])
        .await
        .is_err()
        || crate::ratelimit::limit(
            &s,
            &[(
                crate::ratelimit::buckets::CONSOLE_SIGN_IN_NAME,
                &username,
                1,
            )],
        )
        .await
        .is_err();
    if limited {
        return sign_in_render(
            &s,
            &csrf,
            Some("Too many tries. Wait a few minutes, then try again.".into()),
            &username,
            false,
            StatusCode::TOO_MANY_REQUESTS,
        );
    }
    let op = s.store.operator(&username).await.ok().flatten();
    // A missing operator costs the same as a wrong password.
    let ok = match &op {
        Some(o) if !o.disabled => password::verify(pw, &o.password_hash),
        _ => {
            let _ = password::verify(pw, DUMMY_HASH.as_str());
            false
        }
    };
    let Some(op) = op.filter(|_| ok) else {
        return sign_in_render(
            &s,
            &csrf,
            Some("That username and password don't match.".into()),
            &username,
            false,
            StatusCode::UNAUTHORIZED,
        );
    };
    let id = hex::encode(rand::random::<[u8; 32]>());
    let with_code = op.totp_secret.is_some();
    let (stage, ttl) = if with_code {
        ("code", CODE_STAGE_TTL_MS)
    } else {
        ("full", SESSION_TTL_MS)
    };
    let ip = crate::ratelimit::client_ip().map(|i| i.to_string());
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|u| u.chars().take(200).collect::<String>());
    if let Err(e) = s
        .store
        .create_operator_session(
            &session_hash(&id),
            &op.username,
            stage,
            ttl,
            ip.as_deref(),
            ua.as_deref(),
        )
        .await
    {
        return sign_in_render(
            &s,
            &csrf,
            Some(e.to_string()),
            &username,
            false,
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }
    if !with_code {
        let _ = s
            .store
            .audit(
                &format!("operator:{}", op.username),
                "console.sign_in",
                None,
                None,
            )
            .await;
    }
    let to = if with_code {
        "/admin/sign-in/code"
    } else {
        "/admin"
    };
    web::with_cookie(
        web::redirect(to),
        &cookie_header(&s, COOKIE, &id, SESSION_TTL_MS / 1000),
    )
}

/// A hash to check against when the operator doesn't exist, so the answer
/// takes as long.
static DUMMY_HASH: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| password::hash("not a real password").unwrap_or_default());

/// `GET /admin/sign-in/code`.
pub async fn code_page(State(s): St, headers: HeaderMap) -> Response {
    match session_at(&s, &headers).await {
        Some((id, row, op)) if row.stage == "code" => {
            sign_in_render(&s, &csrf_for(&id), None, &op.username, true, StatusCode::OK)
        }
        Some(_) => web::redirect("/admin"),
        None => web::redirect("/admin/sign-in"),
    }
}

pub(crate) fn totp_secret_name(operator: &str) -> String {
    format!("operator/{operator}/totp")
}

fn totp_pending_name(operator: &str) -> String {
    format!("operator/{operator}/totp_pending")
}

/// Whether `code` is a current, unused code of the operator's code app
/// (or of `secret_name`'s secret); each code works once.
pub(crate) async fn code_ok(s: &AppState, operator: &str, secret_name: &str, code: &str) -> bool {
    let Ok(Some(secret)) = s.store.get_secret(secret_name, &s.master_key).await else {
        return false;
    };
    let Some(step) = totp::verify(&secret, code, locus_core::time::now_ms() / 1000) else {
        return false;
    };
    s.store.use_totp_step(operator, step).await.unwrap_or(false)
}

/// `POST /admin/sign-in/code`.
pub async fn sign_in_code(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let Some((id, row, op)) = session_at(&s, &headers).await else {
        return web::redirect("/admin/sign-in");
    };
    if row.stage != "code" {
        return web::redirect("/admin");
    }
    let form = web::parse_form(&body);
    let csrf = csrf_for(&id);
    if !web::same_site(&headers) || !web::ct_eq(web::field(&form, "csrf"), &csrf) {
        return sign_in_render(
            &s,
            &csrf,
            Some("The form expired; try again.".into()),
            &op.username,
            true,
            StatusCode::FORBIDDEN,
        );
    }
    if crate::ratelimit::limit(
        &s,
        &[(
            crate::ratelimit::buckets::CONSOLE_SIGN_IN_NAME,
            &op.username,
            1,
        )],
    )
    .await
    .is_err()
    {
        return sign_in_render(
            &s,
            &csrf,
            Some("Too many tries. Wait a few minutes, then try again.".into()),
            &op.username,
            true,
            StatusCode::TOO_MANY_REQUESTS,
        );
    }
    if !code_ok(
        &s,
        &op.username,
        &totp_secret_name(&op.username),
        web::field(&form, "code"),
    )
    .await
    {
        return sign_in_render(
            &s,
            &csrf,
            Some("That code isn't right (or was already used). Try the next one.".into()),
            &op.username,
            true,
            StatusCode::UNAUTHORIZED,
        );
    }
    // The code stage's short life becomes a full sign-in's.
    let hash = session_hash(&id);
    let _ = s.store.delete_operator_session(&hash).await;
    let ip = crate::ratelimit::client_ip().map(|i| i.to_string());
    let new_id = hex::encode(rand::random::<[u8; 32]>());
    if let Err(e) = s
        .store
        .create_operator_session(
            &session_hash(&new_id),
            &op.username,
            "full",
            SESSION_TTL_MS,
            ip.as_deref(),
            row.user_agent.as_deref(),
        )
        .await
    {
        return sign_in_render(
            &s,
            &csrf,
            Some(e.to_string()),
            &op.username,
            true,
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }
    let _ = s
        .store
        .audit(
            &format!("operator:{}", op.username),
            "console.sign_in",
            None,
            None,
        )
        .await;
    web::with_cookie(
        web::redirect("/admin"),
        &cookie_header(&s, COOKIE, &new_id, SESSION_TTL_MS / 1000),
    )
}

/// `POST /admin/sign-out`.
pub async fn sign_out(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let form = web::parse_form(&body);
    if let Some((id, _, _)) = session_at(&s, &headers).await
        && web::same_site(&headers)
        && web::ct_eq(web::field(&form, "csrf"), &csrf_for(&id))
    {
        let _ = s.store.delete_operator_session(&session_hash(&id)).await;
    }
    web::with_cookie(
        web::redirect("/admin/sign-in"),
        &cookie_header(&s, COOKIE, "", 0),
    )
}

// ---------------------------------------------------------------- re-confirming

/// For a sensitive action: Ok when the operator confirmed recently, else
/// the page asking them to.
pub fn need_confirmed(op: &Op, next: &str) -> Result<(), Response> {
    if op.confirmed_recently() {
        return Ok(());
    }
    Err(web::redirect(&format!(
        "/admin/confirm-identity?next={}",
        url::form_urlencoded::byte_serialize(next.as_bytes()).collect::<String>()
    )))
}

#[derive(Deserialize)]
pub struct NextQuery {
    #[serde(default)]
    next: String,
}

async fn confirm_render(
    s: &AppState,
    op: &Op,
    next: &str,
    error: Option<String>,
    status: StatusCode,
) -> Response {
    let next = if menu::safe_back(next) {
        next
    } else {
        "/admin"
    };
    let mut shell = shell(s, op, PageId::Security).await;
    shell.title = "Confirm it's you".into();
    web::render(
        &ReauthPage {
            shell,
            csrf: op.csrf.clone(),
            next: next.to_owned(),
            totp: op.row.totp_secret.is_some(),
            error,
        },
        status,
    )
}

/// `GET /admin/confirm-identity?next=`.
pub async fn confirm_page(
    State(s): St,
    headers: HeaderMap,
    Query(q): Query<NextQuery>,
) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    confirm_render(&s, &op, &q.next, None, StatusCode::OK).await
}

/// `POST /admin/confirm-identity`.
pub async fn confirm(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let form = web::parse_form(&body);
    if let Err(r) = check_form(&op, &headers, &form) {
        return r;
    }
    let next = web::field(&form, "next").to_owned();
    if crate::ratelimit::limit(
        &s,
        &[(
            crate::ratelimit::buckets::CONSOLE_SIGN_IN_NAME,
            &op.row.username,
            1,
        )],
    )
    .await
    .is_err()
    {
        return confirm_render(
            &s,
            &op,
            &next,
            Some("Too many tries. Wait a few minutes.".into()),
            StatusCode::TOO_MANY_REQUESTS,
        )
        .await;
    }
    let pw = form
        .iter()
        .find(|(k, _)| k == "password")
        .map_or("", |(_, v)| v.as_str());
    let mut ok = password::verify(pw, &op.row.password_hash);
    if ok && op.row.totp_secret.is_some() {
        ok = code_ok(
            &s,
            &op.row.username,
            &totp_secret_name(&op.row.username),
            web::field(&form, "code"),
        )
        .await;
    }
    if !ok {
        return confirm_render(
            &s,
            &op,
            &next,
            Some("That didn't match. Try again.".into()),
            StatusCode::UNAUTHORIZED,
        )
        .await;
    }
    let _ = s
        .store
        .confirm_operator_session(&session_hash(&op.id))
        .await;
    web::redirect(if menu::safe_back(&next) {
        &next
    } else {
        "/admin"
    })
}

// ---------------------------------------------------------------- Home

/// `GET /admin`.
pub async fn home(State(s): St, headers: HeaderMap) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let snap = s.snapshot();
    let counts = s.store.console_counts().await.unwrap_or_default();
    let nodes = s
        .store
        .live_slot_holders(locus_store::env::CLOCK_SLOT_FAMILY)
        .await
        .unwrap_or_default();
    let email = s.mailer.status(Sender::Main).await.ok().flatten();
    let on_off = |b: bool| if b { "on" } else { "off" };
    let facts = vec![
        Fact::new("Version", env!("CARGO_PKG_VERSION")),
        Fact::new("Hostname", s.hostname.clone()),
        Fact::new("Nodes running", nodes.len().to_string()),
        Fact::new(
            "Email",
            match email {
                Some(locus_store::email::SenderStatus::Verified) => "working",
                Some(locus_store::email::SenderStatus::Unverified) => {
                    "connected, delivery unconfirmed"
                }
                Some(locus_store::email::SenderStatus::Failed) => "failing",
                None => "not set up",
            },
        )
        .class(match email {
            Some(locus_store::email::SenderStatus::Verified) => "ok",
            Some(locus_store::email::SenderStatus::Failed) => "bad",
            _ => "warn",
        }),
        Fact::new(
            "Sign-ups",
            if snap.bool("accounts.invite_required") {
                "with an invite"
            } else {
                "open"
            },
        ),
        Fact::new(
            "Private posts",
            on_off(snap.bool("kryphocron.private_tier_enabled")),
        ),
        Fact::new("Account portal", on_off(snap.bool("portal.enabled"))),
        Fact::new("Settings version", snap.version().to_string()),
    ];
    let mut count_facts: Vec<Fact> = counts
        .accounts
        .iter()
        .map(|(status, n)| Fact::new(format!("Accounts, {status}"), n.to_string()))
        .collect();
    if count_facts.is_empty() {
        count_facts.push(Fact::new("Accounts", "0"));
    }
    count_facts.extend([
        Fact::new("Records", counts.records.to_string()),
        Fact::new(
            "Blobs",
            format!("{} ({})", counts.blobs, bytes(counts.blob_bytes)),
        ),
        Fact::new("Quarantined blobs", counts.quarantined.to_string())
            .class(if counts.quarantined > 0 { "warn" } else { "" }),
        Fact::new("Firehose sequence", counts.last_seq.to_string()),
        Fact::new("Events waiting to be sequenced", counts.outbox.to_string())
            .class(if counts.outbox > 1000 { "warn" } else { "" }),
    ]);
    let states = s.store.job_states().await.unwrap_or_default();
    let mut attention = Vec::new();
    for j in states.iter().filter(|j| j.last_error.is_some()) {
        attention.push(
            Fact::new(
                format!("{} is failing:", j.name),
                j.last_error.clone().unwrap_or_default(),
            )
            .class("bad"),
        );
    }
    if counts.quarantined > 0 {
        attention.push(Fact::new(
            "Uploads wait for review:",
            format!("{} quarantined blob(s).", counts.quarantined),
        ));
    }
    let jobs = states
        .into_iter()
        .map(|j| JobRow {
            last_run: j.last_run_at.map(when).unwrap_or_else(|| "never".into()),
            state: if j.last_error.is_some() {
                "failing".into()
            } else {
                "ok".into()
            },
            class: if j.last_error.is_some() {
                "red"
            } else {
                "fine"
            },
            error: j.last_error.unwrap_or_default(),
            name: j.name,
        })
        .collect();
    let page = HomePage {
        shell: shell(&s, &op, PageId::Home).await,
        attention,
        facts,
        counts: count_facts,
        jobs,
    };
    web::render(&page, StatusCode::OK)
}

/// A byte count for people.
#[must_use]
pub fn bytes(n: i64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} bytes")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

// ---------------------------------------------------------------- settings

/// A setting as a form field, with its current value.
#[must_use]
pub fn setting_field(snap: &locus_core::settings::Snapshot, def: &settings::Def) -> SettingField {
    let current = snap.get(def.key).clone();
    let default = def.default_value();
    let text = |v: &Value| match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|i| i.as_str().map_or_else(|| i.to_string(), str::to_owned))
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    };
    let (input, min, max, options) = match def.kind {
        Kind::Bool => ("checkbox", String::new(), String::new(), Vec::new()),
        Kind::Int { min, max } => ("number", min.to_string(), max.to_string(), Vec::new()),
        Kind::Choice(opts) => (
            "select",
            String::new(),
            String::new(),
            opts.iter()
                .map(|o| Choice {
                    value: (*o).into(),
                    label: (*o).into(),
                    checked: current.as_str() == Some(o),
                })
                .collect(),
        ),
        Kind::TextList { .. } => ("textarea", String::new(), String::new(), Vec::new()),
        Kind::Text { .. } | Kind::Url => ("text", String::new(), String::new(), Vec::new()),
    };
    let default_text = match &default {
        Value::String(s) if s.is_empty() => "(empty)".to_owned(),
        Value::Array(a) if a.is_empty() => "(none)".to_owned(),
        Value::Array(_) => text(&default).replace('\n', ", "),
        v => text(v),
    };
    SettingField {
        key: def.key.into(),
        description: def.description.into(),
        input,
        checked: current.as_bool().unwrap_or(false),
        value: text(&current),
        options,
        min,
        max,
        is_default: current == default,
        default: default_text,
        env: def.env.unwrap_or("").into(),
    }
}

fn group_label(prefix: &str) -> String {
    match prefix {
        "log" => "Logging".into(),
        "server" => "This server".into(),
        "http" => "HTTP".into(),
        "identity" => "Identity and handles".into(),
        "accounts" => "Accounts and sign-up".into(),
        "email" => "Email".into(),
        "limits" => "Limits".into(),
        "appview" => "AppView".into(),
        "moderation" => "Moderation".into(),
        "proxy" => "Proxying to other services".into(),
        "oauth" => "OAuth".into(),
        "rate_limits" => "Rate limits".into(),
        "sync" => "Sync and the firehose".into(),
        "blobs" => "Blobs and scanning".into(),
        "kryphocron" => "Private posts (kryphocron)".into(),
        "admin" => "The console and admin API".into(),
        "portal" => "Account portal".into(),
        "deploy" => "Deployment".into(),
        other => other.to_owned(),
    }
}

#[derive(Deserialize)]
pub struct SettingsQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    saved: Option<String>,
}

/// `GET /admin/settings?q=`.
pub async fn settings_page(
    State(s): St,
    headers: HeaderMap,
    Query(q): Query<SettingsQuery>,
) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let snap = s.snapshot();
    let needle = q.q.trim().to_ascii_lowercase();
    let mut groups: Vec<SettingGroup> = Vec::new();
    for def in REGISTRY {
        if !needle.is_empty()
            && !def.key.to_ascii_lowercase().contains(&needle)
            && !def.description.to_ascii_lowercase().contains(&needle)
        {
            continue;
        }
        let prefix = def.key.split('.').next().unwrap_or("");
        let field = setting_field(&snap, def);
        match groups.iter_mut().find(|g| g.key == prefix) {
            Some(g) => g.fields.push(field),
            None => groups.push(SettingGroup {
                key: prefix.into(),
                label: group_label(prefix),
                fields: vec![field],
            }),
        }
    }
    let back = if needle.is_empty() {
        "/admin/settings".to_owned()
    } else {
        format!(
            "/admin/settings?q={}",
            url::form_urlencoded::byte_serialize(q.q.as_bytes()).collect::<String>()
        )
    };
    let page = SettingsPage {
        shell: shell(&s, &op, PageId::Settings).await,
        csrf: op.csrf.clone(),
        q: q.q,
        groups,
        can_edit: op.role >= Role::Admin,
        back,
        notice: q.saved.map(|k| format!("Saved {k}. It applies now.")),
        errors: Vec::new(),
    };
    web::render(&page, StatusCode::OK)
}

/// `POST /admin/settings/save`: one setting, through the admin API's own
/// method (so it's checked, live and audited the same way).
pub async fn save_setting(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let form = web::parse_form(&body);
    if let Err(r) = check_form(&op, &headers, &form) {
        return r;
    }
    let key = web::field(&form, "key").to_owned();
    let back = back_or(&form, "/admin/settings");
    let Ok(def) = settings::def(&key) else {
        return refused("Unknown setting");
    };
    let raw = form
        .iter()
        .find(|(k, _)| k == "value")
        .map(|(_, v)| v.as_str());
    let value = match def.kind {
        Kind::Bool => Ok(Value::Bool(raw == Some("true"))),
        Kind::TextList { .. } => Ok(Value::Array(
            web::lines(raw.unwrap_or(""))
                .into_iter()
                .map(Value::String)
                .collect(),
        )),
        _ => def
            .parse_env(raw.unwrap_or("").trim())
            .map_err(|e| e.to_string()),
    };
    let result = match value {
        Ok(v) => crate::locus_admin::set_setting(
            State(Arc::clone(&s)),
            op.api_headers(),
            axum::Json(
                serde_json::from_value(serde_json::json!({"key": key, "value": v}))
                    .expect("setting input"),
            ),
        )
        .await
        .map(|_| ())
        .map_err(|e| e.message),
        Err(e) => Err(e),
    };
    match result {
        Ok(()) => {
            let _ = s.settings.refresh().await;
            let sep = if back.contains('?') { '&' } else { '?' };
            web::redirect(&format!(
                "{back}{sep}saved={}",
                url::form_urlencoded::byte_serialize(key.as_bytes()).collect::<String>()
            ))
        }
        Err(e) => {
            // Show the page it came from with the problem.
            let snap = s.snapshot();
            let mut shell = shell(&s, &op, PageId::Settings).await;
            shell.title = "Not saved".into();
            let field = setting_field(&snap, def);
            web::render(
                &SettingsPage {
                    shell,
                    csrf: op.csrf.clone(),
                    q: String::new(),
                    groups: vec![SettingGroup {
                        key: key.split('.').next().unwrap_or("").into(),
                        label: group_label(key.split('.').next().unwrap_or("")),
                        fields: vec![field],
                    }],
                    can_edit: op.role >= Role::Admin,
                    back,
                    notice: None,
                    errors: vec![e],
                },
                StatusCode::BAD_REQUEST,
            )
        }
    }
}

// ---------------------------------------------------------------- Appearance

#[derive(Deserialize)]
pub struct SavedQuery {
    #[serde(default)]
    saved: Option<String>,
}

/// `GET /admin/appearance`.
pub async fn appearance_page(
    State(s): St,
    headers: HeaderMap,
    Query(q): Query<SavedQuery>,
) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let t = theme_of(&s, &op.row).await;
    let page = AppearancePage {
        shell: shell(&s, &op, PageId::Appearance).await,
        csrf: op.csrf.clone(),
        modes: Mode::ALL
            .into_iter()
            .map(|m| Choice {
                value: m.as_str().into(),
                label: m.label().into(),
                checked: m == t.mode,
            })
            .collect(),
        schemes: Scheme::ALL
            .into_iter()
            .map(|m| Choice {
                value: m.as_str().into(),
                label: m.label().into(),
                checked: m == t.scheme,
            })
            .collect(),
        fonts: Font::ALL
            .into_iter()
            .map(|m| Choice {
                value: m.as_str().into(),
                label: m.label().into(),
                checked: m == t.font,
            })
            .collect(),
        saved: q.saved.is_some(),
    };
    web::render(&page, StatusCode::OK)
}

/// `POST /admin/appearance`. The operator's own; not audited.
pub async fn save_appearance(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let form = web::parse_form(&body);
    if let Err(r) = check_form(&op, &headers, &form) {
        return r;
    }
    let t = Theme::from_stored(
        web::field(&form, "mode"),
        web::field(&form, "scheme"),
        web::field(&form, "font"),
    );
    if let Err(e) = s
        .store
        .set_operator_theme(
            &op.row.username,
            t.mode.as_str(),
            t.scheme.as_str(),
            t.font.as_str(),
        )
        .await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    web::redirect("/admin/appearance?saved=1")
}

// ---------------------------------------------------------------- Sign-in security

async fn security_render(
    s: &AppState,
    op: &Op,
    notice: Option<String>,
    errors: Vec<String>,
    status: StatusCode,
) -> Response {
    // Fresh: a change on this page may have just turned the code app on.
    let row = s
        .store
        .operator(&op.row.username)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| op.row.clone());
    let enrolling = if row.totp_secret.is_none() {
        s.store
            .get_secret(&totp_pending_name(&row.username), &s.master_key)
            .await
            .ok()
            .flatten()
            .map(|secret| Enrolling {
                secret: totp::secret_grouped(&secret),
                uri: totp::uri(&secret, &brand(s), &row.username),
            })
    } else {
        None
    };
    let this = session_hash(&op.id);
    let sessions = s
        .store
        .operator_sessions(&row.username)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.stage == "full")
        .map(|r| SessionView {
            this_one: r.id_hash == this,
            started: when(r.created_at),
            last_used: when(r.last_seen_at),
            from: format!(
                "{}{}",
                r.ip.unwrap_or_else(|| "unknown address".into()),
                r.user_agent.map(|u| format!(", {u}")).unwrap_or_default()
            ),
            id: r.id_hash,
        })
        .collect();
    let op_now = Op {
        row: row.clone(),
        ..clone_op(op)
    };
    let page = SecurityPage {
        shell: shell(s, &op_now, PageId::Security).await,
        csrf: op.csrf.clone(),
        totp_on: row.totp_secret.is_some(),
        enrolling,
        sessions,
        notice,
        errors,
    };
    web::render(&page, status)
}

fn clone_op(op: &Op) -> Op {
    Op {
        row: op.row.clone(),
        session: op.session.clone(),
        id: op.id.clone(),
        csrf: op.csrf.clone(),
        role: op.role,
    }
}

/// `GET /admin/security`.
pub async fn security_page(State(s): St, headers: HeaderMap) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    security_render(&s, &op, None, Vec::new(), StatusCode::OK).await
}

pub(crate) async fn form_op(
    s: &AppState,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<(Op, Vec<(String, String)>), Response> {
    let op = require_op(s, headers).await?;
    let form = web::parse_form(body);
    check_form(&op, headers, &form)?;
    Ok((op, form))
}

/// `POST /admin/security/password`.
pub async fn change_password(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, form) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let raw = |k: &str| {
        form.iter()
            .find(|(n, _)| n == k)
            .map_or("", |(_, v)| v.as_str())
            .to_owned()
    };
    let (current, new, new2) = (raw("current"), raw("new"), raw("new2"));
    let err = if !password::verify(&current, &op.row.password_hash) {
        Some("The current password isn't right.")
    } else if new != new2 {
        Some("The two new passwords differ.")
    } else if new.chars().count() < locus_core::setup::MIN_OPERATOR_PASSWORD
        || new.len() > password::MAX_NEW_PASSWORD
    {
        Some("Operator passwords are at least 12 characters.")
    } else {
        None
    };
    if let Some(e) = err {
        return security_render(&s, &op, None, vec![e.into()], StatusCode::BAD_REQUEST).await;
    }
    let hash = match password::hash(&new) {
        Ok(h) => h,
        Err(e) => {
            return security_render(&s, &op, None, vec![e], StatusCode::INTERNAL_SERVER_ERROR)
                .await;
        }
    };
    let this = session_hash(&op.id);
    let result = async {
        s.store
            .update_operator(&op.row.username, None, Some(&hash), None)
            .await?;
        s.store
            .delete_operator_sessions(&op.row.username, Some(&this))
            .await?;
        s.store.confirm_operator_session(&this).await?;
        s.store
            .audit(
                &op.actor(),
                "operator.password",
                Some(&op.row.username),
                None,
            )
            .await
    }
    .await;
    match result {
        Ok(()) => {
            security_render(
                &s,
                &op,
                Some("Password changed. You're signed out everywhere else.".into()),
                Vec::new(),
                StatusCode::OK,
            )
            .await
        }
        Err(e) => {
            security_render(
                &s,
                &op,
                None,
                vec![e.to_string()],
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .await
        }
    }
}

/// `POST /admin/security/totp/start`: a new secret, not yet in use.
pub async fn totp_start(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, _) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if op.row.totp_secret.is_some() {
        return web::redirect("/admin/security");
    }
    let secret = totp::new_secret();
    let result = async {
        let mut tx = s.store.begin().await?;
        tx.put_secret(&totp_pending_name(&op.row.username), &secret, &s.master_key)
            .await?;
        tx.commit().await
    }
    .await;
    match result {
        Ok(()) => web::redirect("/admin/security"),
        Err(e) => {
            security_render(
                &s,
                &op,
                None,
                vec![e.to_string()],
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .await
        }
    }
}

/// `POST /admin/security/totp/confirm`: the app's first code turns it on.
pub async fn totp_confirm(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, form) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let name = op.row.username.clone();
    let Ok(Some(secret)) = s
        .store
        .get_secret(&totp_pending_name(&name), &s.master_key)
        .await
    else {
        return web::redirect("/admin/security");
    };
    let Some(step) = totp::verify(
        &secret,
        web::field(&form, "code"),
        locus_core::time::now_ms() / 1000,
    ) else {
        return security_render(
            &s,
            &op,
            None,
            vec![
                "That code isn't right. Check the app's clock, and enter the code it shows now."
                    .into(),
            ],
            StatusCode::BAD_REQUEST,
        )
        .await;
    };
    let result = async {
        let mut tx = s.store.begin().await?;
        tx.put_secret(&totp_secret_name(&name), &secret, &s.master_key)
            .await?;
        tx.commit().await?;
        s.store.delete_secret(&totp_pending_name(&name)).await?;
        s.store.set_operator_totp(&name, true).await?;
        // The code that turned it on can't sign in too.
        s.store.use_totp_step(&name, step).await?;
        s.store
            .audit(&op.actor(), "operator.totp_on", Some(&name), None)
            .await
    }
    .await;
    match result {
        Ok(()) => {
            security_render(
                &s,
                &op,
                Some("Your sign-in code app is on. Signing in now asks for its code.".into()),
                Vec::new(),
                StatusCode::OK,
            )
            .await
        }
        Err(e) => {
            security_render(
                &s,
                &op,
                None,
                vec![e.to_string()],
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .await
        }
    }
}

/// `POST /admin/security/totp/off`: needs a current code.
pub async fn totp_off(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, form) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let name = op.row.username.clone();
    if !code_ok(
        &s,
        &name,
        &totp_secret_name(&name),
        web::field(&form, "code"),
    )
    .await
    {
        return security_render(
            &s,
            &op,
            None,
            vec!["That code isn't right (or was already used).".into()],
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    let result = async {
        s.store.set_operator_totp(&name, false).await?;
        s.store.delete_secret(&totp_secret_name(&name)).await?;
        s.store
            .audit(&op.actor(), "operator.totp_off", Some(&name), None)
            .await
    }
    .await;
    match result {
        Ok(()) => {
            security_render(
                &s,
                &op,
                Some("Your sign-in code app is off.".into()),
                Vec::new(),
                StatusCode::OK,
            )
            .await
        }
        Err(e) => {
            security_render(
                &s,
                &op,
                None,
                vec![e.to_string()],
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .await
        }
    }
}

/// `POST /admin/security/sign-out`: ends another of the operator's own
/// sign-ins.
pub async fn end_session(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, form) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let target = web::field(&form, "session");
    let mine = s
        .store
        .operator_sessions(&op.row.username)
        .await
        .unwrap_or_default()
        .iter()
        .any(|r| r.id_hash == target);
    if mine {
        let _ = s.store.delete_operator_session(target).await;
    }
    web::redirect("/admin/security")
}

// ---------------------------------------------------------------- Account portal

/// `GET /admin/portal`.
pub async fn portal_page(
    State(s): St,
    headers: HeaderMap,
    Query(q): Query<SavedQuery>,
) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let snap = s.snapshot();
    let enabled = snap.bool("portal.enabled");
    let page = PortalPage {
        shell: shell(&s, &op, PageId::Portal).await,
        csrf: op.csrf.clone(),
        enabled,
        url: format!("{}/account", s.public_url),
        can_edit: op.role >= Role::Admin,
        back: PageId::Portal.href().into(),
        fields: Vec::new(),
        notice: q.saved.map(|_| {
            if enabled {
                "The account portal is on.".into()
            } else {
                "The account portal is off.".into()
            }
        }),
        errors: Vec::new(),
    };
    web::render(&page, StatusCode::OK)
}
