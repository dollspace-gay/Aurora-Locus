//! The reference's admin API (`com.atproto.admin.*`, the invite admin
//! endpoints), for operators, the legacy admin password and the moderation
//! service, each method gated by a minimum operator role.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use locus_auth::password;
use locus_core::time::iso8601;
use locus_repo::event::account_body;
use locus_store::accounts::{Account, SessionFilter, Status};
use locus_store::invites::InviteCode;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::admin_auth::{self, Caller, Role, Surface};
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

type St = State<Arc<AppState>>;

const NS: &str = "com.atproto.admin.";

/// The reference surface; `mod_service` where the reference lets the
/// moderation service in (its `moderator` verifier).
async fn caller(
    s: &AppState,
    headers: &HeaderMap,
    method: &str,
    min: Role,
    mod_service: bool,
) -> XrpcResult<Caller> {
    admin_auth::require(s, headers, method, Surface::Reference { mod_service }, min).await
}

fn not_found(m: &str) -> XrpcError {
    XrpcError::named("NotFound", m)
}

fn internal(e: impl std::fmt::Display) -> XrpcError {
    XrpcError::internal(e.to_string())
}

/// An account as an admin sees it (gone accounts are absent).
pub(crate) async fn any_account(s: &AppState, did: &str) -> XrpcResult<Option<Account>> {
    Ok(s.store
        .account_by_did(did)
        .await?
        .filter(|a| a.status != Status::Deleted))
}

fn code_detail(c: &InviteCode) -> Value {
    json!({
        "code": c.code,
        "available": c.available,
        "disabled": c.disabled,
        "forAccount": c.for_account,
        "createdBy": c.created_by,
        "createdAt": iso8601(c.created_at),
        "uses": c.uses.iter().map(|(by, at)| json!({"usedBy": by, "usedAt": iso8601(*at)})).collect::<Vec<_>>(),
    })
}

/// `com.atproto.admin.defs#accountView`.
async fn account_view(s: &AppState, a: &Account) -> XrpcResult<Value> {
    let mut v = json!({
        "did": a.did,
        "handle": a.handle,
        "indexedAt": iso8601(a.created_at),
        "invitesDisabled": a.invites_disabled,
    });
    // A shard doesn't manage invites (the entryway does), as the reference.
    let invites = s.entryway.is_none();
    if invites {
        v["invites"] = json!(
            s.store
                .invite_codes_for(&a.did)
                .await?
                .iter()
                .map(code_detail)
                .collect::<Vec<_>>()
        );
    }
    if let Some(e) = &a.email {
        v["email"] = json!(e);
    }
    if let Some(t) = a.email_confirmed_at {
        v["emailConfirmedAt"] = json!(iso8601(t));
    }
    if let Some(t) = a.deactivated_at {
        v["deactivatedAt"] = json!(iso8601(t));
    }
    if invites
        && let Some(code) = s.store.invited_by(&a.did).await?
        && let Some(c) = s.store.invite_code(&code).await?
    {
        v["invitedBy"] = code_detail(&c);
    }
    Ok(v)
}

// ---------------------------------------------------------------- accounts

#[derive(Deserialize)]
pub struct DidParam {
    did: String,
}

pub async fn get_account_info(
    State(s): St,
    headers: HeaderMap,
    Query(p): Query<DidParam>,
) -> XrpcResult<Json<Value>> {
    caller(
        &s,
        &headers,
        "com.atproto.admin.getAccountInfo",
        Role::Viewer,
        true,
    )
    .await?;
    let a = any_account(&s, &p.did)
        .await?
        .ok_or_else(|| not_found("Account not found"))?;
    Ok(Json(account_view(&s, &a).await?))
}

/// `dids` repeated in the query.
pub async fn get_account_infos(
    State(s): St,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> XrpcResult<Json<Value>> {
    caller(
        &s,
        &headers,
        "com.atproto.admin.getAccountInfos",
        Role::Viewer,
        true,
    )
    .await?;
    let dids: Vec<String> = uri
        .query()
        .map(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .filter(|(k, _)| k == "dids")
                .map(|(_, v)| v.into_owned())
                .collect()
        })
        .unwrap_or_default();
    let mut infos = Vec::new();
    for did in dids {
        if let Some(a) = any_account(&s, &did).await? {
            infos.push(account_view(&s, &a).await?);
        }
    }
    Ok(Json(json!({"infos": infos})))
}

#[derive(Deserialize)]
pub struct SearchParams {
    #[serde(alias = "email", alias = "term")]
    q: Option<String>,
    cursor: Option<String>,
    limit: Option<i64>,
}

/// `com.atproto.admin.searchAccounts` (a locus addition: the reference
/// doesn't implement it): accounts by handle or email prefix, or DID.
pub async fn search_accounts(
    State(s): St,
    headers: HeaderMap,
    Query(p): Query<SearchParams>,
) -> XrpcResult<Json<Value>> {
    caller(
        &s,
        &headers,
        "com.atproto.admin.searchAccounts",
        Role::Viewer,
        false,
    )
    .await?;
    let limit = p.limit.unwrap_or(50).clamp(1, 100);
    let dids = s
        .store
        .search_accounts(p.q.as_deref().unwrap_or(""), p.cursor.as_deref(), limit)
        .await?;
    let mut accounts = Vec::new();
    for did in &dids {
        if let Some(a) = any_account(&s, did).await? {
            accounts.push(account_view(&s, &a).await?);
        }
    }
    let mut out = json!({"accounts": accounts});
    if dids.len() as i64 == limit
        && let Some(last) = dids.last()
    {
        out["cursor"] = json!(last);
    }
    Ok(Json(out))
}

// ---------------------------------------------------------------- subject status

#[derive(Deserialize)]
pub struct SubjectParams {
    did: Option<String>,
    uri: Option<String>,
    blob: Option<String>,
}

fn status_attr(takedown: Option<&str>) -> Value {
    match takedown {
        Some(r) => json!({"applied": true, "ref": r}),
        None => json!({"applied": false}),
    }
}

/// Each subject's own takedown status: an account, a record (by URI), or a
/// blob (with `did`).
pub async fn get_subject_status(
    State(s): St,
    headers: HeaderMap,
    Query(p): Query<SubjectParams>,
) -> XrpcResult<Json<Value>> {
    caller(
        &s,
        &headers,
        "com.atproto.admin.getSubjectStatus",
        Role::Viewer,
        true,
    )
    .await?;
    let missing = || not_found("Subject not found");
    if let Some(blob) = &p.blob {
        let did = p.did.as_deref().ok_or_else(|| {
            XrpcError::invalid_request("Must provide a did to request blob state")
        })?;
        let row = s.store.blob(did, blob).await?.ok_or_else(missing)?;
        return Ok(Json(json!({
            "subject": {"$type": "com.atproto.admin.defs#repoBlobRef", "did": did, "cid": blob},
            "takedown": status_attr(row.takedown_ref.as_deref()),
        })));
    }
    if let Some(uri) = &p.uri {
        let (did, collection, rkey) = locus_kryphocron::rules::split_uri(uri)
            .ok_or_else(|| XrpcError::invalid_request("Invalid uri"))?;
        let (cid, takedown) = s
            .store
            .record_entry(did, collection, rkey)
            .await?
            .ok_or_else(missing)?;
        return Ok(Json(json!({
            "subject": {"$type": "com.atproto.repo.strongRef", "uri": uri, "cid": cid},
            "takedown": status_attr(takedown.as_deref()),
        })));
    }
    let did = p
        .did
        .as_deref()
        .ok_or_else(|| XrpcError::invalid_request("No provided subject"))?;
    let a = any_account(&s, did).await?.ok_or_else(missing)?;
    let takedown = matches!(a.status, Status::Takendown | Status::Suspended)
        .then(|| a.status_ref.clone().unwrap_or_default());
    Ok(Json(json!({
        "subject": {"$type": "com.atproto.admin.defs#repoRef", "did": did},
        "takedown": status_attr(takedown.as_deref()),
        "deactivated": {"applied": a.status == Status::Deactivated},
    })))
}

/// Takes an account down (or restores it): status, every session and
/// OAuth grant ended, `#account` emitted. Takes effect on the next request.
pub(crate) async fn set_account_takedown(
    s: &AppState,
    did: &str,
    takedown_ref: Option<&str>,
) -> XrpcResult<()> {
    let (_lock, mut tx) = s.repo.locked_tx(did).await.map_err(internal)?;
    match takedown_ref {
        Some(r) => {
            tx.set_account_status(did, Status::Takendown, Some(r))
                .await?;
            tx.revoke_sessions(did, SessionFilter::All).await?;
        }
        None => tx.set_account_status(did, Status::Active, None).await?,
    }
    let body = match takedown_ref {
        Some(_) => account_body(did, false, Some("takendown")),
        None => account_body(did, true, None),
    }
    .map_err(internal)?;
    tx.insert_outbox(did, "account", &body).await?;
    tx.commit().await?;
    if takedown_ref.is_some() {
        s.store.oauth_delete_account_tokens(did).await?;
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct UpdateSubjectInput {
    subject: Value,
    takedown: Option<StatusAttrInput>,
    deactivated: Option<StatusAttrInput>,
}

#[derive(Deserialize, Clone)]
pub struct StatusAttrInput {
    applied: bool,
    #[serde(rename = "ref")]
    reference: Option<String>,
}

/// Takedowns of accounts (`repoRef`), records (`strongRef`, by URI
/// whatever the CID) and blobs (`repoBlobRef`); account deactivation.
pub async fn update_subject_status(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<UpdateSubjectInput>,
) -> XrpcResult<Json<Value>> {
    let who = caller(
        &s,
        &headers,
        "com.atproto.admin.updateSubjectStatus",
        Role::Moderator,
        true,
    )
    .await?;
    if input.takedown.as_ref().is_some_and(|t| t.applied)
        && input.deactivated.as_ref().is_some_and(|d| !d.applied)
    {
        return Err(XrpcError::invalid_request(
            "Cannot activate and takedown an account at the same time",
        ));
    }
    let kind = input.subject["$type"].as_str().unwrap_or_default();
    let reference =
        |t: &StatusAttrInput| t.applied.then(|| t.reference.clone().unwrap_or_default());
    if let Some(t) = &input.takedown {
        let r = reference(t);
        match kind {
            "com.atproto.admin.defs#repoRef" => {
                let did = input.subject["did"]
                    .as_str()
                    .ok_or_else(|| XrpcError::invalid_request("Invalid subject"))?;
                any_account(&s, did)
                    .await?
                    .ok_or_else(|| not_found("Account not found"))?;
                set_account_takedown(&s, did, r.as_deref()).await?;
            }
            "com.atproto.repo.strongRef" => {
                let uri = input.subject["uri"].as_str().unwrap_or_default();
                let (did, collection, rkey) = locus_kryphocron::rules::split_uri(uri)
                    .ok_or_else(|| XrpcError::invalid_request("Invalid subject"))?;
                s.store
                    .set_record_takedown(did, collection, rkey, r.as_deref())
                    .await?;
            }
            "com.atproto.admin.defs#repoBlobRef" => {
                let (did, cid) = (
                    input.subject["did"].as_str().unwrap_or_default(),
                    input.subject["cid"].as_str().unwrap_or_default(),
                );
                s.store.set_blob_takedown(did, cid, r.as_deref()).await?;
                // Into quarantine, or back out of it.
                crate::repo::place_blob(&s, did, cid).await;
            }
            other => {
                return Err(XrpcError::invalid_request(format!(
                    "Invalid subject ({other})"
                )));
            }
        }
    }
    if let (Some(d), "com.atproto.admin.defs#repoRef") = (&input.deactivated, kind) {
        let did = input.subject["did"].as_str().unwrap_or_default();
        let (_lock, mut tx) = s.repo.locked_tx(did).await.map_err(internal)?;
        let body = if d.applied {
            let st = tx
                .deactivate_account(did, None)
                .await?
                .ok_or_else(|| not_found("Account not found"))?;
            account_body(did, false, Some(st.as_str()))
        } else {
            tx.activate_account(did).await?;
            account_body(did, true, None)
        }
        .map_err(internal)?;
        tx.insert_outbox(did, "account", &body).await?;
        tx.commit().await?;
    }
    if input.takedown.is_none()
        && input.deactivated.is_none()
        && kind == "com.atproto.admin.defs#repoRef"
    {
        // As the reference: re-sequence the account's status.
        let did = input.subject["did"].as_str().unwrap_or_default();
        if let Some(a) = any_account(&s, did).await? {
            let active = a.status == Status::Active;
            let body = account_body(did, active, (!active).then(|| a.status.as_str()))
                .map_err(internal)?;
            let mut tx = s.store.begin().await?;
            tx.insert_outbox(did, "account", &body).await?;
            tx.commit().await?;
        }
    }
    admin_auth::audit(
        &s,
        &who,
        "admin.updateSubjectStatus",
        None,
        Some(json!({"subject": input.subject, "takedown": input.takedown.as_ref().map(|t| json!({"applied": t.applied, "ref": t.reference})),
            "deactivated": input.deactivated.as_ref().map(|d| d.applied)})),
    )
    .await;
    let mut out = json!({"subject": input.subject});
    if let Some(t) = &input.takedown {
        out["takedown"] = status_attr(reference(t).as_deref());
    }
    Ok(Json(out))
}

// ---------------------------------------------------------------- email

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendEmailInput {
    recipient_did: String,
    content: String,
    subject: Option<String>,
    comment: Option<String>,
}

/// Through the moderation sender.
pub async fn send_email(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<SendEmailInput>,
) -> XrpcResult<Json<Value>> {
    let who = caller(
        &s,
        &headers,
        "com.atproto.admin.sendEmail",
        Role::Moderator,
        true,
    )
    .await?;
    let account = any_account(&s, &input.recipient_did)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("Recipient not found"))?;
    let to = account
        .email
        .as_deref()
        .ok_or_else(|| XrpcError::invalid_request("account does not have an email address"))?;
    let snap = s.snapshot();
    let from = match snap.text("email.moderation_from_address") {
        "" => match snap.text("email.from_address") {
            "" => format!("noreply@{}", s.hostname),
            f => f.to_owned(),
        },
        f => f.to_owned(),
    };
    let subject = input
        .subject
        .clone()
        .unwrap_or_else(|| "Message via your PDS".into());
    let email = locus_mail::templates::Email {
        subject: subject.clone(),
        text: input.content.clone(),
        html: input.content.clone(),
    };
    s.mailer
        .send(locus_store::email::Sender::Moderation, &from, to, email)
        .await
        .map_err(|e| match e {
            locus_mail::MailError::NotConfigured => {
                XrpcError::invalid_request("Moderation email isn't configured")
            }
            locus_mail::MailError::Address(m) => XrpcError::invalid_request(m),
            locus_mail::MailError::Send(_) => {
                XrpcError::internal("Couldn't send the email; try again later")
            }
        })?;
    admin_auth::audit(
        &s,
        &who,
        "admin.sendEmail",
        Some(&input.recipient_did),
        Some(json!({"subject": subject, "comment": input.comment})),
    )
    .await;
    Ok(Json(json!({"sent": true})))
}

// ---------------------------------------------------------------- credentials

#[derive(Deserialize)]
pub struct DeleteInput {
    did: String,
}

pub async fn delete_account(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<DeleteInput>,
) -> XrpcResult<Json<Value>> {
    let who = caller(
        &s,
        &headers,
        "com.atproto.admin.deleteAccount",
        Role::Admin,
        false,
    )
    .await?;
    if any_account(&s, &input.did).await?.is_some() {
        let job = crate::account::start_deletion(&s, &input.did).await?;
        crate::account::spawn_deletion(&s, job);
        s.store.oauth_delete_account_tokens(&input.did).await?;
    }
    admin_auth::audit(&s, &who, "admin.deleteAccount", Some(&input.did), None).await;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub struct PasswordInput {
    did: String,
    password: String,
}

pub async fn update_account_password(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<PasswordInput>,
) -> XrpcResult<Json<Value>> {
    let who = caller(
        &s,
        &headers,
        "com.atproto.admin.updateAccountPassword",
        Role::Admin,
        false,
    )
    .await?;
    if input.password.len() > password::MAX_NEW_PASSWORD {
        return Err(XrpcError::invalid_request("Invalid password length."));
    }
    any_account(&s, &input.did)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("Account not found"))?;
    let hash = password::hash(&input.password).map_err(XrpcError::internal)?;
    let mut tx = s.store.begin().await?;
    tx.set_password_hash(&input.did, &hash).await?;
    tx.revoke_sessions(&input.did, SessionFilter::All).await?;
    tx.commit().await?;
    s.store.oauth_delete_account_tokens(&input.did).await?;
    admin_auth::audit(
        &s,
        &who,
        "admin.updateAccountPassword",
        Some(&input.did),
        None,
    )
    .await;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub struct EmailInput {
    account: String,
    email: String,
}

pub async fn update_account_email(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<EmailInput>,
) -> XrpcResult<Json<Value>> {
    let who = caller(
        &s,
        &headers,
        "com.atproto.admin.updateAccountEmail",
        Role::Admin,
        false,
    )
    .await?;
    let account = if input.account.starts_with("did:") {
        any_account(&s, &input.account).await?
    } else {
        s.store
            .account_by_handle(&input.account.to_ascii_lowercase())
            .await?
    }
    .ok_or_else(|| {
        XrpcError::invalid_request(format!("Account does not exist: {}", input.account))
    })?;
    let email = input.email.trim().to_ascii_lowercase();
    crate::server::check_email(&s, &email)?;
    let mut tx = s.store.begin().await?;
    tx.set_email(&account.did, &email).await?;
    tx.commit().await?;
    admin_auth::audit(
        &s,
        &who,
        "admin.updateAccountEmail",
        Some(&account.did),
        Some(json!({"before": account.email, "after": email})),
    )
    .await;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub struct HandleInput {
    did: String,
    handle: String,
}

pub async fn update_account_handle(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<HandleInput>,
) -> XrpcResult<Json<Value>> {
    let who = caller(
        &s,
        &headers,
        "com.atproto.admin.updateAccountHandle",
        Role::Admin,
        false,
    )
    .await?;
    let account = any_account(&s, &input.did)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("Account not found"))?;
    crate::identity::change_handle(&s, &account, &input.handle).await?;
    admin_auth::audit(
        &s,
        &who,
        "admin.updateAccountHandle",
        Some(&account.did),
        Some(json!({"before": account.handle, "after": input.handle})),
    )
    .await;
    Ok(Json(json!({})))
}

// ---------------------------------------------------------------- invites

#[derive(Deserialize)]
pub struct InviteCodesParams {
    sort: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
}

pub async fn get_invite_codes(
    State(s): St,
    headers: HeaderMap,
    Query(p): Query<InviteCodesParams>,
) -> XrpcResult<Json<Value>> {
    caller(
        &s,
        &headers,
        "com.atproto.admin.getInviteCodes",
        Role::Moderator,
        true,
    )
    .await?;
    let usage = match p.sort.as_deref().unwrap_or("recent") {
        "recent" => false,
        "usage" => true,
        other => {
            return Err(XrpcError::invalid_request(format!(
                "unknown sort method: {other}"
            )));
        }
    };
    let limit = p.limit.unwrap_or(100).clamp(1, 500);
    let codes = s
        .store
        .all_invite_codes(usage, p.cursor.as_deref(), limit)
        .await?;
    let mut out = json!({"codes": codes.iter().map(code_detail).collect::<Vec<_>>()});
    if codes.len() as i64 == limit
        && let Some(last) = codes.last()
    {
        out["cursor"] = json!(last.code);
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct AccountInput {
    account: String,
    note: Option<String>,
}

async fn set_invites(
    s: &AppState,
    headers: &HeaderMap,
    input: &AccountInput,
    disabled: bool,
) -> XrpcResult<()> {
    let method = if disabled {
        "disableAccountInvites"
    } else {
        "enableAccountInvites"
    };
    let who = caller(s, headers, &format!("{NS}{method}"), Role::Moderator, true).await?;
    s.store
        .set_account_invites_disabled(&input.account, disabled)
        .await?;
    admin_auth::audit(
        s,
        &who,
        &format!("admin.{method}"),
        Some(&input.account),
        Some(json!({"note": input.note})),
    )
    .await;
    Ok(())
}

pub async fn disable_account_invites(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<AccountInput>,
) -> XrpcResult<Json<Value>> {
    set_invites(&s, &headers, &input, true).await?;
    Ok(Json(json!({})))
}

pub async fn enable_account_invites(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<AccountInput>,
) -> XrpcResult<Json<Value>> {
    set_invites(&s, &headers, &input, false).await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize, Default)]
pub struct DisableCodesInput {
    #[serde(default)]
    codes: Vec<String>,
    #[serde(default)]
    accounts: Vec<String>,
}

pub async fn disable_invite_codes(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<DisableCodesInput>,
) -> XrpcResult<Json<Value>> {
    let who = caller(
        &s,
        &headers,
        "com.atproto.admin.disableInviteCodes",
        Role::Moderator,
        true,
    )
    .await?;
    if input.accounts.iter().any(|a| a == "admin") {
        return Err(XrpcError::invalid_request(
            "cannot disable admin invite codes",
        ));
    }
    s.store
        .disable_invite_codes(&input.codes, &input.accounts)
        .await?;
    admin_auth::audit(
        &s,
        &who,
        "admin.disableInviteCodes",
        None,
        Some(json!({"codes": input.codes, "accounts": input.accounts})),
    )
    .await;
    Ok(Json(json!({})))
}

fn new_code(s: &AppState) -> String {
    format!(
        "{}-{}",
        s.hostname.replace('.', "-"),
        crate::email::new_token().to_ascii_lowercase()
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCodeInput {
    use_count: i64,
    for_account: Option<String>,
}

/// `com.atproto.server.createInviteCode`: moderators (not the moderation
/// service, which the reference keeps off it).
pub async fn create_invite_code(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<CreateCodeInput>,
) -> XrpcResult<Json<Value>> {
    let who = caller(
        &s,
        &headers,
        "com.atproto.server.createInviteCode",
        Role::Moderator,
        false,
    )
    .await?;
    let code = new_code(&s);
    let for_account = input.for_account.unwrap_or_else(|| "admin".into());
    let mut tx = s.store.begin().await?;
    tx.insert_invite_code(&code, input.use_count, &for_account, "admin")
        .await?;
    tx.commit().await?;
    admin_auth::audit(
        &s,
        &who,
        "server.createInviteCode",
        Some(&for_account),
        Some(json!({"useCount": input.use_count})),
    )
    .await;
    Ok(Json(json!({"code": code})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCodesInput {
    code_count: i64,
    use_count: i64,
    for_accounts: Option<Vec<String>>,
}

pub async fn create_invite_codes(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<CreateCodesInput>,
) -> XrpcResult<Json<Value>> {
    let who = caller(
        &s,
        &headers,
        "com.atproto.server.createInviteCodes",
        Role::Moderator,
        false,
    )
    .await?;
    let accounts = input
        .for_accounts
        .clone()
        .unwrap_or_else(|| vec!["admin".into()]);
    let mut out = Vec::new();
    let mut tx = s.store.begin().await?;
    for account in &accounts {
        let mut codes = Vec::new();
        for _ in 0..input.code_count.clamp(0, 1000) {
            let code = new_code(&s);
            tx.insert_invite_code(&code, input.use_count, account, "admin")
                .await?;
            codes.push(code);
        }
        out.push(json!({"account": account, "codes": codes}));
    }
    tx.commit().await?;
    admin_auth::audit(
        &s,
        &who,
        "server.createInviteCodes",
        None,
        Some(json!({"accounts": accounts, "codeCount": input.code_count, "useCount": input.use_count})),
    )
    .await;
    Ok(Json(json!({"codes": out})))
}

/// Whether the caller holds admin credentials for the sync reads the
/// reference opens to the admin token (taken-down or deactivated repos).
pub async fn admin_reader(s: &AppState, headers: &HeaderMap) -> bool {
    use axum::http::header;
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !(auth.starts_with("Basic ") || auth.starts_with("Bearer lop_")) {
        return false;
    }
    admin_auth::require(
        s,
        headers,
        "",
        Surface::Reference { mod_service: false },
        Role::Viewer,
    )
    .await
    .is_ok()
}
