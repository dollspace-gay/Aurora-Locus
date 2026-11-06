//! The console's People pages: accounts, invites and operators. Pages read
//! the store; every change is a call to the admin API as the signed-in
//! operator, so roles, checks and the audit log are the API's own.

#![allow(clippy::result_large_err)]

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use locus_store::accounts::Status;
use locus_web::console::PageId;
use locus_web::console_pages::{
    AccountPage, AccountRow, AccountsPage, AppPasswordRow, Fact, InviteRow, InvitesPage,
    OperatorView, OperatorsPage, TokenRow,
};
use locus_web::setup::Choice;
use locus_web::view::{ConfirmPage, Hidden};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::admin_auth::Role;
use crate::console::{Op, bytes, form_op, need_confirmed, require_op, shell, when};
use crate::state::AppState;
use crate::web;

type St = State<Arc<AppState>>;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/accounts", get(accounts))
        .route("/admin/accounts/create", post(create_account))
        .route("/admin/accounts/{did}", get(account))
        .route(
            "/admin/accounts/{did}/delete",
            get(delete_page).post(delete),
        )
        .route("/admin/accounts/{did}/{action}", post(account_action))
        .route("/admin/invites", get(invites))
        .route("/admin/invites/create", post(create_invites))
        .route("/admin/invites/disable", post(disable_invite))
        .route("/admin/operators", get(operators))
        .route("/admin/operators/{action}", post(operator_action))
}

/// An admin API input the console builds.
fn input<T: DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).expect("the console builds valid API inputs")
}

fn api(r: crate::error::XrpcResult<Json<Value>>) -> Result<Value, String> {
    r.map(|j| j.0).map_err(|e| e.message)
}

/// The same for a method with no output.
fn api_unit(r: crate::error::XrpcResult<()>) -> Result<Value, String> {
    r.map(|()| Value::Null).map_err(|e| e.message)
}

/// An account's page. A `%` in the DID is written `%25`, so the path
/// decodes back to the DID.
#[must_use]
pub fn account_href(did: &str) -> String {
    format!("/admin/accounts/{}", did.replace('%', "%25"))
}

fn status_pill(status: Status) -> (&'static str, &'static str) {
    match status {
        Status::Active => ("active", "on"),
        Status::Takendown => ("taken down", "off"),
        Status::Suspended => ("suspended", "amber"),
        Status::Deactivated => ("deactivated", "amber"),
        Status::Deleted => ("deleted", ""),
    }
}

// ---------------------------------------------------------------- accounts

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    done: Option<String>,
}

/// `GET /admin/accounts?q=`.
pub async fn accounts(State(s): St, headers: HeaderMap, Query(q): Query<ListQuery>) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    render_accounts(
        &s,
        &op,
        &q.q,
        q.cursor.as_deref(),
        q.done.map(|_| "Done.".into()),
        Vec::new(),
        StatusCode::OK,
    )
    .await
}

async fn render_accounts(
    s: &AppState,
    op: &Op,
    q: &str,
    cursor: Option<&str>,
    notice: Option<String>,
    errors: Vec<String>,
    status: StatusCode,
) -> Response {
    const PAGE: i64 = 50;
    let dids = s
        .store
        .search_accounts(q.trim(), cursor, PAGE)
        .await
        .unwrap_or_default();
    let mut rows = Vec::new();
    for did in &dids {
        if let Ok(Some(a)) = s.store.account_by_did(did).await {
            let (word, class) = status_pill(a.status);
            rows.push(AccountRow {
                href: account_href(&a.did),
                handle: a.handle,
                email: a.email.unwrap_or_default(),
                status: word.into(),
                class,
                created: when(a.created_at),
                did: a.did,
            });
        }
    }
    let more = (dids.len() as i64 == PAGE).then(|| {
        format!(
            "/admin/accounts?q={}&cursor={}",
            url::form_urlencoded::byte_serialize(q.as_bytes()).collect::<String>(),
            url::form_urlencoded::byte_serialize(dids.last().map_or("", String::as_str).as_bytes())
                .collect::<String>()
        )
    });
    let page = AccountsPage {
        shell: shell(s, op, PageId::Accounts).await,
        csrf: op.csrf.clone(),
        q: q.to_owned(),
        rows,
        more,
        can_create: op.role >= Role::Admin,
        domains: s.handle_domains().join(", "),
        notice,
        errors,
    };
    web::render(&page, status)
}

/// `POST /admin/accounts/create` (admins): an account with an invite the
/// console makes when sign-ups need one.
pub async fn create_account(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, form) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if op.role < Role::Admin {
        return render_accounts(
            &s,
            &op,
            "",
            None,
            None,
            vec!["Creating accounts needs the admin role.".into()],
            StatusCode::FORBIDDEN,
        )
        .await;
    }
    let raw = |k: &str| {
        form.iter()
            .find(|(n, _)| n == k)
            .map_or("", |(_, v)| v.as_str())
            .to_owned()
    };
    let mut body = json!({
        "handle": web::field(&form, "handle"),
        "email": web::field(&form, "email"),
        "password": raw("password"),
    });
    if s.snapshot().bool("accounts.invite_required") {
        let code = api(crate::admin::create_invite_code(
            State(Arc::clone(&s)),
            op.api_headers(),
            Json(input(json!({"useCount": 1}))),
        )
        .await);
        match code {
            Ok(v) => body["inviteCode"] = v["code"].clone(),
            Err(e) => {
                return render_accounts(&s, &op, "", None, None, vec![e], StatusCode::BAD_REQUEST)
                    .await;
            }
        }
    }
    match api(crate::server::create_account(
        State(Arc::clone(&s)),
        HeaderMap::new(),
        Json(input(body)),
    )
    .await)
    {
        Ok(v) => {
            let did = v["did"].as_str().unwrap_or_default().to_owned();
            let _ = s
                .store
                .audit(
                    &op.actor(),
                    "account.create",
                    Some(&did),
                    Some(&json!({"handle": v["handle"]})),
                )
                .await;
            web::redirect(&format!("{}?done=created", account_href(&did)))
        }
        Err(e) => render_accounts(&s, &op, "", None, None, vec![e], StatusCode::BAD_REQUEST).await,
    }
}

#[derive(Deserialize)]
pub struct DoneQuery {
    #[serde(default)]
    done: Option<String>,
}

fn done_text(key: &str) -> &'static str {
    match key {
        "created" => "Account created.",
        "takedown" => "Takedown changed.",
        "deactivate" => "Deactivation changed.",
        "private-block" => "Private-post block changed.",
        "invites" => "Invite codes changed.",
        "handle" => "Handle changed.",
        "email" => "Email changed.",
        "password" => "Password reset; every sign-in of the account ended.",
        _ => "Done.",
    }
}

/// `GET /admin/accounts/{did}`.
pub async fn account(
    State(s): St,
    headers: HeaderMap,
    Path(did): Path<String>,
    Query(q): Query<DoneQuery>,
) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    render_account(
        &s,
        &op,
        &did,
        q.done.as_deref().map(|d| done_text(d).to_owned()),
        Vec::new(),
        StatusCode::OK,
    )
    .await
}

async fn render_account(
    s: &AppState,
    op: &Op,
    did: &str,
    notice: Option<String>,
    errors: Vec<String>,
    status: StatusCode,
) -> Response {
    let Ok(Some(a)) = s.store.account_by_did(did).await else {
        return (StatusCode::NOT_FOUND, "No such account").into_response();
    };
    let (word, _) = status_pill(a.status);
    let mut facts = vec![
        Fact::new("Status", word).class(if a.status == Status::Active {
            "ok"
        } else {
            "warn"
        }),
        Fact::new("Email", a.email.clone().unwrap_or_else(|| "none".into())),
        Fact::new(
            "Email confirmed",
            a.email_confirmed_at.map_or_else(|| "no".into(), when),
        ),
        Fact::new("Created", when(a.created_at)),
        Fact::new("DID method", a.did_method.clone()),
    ];
    if let Some(r) = &a.status_ref {
        facts.push(Fact::new("Takedown reference", r.clone()));
    }
    if let Some(t) = a.deactivated_at {
        facts.push(Fact::new("Deactivated", when(t)));
    }
    if let Some(t) = a.delete_after {
        facts.push(Fact::new("Deleted after", when(t)).class("warn"));
    }
    if a.email_auth_factor_at.is_some() {
        facts.push(Fact::new("Email sign-in codes", "on"));
    }
    if let Ok(Some(code)) = s.store.invited_by(did).await {
        facts.push(Fact::new("Invited with", code));
    }
    let (records, blobs, blob_bytes) = s.store.account_stats(did).await.unwrap_or_default();
    let root = s.store.repo_root_of(did).await.ok().flatten();
    let stats = vec![
        Fact::new("Records", records.to_string()),
        Fact::new("Blobs", format!("{blobs} ({})", bytes(blob_bytes))),
        Fact::new(
            "Repo revision",
            root.as_ref()
                .map_or_else(|| "none".into(), |r| r.rev.clone()),
        ),
    ];
    let blocked = s.store.private_tier_blocked(did).await.unwrap_or(false);
    let mut private = vec![
        Fact::new("Blocked here", if blocked { "yes" } else { "no" }).class(if blocked {
            "bad"
        } else {
            ""
        }),
    ];
    for source in [
        locus_store::kryphocron::SOURCE_MEMBERSHIP,
        locus_store::kryphocron::SOURCE_PROBE,
    ] {
        if let Ok(Some(p)) = s.store.kr_permission(did, source).await {
            private.push(Fact::new(
                format!("AppView says ({source})"),
                format!("{} · checked {}", p.status, when(p.checked_at)),
            ));
        }
    }
    if private.len() == 1 {
        private.push(Fact::new("AppView says", "nothing yet"));
    }
    let app_passwords = s
        .store
        .app_passwords(did)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|p| AppPasswordRow {
            name: p.name,
            created: when(p.created_at),
            privileged: p.privileged,
        })
        .collect();
    let invites = s
        .store
        .invite_codes_for(did)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(invite_row)
        .collect();
    let mut sh = shell(s, op, PageId::Accounts).await;
    sh.title = format!("@{}", a.handle);
    let page = AccountPage {
        shell: sh,
        csrf: op.csrf.clone(),
        base: account_href(did),
        did: a.did.clone(),
        handle: a.handle.clone(),
        facts,
        stats,
        private,
        app_passwords,
        invites,
        taken_down: a.status == Status::Takendown,
        deactivated: a.status == Status::Deactivated,
        blocked,
        invites_disabled: a.invites_disabled,
        email: a.email.unwrap_or_default(),
        can_moderate: op.role >= Role::Moderator,
        can_admin: op.role >= Role::Admin,
        notice,
        errors,
    };
    web::render(&page, status)
}

fn invite_row(c: locus_store::invites::InviteCode) -> InviteRow {
    InviteRow {
        uses: c.uses.len(),
        code: c.code,
        available: c.available,
        for_account: c.for_account,
        created: when(c.created_at),
        disabled: c.disabled,
    }
}

/// `POST /admin/accounts/{did}/{action}`.
pub async fn account_action(
    State(s): St,
    headers: HeaderMap,
    Path((did, action)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let (op, form) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let st = || State(Arc::clone(&s));
    let h = op.api_headers();
    let repo = json!({"$type": "com.atproto.admin.defs#repoRef", "did": did});
    let flag = |k: &str| web::field(&form, k) == "true";
    let result = match action.as_str() {
        "takedown" => {
            let mut t = json!({"applied": flag("applied")});
            let r = web::field(&form, "ref");
            if flag("applied") && !r.is_empty() {
                t["ref"] = json!(r);
            }
            api(crate::admin::update_subject_status(
                st(),
                h,
                Json(input(json!({"subject": repo, "takedown": t}))),
            )
            .await)
        }
        "deactivate" => api(crate::admin::update_subject_status(
            st(),
            h,
            Json(input(
                json!({"subject": repo, "deactivated": {"applied": flag("applied")}}),
            )),
        )
        .await),
        "private-block" => api(crate::locus_admin::set_private_tier_block(
            st(),
            h,
            Json(input(json!({"did": did, "blocked": flag("blocked")}))),
        )
        .await),
        "invites" => {
            let body = Json(input(json!({"account": did})));
            if flag("disabled") {
                api_unit(crate::admin::disable_account_invites(st(), h, body).await)
            } else {
                api_unit(crate::admin::enable_account_invites(st(), h, body).await)
            }
        }
        "handle" => api_unit(
            crate::admin::update_account_handle(
                st(),
                h,
                Json(input(
                    json!({"did": did, "handle": web::field(&form, "handle")}),
                )),
            )
            .await,
        ),
        "email" => api_unit(
            crate::admin::update_account_email(
                st(),
                h,
                Json(input(
                    json!({"account": did, "email": web::field(&form, "email")}),
                )),
            )
            .await,
        ),
        "password" => {
            let pw = form
                .iter()
                .find(|(k, _)| k == "password")
                .map_or("", |(_, v)| v.as_str());
            if pw.is_empty() {
                Err("Enter the new password.".into())
            } else {
                api_unit(
                    crate::admin::update_account_password(
                        st(),
                        h,
                        Json(input(json!({"did": did, "password": pw}))),
                    )
                    .await,
                )
            }
        }
        _ => return (StatusCode::NOT_FOUND, "Not found").into_response(),
    };
    match result {
        Ok(_) => web::redirect(&format!("{}?done={action}", account_href(&did))),
        Err(e) => render_account(&s, &op, &did, None, vec![e], StatusCode::BAD_REQUEST).await,
    }
}

async fn delete_confirm(
    s: &AppState,
    op: &Op,
    did: &str,
    handle: &str,
    error: Option<String>,
    status: StatusCode,
) -> Response {
    let mut sh = shell(s, op, PageId::Accounts).await;
    sh.title = format!("Delete @{handle}?");
    web::render(
        &ConfirmPage {
            shell: sh,
            csrf: op.csrf.clone(),
            error,
            what: format!(
                "This deletes @{handle} ({did}): its repo, its records and its blobs, and ends every sign-in and connected app."
            ),
            undo: "It can't be undone.".into(),
            action: format!("{}/delete", account_href(did)),
            back: account_href(did),
            hidden: Vec::<Hidden>::new(),
            typed: Some("delete".into()),
            button: "Delete the account".into(),
        },
        status,
    )
}

/// `GET /admin/accounts/{did}/delete` (admins, freshly confirmed).
pub async fn delete_page(State(s): St, headers: HeaderMap, Path(did): Path<String>) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    if op.role < Role::Admin {
        return render_account(
            &s,
            &op,
            &did,
            None,
            vec!["Deleting accounts needs the admin role.".into()],
            StatusCode::FORBIDDEN,
        )
        .await;
    }
    if let Err(r) = need_confirmed(&op, &format!("{}/delete", account_href(&did))) {
        return r;
    }
    let Ok(Some(a)) = s.store.account_by_did(&did).await else {
        return (StatusCode::NOT_FOUND, "No such account").into_response();
    };
    delete_confirm(&s, &op, &did, &a.handle, None, StatusCode::OK).await
}

/// `POST /admin/accounts/{did}/delete`: the typed word, and a fresh
/// confirmation.
pub async fn delete(
    State(s): St,
    headers: HeaderMap,
    Path(did): Path<String>,
    body: Bytes,
) -> Response {
    let (op, form) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = need_confirmed(&op, &format!("{}/delete", account_href(&did))) {
        return r;
    }
    let Ok(Some(a)) = s.store.account_by_did(&did).await else {
        return (StatusCode::NOT_FOUND, "No such account").into_response();
    };
    if web::field(&form, "typed") != "delete" {
        return delete_confirm(
            &s,
            &op,
            &did,
            &a.handle,
            Some("Type delete to confirm.".into()),
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    match api_unit(
        crate::admin::delete_account(
            State(Arc::clone(&s)),
            op.api_headers(),
            Json(input(json!({"did": did}))),
        )
        .await,
    ) {
        Ok(_) => web::redirect("/admin/accounts?done=deleted"),
        Err(e) => delete_confirm(&s, &op, &did, &a.handle, Some(e), StatusCode::BAD_REQUEST).await,
    }
}

// ---------------------------------------------------------------- invites

#[derive(Deserialize)]
pub struct InvitesQuery {
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
}

async fn render_invites(
    s: &AppState,
    op: &Op,
    sort: &str,
    cursor: Option<&str>,
    new_codes: Vec<String>,
    errors: Vec<String>,
    status: StatusCode,
) -> Response {
    const PAGE: i64 = 100;
    let sort = if sort == "usage" { "usage" } else { "recent" };
    let codes = s
        .store
        .all_invite_codes(sort == "usage", cursor, PAGE)
        .await
        .unwrap_or_default();
    let more = (codes.len() as i64 == PAGE).then(|| {
        format!(
            "/admin/invites?sort={sort}&cursor={}",
            url::form_urlencoded::byte_serialize(
                codes.last().map_or("", |c| c.code.as_str()).as_bytes()
            )
            .collect::<String>()
        )
    });
    let page = InvitesPage {
        shell: shell(s, op, PageId::Invites).await,
        csrf: op.csrf.clone(),
        rows: codes.into_iter().map(invite_row).collect(),
        sort: sort.into(),
        more,
        can_moderate: op.role >= Role::Moderator,
        required: s.snapshot().bool("accounts.invite_required"),
        notice: None,
        new_codes,
        errors,
    };
    web::render(&page, status)
}

/// `GET /admin/invites?sort=`.
pub async fn invites(State(s): St, headers: HeaderMap, Query(q): Query<InvitesQuery>) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    render_invites(
        &s,
        &op,
        q.sort.as_deref().unwrap_or("recent"),
        q.cursor.as_deref(),
        Vec::new(),
        Vec::new(),
        StatusCode::OK,
    )
    .await
}

/// `POST /admin/invites/create`.
pub async fn create_invites(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, form) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let n = |k: &str, max: i64| {
        web::field(&form, k)
            .parse::<i64>()
            .unwrap_or(1)
            .clamp(1, max)
    };
    let result = api(crate::admin::create_invite_codes(
        State(Arc::clone(&s)),
        op.api_headers(),
        Json(input(
            json!({"codeCount": n("count", 100), "useCount": n("uses", 1000)}),
        )),
    )
    .await);
    match result {
        Ok(v) => {
            let codes = v["codes"][0]["codes"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|c| c.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            render_invites(&s, &op, "recent", None, codes, Vec::new(), StatusCode::OK).await
        }
        Err(e) => {
            render_invites(
                &s,
                &op,
                "recent",
                None,
                Vec::new(),
                vec![e],
                StatusCode::BAD_REQUEST,
            )
            .await
        }
    }
}

/// `POST /admin/invites/disable`.
pub async fn disable_invite(State(s): St, headers: HeaderMap, body: Bytes) -> Response {
    let (op, form) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let code = web::field(&form, "code");
    match api_unit(
        crate::admin::disable_invite_codes(
            State(Arc::clone(&s)),
            op.api_headers(),
            Json(input(json!({"codes": [code]}))),
        )
        .await,
    ) {
        Ok(_) => web::redirect("/admin/invites"),
        Err(e) => {
            render_invites(
                &s,
                &op,
                "recent",
                None,
                Vec::new(),
                vec![e],
                StatusCode::BAD_REQUEST,
            )
            .await
        }
    }
}

// ---------------------------------------------------------------- operators

fn role_choices(selected: &str) -> Vec<Choice> {
    [
        ("owner", "Owner"),
        ("admin", "Admin"),
        ("moderator", "Moderator"),
        ("viewer", "Viewer"),
    ]
    .into_iter()
    .map(|(v, l)| Choice {
        value: v.into(),
        label: l.into(),
        checked: v == selected,
    })
    .collect()
}

async fn render_operators(
    s: &AppState,
    op: &Op,
    new_token: Option<(String, String)>,
    notice: Option<String>,
    errors: Vec<String>,
    status: StatusCode,
) -> Response {
    let mut rows = Vec::new();
    for o in s.store.operators().await.unwrap_or_default() {
        let tokens = s
            .store
            .operator_tokens(&o.username)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|t| TokenRow {
                id: t.id,
                name: t.name,
                created: when(t.created_at),
                last_used: t.last_used_at.map_or_else(|| "never".into(), when),
                expires: t.expires_at.map_or_else(|| "never".into(), when),
            })
            .collect();
        rows.push(OperatorView {
            me: o.username == op.row.username,
            roles: role_choices(&o.role),
            role: o.role,
            disabled: o.disabled,
            totp: o.totp_secret.is_some(),
            created: when(o.created_at),
            tokens,
            username: o.username,
        });
    }
    let page = OperatorsPage {
        shell: shell(s, op, PageId::Operators).await,
        csrf: op.csrf.clone(),
        rows,
        roles: role_choices("viewer"),
        new_token,
        notice,
        errors,
    };
    web::render(&page, status)
}

/// `GET /admin/operators` (owners, freshly confirmed).
pub async fn operators(State(s): St, headers: HeaderMap, Query(q): Query<DoneQuery>) -> Response {
    let op = match require_op(&s, &headers).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    if op.role < Role::Owner {
        return (StatusCode::FORBIDDEN, "Operators are managed by owners.").into_response();
    }
    if let Err(r) = need_confirmed(&op, "/admin/operators") {
        return r;
    }
    render_operators(
        &s,
        &op,
        None,
        q.done.map(|_| "Done.".into()),
        Vec::new(),
        StatusCode::OK,
    )
    .await
}

/// `POST /admin/operators/{action}`.
pub async fn operator_action(
    State(s): St,
    headers: HeaderMap,
    Path(action): Path<String>,
    body: Bytes,
) -> Response {
    let (op, form) = match form_op(&s, &headers, &body).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = need_confirmed(&op, "/admin/operators") {
        return r;
    }
    let st = || State(Arc::clone(&s));
    let h = op.api_headers();
    let raw = |k: &str| {
        form.iter()
            .find(|(n, _)| n == k)
            .map_or("", |(_, v)| v.as_str())
            .to_owned()
    };
    let user = web::field(&form, "username");
    let result = match action.as_str() {
        "create" => api(crate::locus_admin::create_operator(
            st(),
            h,
            Json(input(json!({
                "username": user, "password": raw("password"), "role": web::field(&form, "role"),
            }))),
        )
        .await),
        "role" => api(crate::locus_admin::update_operator(
            st(),
            h,
            Json(input(
                json!({"username": user, "role": web::field(&form, "role")}),
            )),
        )
        .await),
        "disable" => {
            if user == op.row.username {
                Err("You can't disable yourself.".into())
            } else {
                api(crate::locus_admin::update_operator(st(), h, Json(input(json!({"username": user, "disabled": web::field(&form, "disabled") == "true"})))).await)
            }
        }
        "token" => {
            let mut body = json!({"operator": web::field(&form, "operator"), "name": web::field(&form, "name")});
            if let Ok(d) = web::field(&form, "days").parse::<i64>() {
                body["expiresInDays"] = json!(d);
            }
            match api(crate::locus_admin::create_api_token(st(), h, Json(input(body))).await) {
                Ok(v) => {
                    let token = v["token"].as_str().unwrap_or_default().to_owned();
                    return render_operators(
                        &s,
                        &op,
                        Some((web::field(&form, "operator").to_owned(), token)),
                        None,
                        Vec::new(),
                        StatusCode::OK,
                    )
                    .await;
                }
                Err(e) => Err(e),
            }
        }
        "revoke-token" => api(crate::locus_admin::revoke_api_token(
            st(),
            h,
            Json(input(json!({"id": web::field(&form, "id")}))),
        )
        .await),
        _ => return (StatusCode::NOT_FOUND, "Not found").into_response(),
    };
    match result {
        Ok(_) => web::redirect("/admin/operators?done=1"),
        Err(e) => render_operators(&s, &op, None, None, vec![e], StatusCode::BAD_REQUEST).await,
    }
}
