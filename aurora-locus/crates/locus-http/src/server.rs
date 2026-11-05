//! `com.atproto.server.*`: describeServer, account creation, legacy
//! sessions and app passwords.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use locus_auth::password;
use locus_auth::session::{
    SCOPE_ACCESS, SCOPE_APP_PASS, SCOPE_APP_PASS_PRIVILEGED, SCOPE_TAKENDOWN, Tokens,
};
use locus_core::CoreError;
use locus_identity::did_doc::DidDocument;
use locus_identity::plc::{Operation, Service};
use locus_identity::plc_client::PlcClientError;
use locus_repo::event::{account_body, identity_body, sync_body};
use locus_store::accounts::{Account, AppPassword, NewAccount, SessionFilter, Status};
use proto_blue_crypto::{ExportableKeypair, K256Keypair, Keypair};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth;
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

/// Logins take at least this long, so timing doesn't reveal which accounts exist.
pub(crate) const LOGIN_FLOOR: Duration = Duration::from_millis(350);

type St = State<Arc<AppState>>;

pub async fn describe_server(State(s): St) -> Json<Value> {
    let snap = s.snapshot();
    let mut links = serde_json::Map::new();
    for (key, field) in [
        ("server.privacy_policy_url", "privacyPolicy"),
        ("server.terms_of_service_url", "termsOfService"),
    ] {
        let v = snap.text(key);
        if !v.is_empty() {
            links.insert(field.into(), v.into());
        }
    }
    let mut out = json!({
        "did": s.service_did,
        "availableUserDomains": s.handle_domains(),
        "inviteCodeRequired": snap.bool("accounts.invite_required"),
        "phoneVerificationRequired": false,
        "links": links,
    });
    let contact = snap.text("server.contact_email");
    if !contact.is_empty() {
        out["contact"] = json!({ "email": contact });
    }
    Json(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAccountInput {
    email: Option<String>,
    handle: String,
    did: Option<String>,
    invite_code: Option<String>,
    password: Option<String>,
    recovery_key: Option<String>,
    /// Locus extension, ignored by stock clients: `plc` or `web`.
    locus_did_method: Option<String>,
}

/// Disposable-address domains (the list the reference uses, CC0;
/// refreshed with `scripts/update-disposable-domains.sh`).
static DISPOSABLE_DOMAINS: std::sync::LazyLock<std::collections::HashSet<&'static str>> =
    std::sync::LazyLock::new(|| {
        include_str!("../data/disposable_email_domains.txt")
            .lines()
            .collect()
    });

/// Validates and lowercases an email address, refusing disposable domains
/// (the vendored list plus `accounts.disposable_email_domains`), as the
/// reference.
pub(crate) fn check_email(s: &AppState, raw: &str) -> XrpcResult<String> {
    let email = raw.trim().to_ascii_lowercase();
    let domain = email.rsplit('@').next().unwrap_or_default();
    let valid = email.len() <= 320
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        })
        && !email.contains(char::is_whitespace);
    let disposable = DISPOSABLE_DOMAINS.contains(domain)
        || s.snapshot()
            .list("accounts.disposable_email_domains")
            .iter()
            .any(|d| d.eq_ignore_ascii_case(domain));
    if valid && !disposable {
        Ok(email)
    } else {
        Err(XrpcError::invalid_request(
            "This email address is not supported, please use a different email.",
        ))
    }
}

fn status_fields(status: Status) -> (bool, Option<&'static str>) {
    match status {
        Status::Active => (true, None),
        other => (false, Some(other.as_str())),
    }
}

async fn did_doc_for(s: &AppState, account: &Account) -> XrpcResult<Option<Value>> {
    if !s.snapshot().bool("accounts.did_doc_with_session") {
        return Ok(None);
    }
    let Some((_, sk)) = s.store.get_signing_key(&account.did, &s.master_key).await? else {
        return Ok(None);
    };
    let key = K256Keypair::from_private_key(&sk).map_err(|e| XrpcError::internal(e.to_string()))?;
    Ok(Some(
        DidDocument::account(&account.did, &account.handle, &key.did(), &s.public_url).to_json(),
    ))
}

fn session_json(account: &Account, tokens: &Tokens, did_doc: Option<Value>) -> Value {
    let (active, status) = status_fields(account.status);
    let mut out = json!({
        "accessJwt": tokens.access_jwt,
        "refreshJwt": tokens.refresh_jwt,
        "handle": account.handle,
        "did": account.did,
        "email": account.email,
        "emailConfirmed": account.email_confirmed_at.is_some(),
        "emailAuthFactor": account.email_auth_factor_at.is_some(),
        "active": active,
    });
    if let Some(s) = status {
        out["status"] = s.into();
    }
    if let Some(d) = did_doc {
        out["didDoc"] = d;
    }
    out
}

pub async fn create_account(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<CreateAccountInput>,
) -> XrpcResult<Json<Value>> {
    crate::ratelimit::limit_ip(&s, &[crate::ratelimit::buckets::CREATE_ACCOUNT]).await?;
    // Migrating in: the DID must authorize this with a service JWT, and the
    // account starts deactivated, emitting nothing until it's activated.
    let migrating = match input.did.as_deref() {
        Some(did) => {
            let requester = if headers.contains_key(axum::http::header::AUTHORIZATION) {
                Some(
                    auth::user_service_auth(&s, &headers, "com.atproto.server.createAccount")
                        .await?,
                )
            } else {
                None
            };
            if requester.as_deref() != Some(did) {
                return Err(XrpcError::auth_required(format!(
                    "Missing auth to create account with did: {did}"
                )));
            }
            if s.store.account_by_did(did).await?.is_some() {
                return Err(XrpcError::invalid_request("Account already exists"));
            }
            Some(did.to_owned())
        }
        None => None,
    };
    let email = check_email(
        &s,
        input
            .email
            .as_deref()
            .ok_or_else(|| XrpcError::invalid_request("Email is required"))?,
    )?;
    let handle = crate::identity::validate_handle(&s, &input.handle, migrating.as_deref()).await?;
    let password_hash = match &input.password {
        Some(p) if p.len() > password::MAX_NEW_PASSWORD => {
            return Err(XrpcError::invalid_request("Password too long"));
        }
        Some(p) => Some(password::hash(p).map_err(XrpcError::internal)?),
        None => None,
    };
    let snap = s.snapshot();
    let invite = input.invite_code.clone();
    if snap.bool("accounts.invite_required") && invite.is_none() {
        return Err(XrpcError::named(
            "InvalidInviteCode",
            "No invite code provided",
        ));
    }
    // Fail fast on taken names; the unique indexes are the real guard.
    if s.store.account_by_handle(&handle).await?.is_some() {
        return Err(XrpcError::named(
            "HandleNotAvailable",
            "Handle already taken",
        ));
    }
    if s.store.account_by_email(&email).await?.is_some() {
        return Err(XrpcError::invalid_request("Email already taken"));
    }

    let method = match &migrating {
        Some(did) if did.starts_with("did:plc:") => "plc".to_owned(),
        Some(did) if did.starts_with("did:web:") => "web".to_owned(),
        Some(_) => return Err(XrpcError::invalid_request("Unsupported DID method")),
        None => input
            .locus_did_method
            .as_deref()
            .unwrap_or_else(|| snap.text("accounts.default_did_method"))
            .to_owned(),
    };
    let signing = K256Keypair::generate();
    let (did, web_doc, rotation_keys) = match method.as_str() {
        _ if migrating.is_some() => (migrating.clone().unwrap_or_default(), None, Vec::new()),
        "plc" => {
            let (did, keys) =
                register_plc_did(&s, &handle, &signing, input.recovery_key.as_deref()).await?;
            (did, None, keys)
        }
        "web" => {
            // did:web:<handle host>, served by locus at that host.
            let did = format!("did:web:{handle}");
            if s.store.account_by_did(&did).await?.is_some() {
                return Err(XrpcError::named(
                    "HandleNotAvailable",
                    "Handle already taken",
                ));
            }
            let doc = DidDocument::account(&did, &handle, &signing.did(), &s.public_url).to_json();
            (did, Some(doc.to_string()), Vec::new())
        }
        _ => {
            return Err(XrpcError::invalid_request(
                "locusDidMethod must be plc or web",
            ));
        }
    };

    // One transaction writes the account and everything that goes with it.
    let result: XrpcResult<(Account, Tokens)> = async {
        let mut tx = s.store.begin().await?;
        tx.insert_account(&NewAccount {
            did: &did,
            handle: &handle,
            email: Some(&email),
            password_hash: password_hash.as_deref(),
            status: Status::Active,
            did_method: &method,
        })
        .await
        .map_err(|e| match e {
            CoreError::Conflict(m) if m.starts_with("Handle") => {
                XrpcError::named("HandleNotAvailable", m)
            }
            other => other.into(),
        })?;
        if migrating.is_some() {
            tx.deactivate_account(&did, None).await?;
        }
        if let Some(doc) = &web_doc {
            tx.insert_hosted_did_web(&did, &handle, doc)
                .await
                .map_err(|e| match e {
                    CoreError::Conflict(m) => XrpcError::named("HandleNotAvailable", m),
                    other => other.into(),
                })?;
        }
        tx.put_signing_key(&did, "k256", &signing.export_private_key(), &s.master_key)
            .await?;
        if let Some(code) = &invite {
            tx.use_invite_code(code, &did).await.map_err(|e| match e {
                CoreError::Invalid(m) => XrpcError::named("InvalidInviteCode", m),
                other => other.into(),
            })?;
        }
        // Events in the reference's order: #identity, #account, #commit, #sync,
        // only for an account created active.
        let emit = migrating.is_none();
        let enc = |e: proto_blue_repo::error::RepoError| XrpcError::internal(e.to_string());
        if emit {
            tx.insert_outbox(
                &did,
                "identity",
                &identity_body(&did, Some(&handle)).map_err(enc)?,
            )
            .await?;
            tx.insert_outbox(
                &did,
                "account",
                &account_body(&did, true, None).map_err(enc)?,
            )
            .await?;
        }
        let repo = s
            .repo
            .create_repo_in(&mut tx, &did, &signing, emit)
            .await
            .map_err(|e| XrpcError::internal(e.to_string()))?;
        if emit {
            tx.insert_outbox(
                &did,
                "sync",
                &sync_body(&did, &repo.rev, &repo.commit_cid, &repo.commit_bytes).map_err(enc)?,
            )
            .await?;
        }
        let tokens = s
            .sessions
            .create_in(&mut tx, &did, SCOPE_ACCESS, None, true)
            .await?;
        tx.commit().await?;
        let account = s
            .store
            .account_by_did(&did)
            .await?
            .ok_or_else(|| XrpcError::internal("account vanished after creation"))?;
        Ok((account, tokens))
    }
    .await;

    let (account, tokens) = match result {
        Ok(v) => v,
        Err(e) => {
            if method == "plc" && migrating.is_none() {
                // Locus created this DID; tombstone it so it doesn't dangle.
                tombstone_or_queue(&s, &did).await;
            }
            return Err(e);
        }
    };
    if !rotation_keys.is_empty() {
        let _ = s
            .store
            .set_plc_key_listing(&did, &s.secrets.plc_keys.listed(&rotation_keys))
            .await;
    }
    let did_doc = did_doc_for(&s, &account).await?;
    let mut out = json!({
        "handle": account.handle,
        "did": account.did,
        "accessJwt": tokens.access_jwt,
        "refreshJwt": tokens.refresh_jwt,
    });
    if let Some(d) = did_doc {
        out["didDoc"] = d;
    }
    Ok(Json(out))
}

/// Builds, signs and registers a new account's genesis operation. Returns
/// the DID and its rotation keys. No transaction is open meanwhile.
async fn register_plc_did(
    s: &AppState,
    handle: &str,
    signing: &K256Keypair,
    recovery_key: Option<&str>,
) -> XrpcResult<(String, Vec<String>)> {
    let snap = s.snapshot();
    let mut rotation_keys = Vec::new();
    if let Some(k) = recovery_key.filter(|k| !k.is_empty()) {
        rotation_keys.push(k.to_owned());
    }
    let operator_recovery = snap.text("identity.recovery_did_key");
    if !operator_recovery.is_empty() {
        rotation_keys.push(operator_recovery.to_owned());
    }
    rotation_keys.push(s.secrets.plc_keys.current().did());
    let op = Operation {
        rotation_keys: rotation_keys.clone(),
        verification_methods: BTreeMap::from([("atproto".to_owned(), signing.did())]),
        also_known_as: vec![format!("at://{handle}")],
        services: BTreeMap::from([(
            "atproto_pds".to_owned(),
            Service {
                kind: "AtprotoPersonalDataServer".into(),
                endpoint: s.public_url.clone(),
            },
        )]),
        prev: None,
    };
    op.validate()
        .map_err(|e| XrpcError::invalid_request(e.to_string()))?;
    let genesis = op
        .sign(s.secrets.plc_keys.current())
        .map_err(|e| XrpcError::internal(e.to_string()))?;
    let did = genesis
        .genesis_did()
        .map_err(|e| XrpcError::internal(e.to_string()))?;
    let plc = s.plc()?;
    match plc.submit(&did, &genesis).await {
        Ok(()) => {}
        Err(PlcClientError::Refused(m)) => {
            return Err(XrpcError::invalid_request(format!(
                "The PLC directory refused the DID: {m}"
            )));
        }
        Err(e) => {
            // Ambiguous: it may have been applied. Check before giving up.
            match plc.document(&did).await {
                Ok(Some(_)) => {}
                _ => {
                    return Err(XrpcError::new(
                        StatusCode::BAD_GATEWAY,
                        "UpstreamFailure",
                        e.to_string(),
                    ));
                }
            }
        }
    }
    Ok((did, rotation_keys))
}

/// Tombstones a DID locus created for a failed account creation, or leaves
/// it to the identity repair job when PLC can't be reached.
async fn tombstone_or_queue(s: &AppState, did: &str) {
    match crate::identity::plc_tombstone(s, did).await {
        Ok(()) => {}
        Err(crate::identity::PlcStep::Refused(e)) => {
            tracing::warn!(
                did,
                "PLC refused the tombstone of a failed account creation: {}",
                e.message
            );
        }
        Err(crate::identity::PlcStep::Ambiguous(_)) => {
            if let Err(e) = s
                .store
                .queue_identity_repair(did, locus_store::identity::RepairKind::Tombstone, did)
                .await
            {
                tracing::error!(
                    did,
                    "couldn't queue the tombstone of a failed account creation: {e}"
                );
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionInput {
    identifier: String,
    password: String,
    #[serde(default)]
    allow_takendown: bool,
    auth_factor_token: Option<String>,
}

pub(crate) async fn find_account(s: &AppState, identifier: &str) -> XrpcResult<Option<Account>> {
    let id = identifier.trim();
    Ok(if id.starts_with("did:") {
        s.store.account_by_did(id).await?
    } else if id.contains('@') {
        s.store.account_by_email(id).await?
    } else {
        s.store.account_by_handle(id).await?
    })
}

/// How a login's password matched.
pub(crate) enum Credential {
    Account,
    AppPassword { name: String, privileged: bool },
}

pub(crate) async fn check_password(
    s: &AppState,
    account: &Account,
    pw: &str,
) -> XrpcResult<Option<Credential>> {
    if let Some(hash) = &account.password_hash
        && password::verify(pw, hash)
    {
        return Ok(Some(Credential::Account));
    }
    if password::looks_like_app_password(pw) {
        let lookup =
            password::app_password_lookup(&s.secrets.app_password_pepper, &account.did, pw);
        if let Some(ap) = s
            .store
            .app_password_by_lookup(&account.did, &lookup)
            .await?
            && password::verify(pw, &ap.hash)
        {
            return Ok(Some(Credential::AppPassword {
                name: ap.name,
                privileged: ap.privileged,
            }));
        }
    }
    Ok(None)
}

pub async fn create_session(
    State(s): St,
    Json(input): Json<CreateSessionInput>,
) -> XrpcResult<Json<Value>> {
    {
        // Keyed by identifier and IP, as the reference.
        use crate::ratelimit::buckets::{CREATE_SESSION_5M, CREATE_SESSION_DAY};
        let ip =
            crate::ratelimit::client_ip().map_or_else(|| "unknown".to_owned(), |i| i.to_string());
        let key = format!("{}-{ip}", input.identifier);
        crate::ratelimit::limit(
            &s,
            &[(CREATE_SESSION_DAY, &key, 1), (CREATE_SESSION_5M, &key, 1)],
        )
        .await?;
    }
    let started = tokio::time::Instant::now();
    let result = create_session_inner(&s, &input).await;
    tokio::time::sleep_until(started + LOGIN_FLOOR).await;
    result
}

async fn create_session_inner(s: &AppState, input: &CreateSessionInput) -> XrpcResult<Json<Value>> {
    let invalid = || XrpcError::auth_required("Invalid identifier or password");
    if input.password.len() > password::MAX_LOGIN_PASSWORD {
        return Err(invalid());
    }
    let Some(account) = find_account(s, &input.identifier).await? else {
        return Err(invalid());
    };
    if account.status == Status::Deleted {
        return Err(invalid());
    }
    let Some(cred) = check_password(s, &account, &input.password).await? else {
        return Err(invalid());
    };
    crate::email::sign_in_factor(
        s,
        &account,
        matches!(cred, Credential::AppPassword { .. }),
        input.auth_factor_token.as_deref(),
    )
    .await?;
    let taken_down = matches!(account.status, Status::Takendown | Status::Suspended);
    let (scope, app_password, store_refresh) = match (&cred, taken_down) {
        (Credential::AppPassword { .. }, true) => {
            return Err(XrpcError::new(
                StatusCode::UNAUTHORIZED,
                "AccountTakedown",
                "Account has been taken down",
            ));
        }
        (Credential::Account, true) if !input.allow_takendown => {
            return Err(XrpcError::new(
                StatusCode::UNAUTHORIZED,
                "AccountTakedown",
                "Account has been taken down",
            ));
        }
        (Credential::Account, true) => (SCOPE_TAKENDOWN, None, false),
        (Credential::Account, false) => (SCOPE_ACCESS, None, true),
        (Credential::AppPassword { name, privileged }, false) => (
            if *privileged {
                SCOPE_APP_PASS_PRIVILEGED
            } else {
                SCOPE_APP_PASS
            },
            Some(name.as_str()),
            true,
        ),
    };
    let tokens = s
        .sessions
        .create(&account.did, scope, app_password, store_refresh)
        .await?;
    let did_doc = did_doc_for(s, &account).await?;
    Ok(Json(session_json(&account, &tokens, did_doc)))
}

pub async fn get_session(State(s): St, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    let authed = auth::authorization(
        &s,
        &headers,
        auth::STANDARD,
        &[locus_auth::session::SCOPE_SIGNUP_QUEUED],
        auth::any_scope,
    )
    .await?;
    let account = s.store.account_by_did(&authed.did).await?.ok_or_else(|| {
        XrpcError::new(
            StatusCode::FORBIDDEN,
            "AccountNotFound",
            "Account not found",
        )
    })?;
    let (active, status) = status_fields(account.status);
    let mut out = json!({
        "handle": account.handle,
        "did": account.did,
        "email": account.email,
        "emailConfirmed": account.email_confirmed_at.is_some(),
        "emailAuthFactor": account.email_auth_factor_at.is_some(),
        "active": active,
    });
    if let Some(st) = status {
        out["status"] = st.into();
    }
    if let Some(d) = did_doc_for(&s, &account).await? {
        out["didDoc"] = d;
    }
    // OAuth tokens see the email only with account:email.
    if let Some(g) = &authed.oauth
        && !g.permissions().allows_account("email", "read")
        && let Some(m) = out.as_object_mut()
    {
        m.remove("email");
        m.remove("emailConfirmed");
        m.remove("emailAuthFactor");
    }
    Ok(Json(out))
}

pub async fn refresh_session(State(s): St, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    let token = auth::bearer(&headers)?;
    let (tokens, did) = s.sessions.refresh(token).await?;
    let account = s.store.account_by_did(&did).await?.ok_or_else(|| {
        XrpcError::new(
            StatusCode::FORBIDDEN,
            "AccountNotFound",
            "Account not found",
        )
    })?;
    let did_doc = did_doc_for(&s, &account).await?;
    let mut out = session_json(&account, &tokens, did_doc);
    if let Some(m) = out.as_object_mut() {
        m.remove("email");
        m.remove("emailConfirmed");
        m.remove("emailAuthFactor");
    }
    Ok(Json(out))
}

pub async fn delete_session(State(s): St, headers: HeaderMap) -> XrpcResult<()> {
    let token = auth::bearer(&headers)?;
    s.sessions.delete(token).await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct CreateAppPasswordInput {
    name: String,
    #[serde(default)]
    privileged: bool,
}

pub async fn create_app_password(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<CreateAppPasswordInput>,
) -> XrpcResult<Json<Value>> {
    let authed = auth::authorization(
        &s,
        &headers,
        auth::FULL,
        &[],
        auth::forbid(auth::OAUTH_UNSUPPORTED),
    )
    .await?;
    let name = input.name.trim();
    if name.is_empty() || name.len() > 32 {
        return Err(XrpcError::invalid_request("Invalid app password name"));
    }
    let pw = password::new_app_password();
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    let row = AppPassword {
        did: authed.did.clone(),
        name: name.to_owned(),
        lookup_hash: password::app_password_lookup(
            &s.secrets.app_password_pepper,
            &authed.did,
            &pw,
        ),
        hash: password::hash(&pw).map_err(XrpcError::internal)?,
        privileged: input.privileged,
        created_at,
    };
    let mut tx = s.store.begin().await?;
    tx.insert_app_password(&row).await.map_err(|e| match e {
        CoreError::Conflict(m) => XrpcError::named("DuplicateName", m),
        other => other.into(),
    })?;
    tx.commit().await?;
    Ok(Json(json!({
        "name": row.name,
        "password": pw,
        "createdAt": iso8601(created_at),
        "privileged": row.privileged,
    })))
}

pub async fn list_app_passwords(State(s): St, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    let authed = auth::authorization(
        &s,
        &headers,
        auth::STANDARD,
        &[],
        auth::forbid(auth::OAUTH_UNSUPPORTED),
    )
    .await?;
    let list: Vec<Value> = s
        .store
        .app_passwords(&authed.did)
        .await?
        .into_iter()
        .map(|p| json!({"name": p.name, "createdAt": iso8601(p.created_at), "privileged": p.privileged}))
        .collect();
    Ok(Json(json!({ "passwords": list })))
}

#[derive(Deserialize)]
pub struct RevokeAppPasswordInput {
    name: String,
}

pub async fn revoke_app_password(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<RevokeAppPasswordInput>,
) -> XrpcResult<()> {
    let authed = auth::authorization(
        &s,
        &headers,
        auth::STANDARD,
        &[],
        auth::forbid(auth::OAUTH_UNSUPPORTED),
    )
    .await?;
    let mut tx = s.store.begin().await?;
    tx.delete_app_password(&authed.did, &input.name).await?;
    tx.revoke_sessions(&authed.did, SessionFilter::AppPassword(&input.name))
        .await?;
    tx.commit().await?;
    Ok(())
}

use locus_core::time::iso8601;

use crate::pipethrough::{is_privileged_method, is_protected_method};

#[derive(Deserialize)]
pub struct GetServiceAuthParams {
    aud: String,
    exp: Option<i64>,
    lxm: Option<String>,
}

pub async fn get_service_auth(
    State(s): St,
    headers: HeaderMap,
    axum::extract::Query(p): axum::extract::Query<GetServiceAuthParams>,
) -> XrpcResult<Json<Value>> {
    let authed = auth::authorization(&s, &headers, auth::STANDARD, &[SCOPE_TAKENDOWN], |perm| {
        Ok(perm.assert_rpc(p.lxm.as_deref().unwrap_or("*"), &p.aud)?)
    })
    .await?;
    let lxm = p.lxm.as_deref().filter(|l| !l.is_empty() && *l != "*");
    let (aud_did, fragment) = match p.aud.split_once('#') {
        Some((d, f)) => (d, Some(f)),
        None => (p.aud.as_str(), None),
    };
    if proto_blue_syntax::Did::new(aud_did).is_err() || fragment.is_some_and(str::is_empty) {
        return Err(XrpcError::invalid_request(
            "aud must be a valid atproto DID or did#serviceId reference",
        ));
    }
    // A taken-down account can only get the token that moves it away.
    if authed.scope == SCOPE_TAKENDOWN && lxm != Some("com.atproto.server.createAccount") {
        return Err(XrpcError::named("InvalidToken", "Bad token scope"));
    }
    if let Some(m) = lxm
        && is_privileged_method(m)
        && !auth::PRIVILEGED.contains(&authed.scope.as_str())
    {
        return Err(XrpcError::invalid_request(format!(
            "insufficient access to request a service auth token for the following method: {m}"
        )));
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    if let Some(exp) = p.exp {
        let diff = exp.saturating_mul(1000) - now_ms;
        if diff < 0 {
            return Err(XrpcError::named("BadExpiration", "expiration is in past"));
        } else if diff > 60 * 60 * 1000 {
            return Err(XrpcError::named(
                "BadExpiration",
                "cannot request a token with an expiration more than an hour in the future",
            ));
        } else if lxm.is_none() && diff > 60 * 1000 {
            return Err(XrpcError::named(
                "BadExpiration",
                "cannot request a method-less token with an expiration more than a minute in the future",
            ));
        }
    }
    if let Some(m) = lxm
        && is_protected_method(m)
    {
        return Err(XrpcError::invalid_request(format!(
            "cannot request a service auth token for the following protected method: {m}"
        )));
    }
    let key = s.signing_key(&authed.did).await?;
    let token =
        locus_auth::service_jwt::create(&key, &authed.did, &p.aud, p.exp, lxm, now_ms / 1000)
            .map_err(|e| XrpcError::internal(e.to_string()))?;
    Ok(Json(json!({ "token": token })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetAccountInviteCodesParams {
    #[serde(default = "yes")]
    include_used: bool,
    #[serde(default = "yes")]
    create_available: bool,
}

fn yes() -> bool {
    true
}

/// How many routine codes an account may make now, as the reference's
/// `calculateCodesToCreate`: one per interval of its lifetime (counted from
/// the epoch), at most 5 unused at a time; admin-gifted codes don't count.
fn codes_to_create(
    codes: &[locus_store::invites::InviteCode],
    created_at: i64,
    now: i64,
    epoch: i64,
    interval: i64,
) -> (i64, i64) {
    let routine: Vec<_> = codes.iter().filter(|c| c.created_by != "admin").collect();
    let unused = routine
        .iter()
        .filter(|c| !c.disabled && c.available > i64::try_from(c.uses.len()).unwrap_or(i64::MAX))
        .count();
    let lifespan = now - created_at;
    let could_create = if created_at >= epoch {
        lifespan / interval
    } else {
        lifespan / interval - (epoch - created_at) / interval
    };
    let epoch_codes = routine.iter().filter(|c| c.created_at > epoch).count();
    let to_create = (5 - i64::try_from(unused).unwrap_or(5))
        .min(could_create - i64::try_from(epoch_codes).unwrap_or(0));
    (
        to_create,
        i64::try_from(routine.len()).unwrap_or(0) + to_create,
    )
}

pub async fn get_account_invite_codes(
    State(s): St,
    headers: HeaderMap,
    axum::extract::Query(p): axum::extract::Query<GetAccountInviteCodesParams>,
) -> XrpcResult<Json<Value>> {
    let authed = auth::authorization(
        &s,
        &headers,
        auth::FULL,
        &[],
        auth::forbid(auth::OAUTH_UNSUPPORTED),
    )
    .await?;
    let account = auth::account_checked(&s, &authed.did, false).await?;
    let did = account.did.as_str();
    let snap = s.snapshot();
    let interval = snap.int("accounts.invite_interval_ms");
    if p.create_available && snap.bool("accounts.invite_required") && interval > 0 {
        let codes = s.store.invite_codes_for(did).await?;
        let now = s.store.now_ms().await?;
        let (to_create, total) = codes_to_create(
            &codes,
            account.created_at,
            now,
            snap.int("accounts.invite_epoch_ms"),
            interval,
        );
        if to_create > 0 {
            let mut tx = s.store.begin().await?;
            // Another request may have topped the account up meanwhile.
            if tx.routine_invite_count(did).await? == total - to_create {
                for _ in 0..to_create {
                    let code = format!(
                        "{}-{}",
                        s.hostname.replace('.', "-"),
                        crate::email::new_token().to_ascii_lowercase()
                    );
                    tx.insert_invite_code(&code, 1, did, did).await?;
                    if account.invites_disabled {
                        tx.set_invite_disabled(&code, true).await?;
                    }
                }
            }
            tx.commit().await?;
        }
    }
    let codes: Vec<Value> = s
        .store
        .invite_codes_for(did)
        .await?
        .into_iter()
        .filter(|c| !c.disabled && (p.include_used || c.available > i64::try_from(c.uses.len()).unwrap_or(0)))
        .map(|c| {
            json!({
                "code": c.code,
                "available": c.available,
                "disabled": c.disabled,
                "forAccount": c.for_account,
                "createdBy": c.created_by,
                "createdAt": iso8601(c.created_at),
                "uses": c.uses.iter().map(|(by, at)| json!({"usedBy": by, "usedAt": iso8601(*at)})).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(Json(json!({ "codes": codes })))
}

/// As the reference: no signup queue here, so everyone is activated.
pub async fn check_signup_queue(State(s): St, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    auth::authorization(
        &s,
        &headers,
        auth::STANDARD,
        &[locus_auth::session::SCOPE_SIGNUP_QUEUED],
        auth::forbid(auth::OAUTH_UNSUPPORTED),
    )
    .await?;
    Ok(Json(json!({ "activated": true })))
}

#[cfg(test)]
mod invite_tests {
    use super::codes_to_create;
    use locus_store::invites::InviteCode;

    fn code(created_by: &str, created_at: i64, used: bool) -> InviteCode {
        InviteCode {
            code: "c".into(),
            available: 1,
            disabled: false,
            for_account: "did:plc:a".into(),
            created_by: created_by.into(),
            created_at,
            uses: if used {
                vec![("did:plc:b".into(), created_at)]
            } else {
                vec![]
            },
        }
    }

    #[test]
    fn invite_math_matches_the_reference() {
        const DAY: i64 = 86_400_000;
        // Ten days old, one code per day, none yet: capped at 5 unused.
        assert_eq!(codes_to_create(&[], 0, 10 * DAY, 0, DAY), (5, 5));
        // Three used and two unused routine codes: three more.
        let mut codes: Vec<_> = (1..=3).map(|d| code("did:plc:a", d * DAY, true)).collect();
        codes.extend((4..=5).map(|d| code("did:plc:a", d * DAY, false)));
        assert_eq!(codes_to_create(&codes, 0, 10 * DAY, 0, DAY), (3, 8));
        // Admin gifts don't count.
        codes.push(code("admin", DAY, false));
        assert_eq!(codes_to_create(&codes, 0, 10 * DAY, 0, DAY), (3, 8));
        // An epoch after creation only counts intervals since then.
        assert_eq!(codes_to_create(&[], 0, 10 * DAY, 8 * DAY, DAY), (2, 2));
        // Two days old: two codes at most.
        assert_eq!(codes_to_create(&[], 8 * DAY, 10 * DAY, 0, DAY), (2, 2));
    }
}
