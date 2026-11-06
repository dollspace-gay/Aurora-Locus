//! The entryway role: the front door owning accounts, logins, handles and
//! the PLC rotation key for many shards, speaking the reference's shard
//! contract (and locus's own callbacks to locus shards).
//!
//! Accounts live on shards: the entryway reserves a signing key there,
//! signs the genesis operation with its PLC rotation key and has the shard
//! create the account. Its tokens are ES256K with its token key, access
//! tokens addressed to the account's shard. Status and handle changes go
//! to the shard with its admin password (the reference callbacks). Other
//! XRPC requests are forwarded to the account's shard.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use locus_auth::password;
use locus_auth::session::{Authed, SCOPE_ACCESS};
use locus_core::{CoreError, locus_nsid};
use locus_identity::plc::{Operation, Service};
use locus_store::accounts::{NewAccount, Status};
use locus_store::entryway::{Shard, admin_password_secret, callback_token_secret};
use proto_blue_crypto::{K256Keypair, Keypair};
use serde::Deserialize;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::admin_auth::{self, Role, Surface};
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

type St = State<Arc<AppState>>;

/// The secret holding the entryway's token key (ES256K).
pub const TOKEN_KEY_SECRET: &str = "entryway_token_key";

fn upstream(m: impl Into<String>) -> XrpcError {
    XrpcError::new(StatusCode::BAD_GATEWAY, "UpstreamFailure", m)
}

fn entryway_only(s: &AppState) -> XrpcResult<()> {
    if s.role == locus_core::bootstrap::Role::Entryway {
        Ok(())
    } else {
        Err(XrpcError::new(
            StatusCode::NOT_IMPLEMENTED,
            "MethodNotImplemented",
            "Only an entryway takes this call",
        ))
    }
}

/// The entryway's token key, made on first boot.
pub async fn load_or_create_token_key(
    store: &locus_store::Store,
    key: &locus_core::keys::MasterKey,
) -> locus_core::CoreResult<K256Keypair> {
    use proto_blue_crypto::ExportableKeypair;
    if store.get_secret(TOKEN_KEY_SECRET, key).await?.is_none() {
        let fresh = K256Keypair::generate();
        let mut tx = store.begin().await?;
        tx.put_secret_if_absent(TOKEN_KEY_SECRET, &fresh.export_private_key(), key)
            .await?;
        tx.commit().await?;
    }
    let bytes = store
        .get_secret(TOKEN_KEY_SECRET, key)
        .await?
        .ok_or_else(|| CoreError::Storage("the entryway token key vanished".into()))?;
    K256Keypair::from_private_key(&bytes).map_err(|e| CoreError::Config(e.to_string()))
}

/// The session signer for the entryway role: tokens signed with `key`.
pub fn session_keys(key: K256Keypair) -> locus_core::CoreResult<locus_auth::session::EntrywayKeys> {
    let verifier = crate::shard::did_key_verifier(&key.did()).map_err(CoreError::Config)?;
    Ok(locus_auth::session::EntrywayKeys {
        signing: key,
        accepted: vec![verifier],
    })
}

/// The entryway's token key, from the key store.
pub async fn token_key(s: &AppState) -> XrpcResult<K256Keypair> {
    let bytes = s
        .store
        .get_secret(TOKEN_KEY_SECRET, &s.master_key)
        .await?
        .ok_or_else(|| XrpcError::internal("the entryway token key is missing"))?;
    K256Keypair::from_private_key(&bytes).map_err(|e| XrpcError::internal(e.to_string()))
}

// ---------------------------------------------------------------- calls to shards

/// How a call to a shard authenticates.
pub(crate) enum ShardAuth {
    None,
    /// The shard's admin password (the reference's callbacks).
    Admin,
}

/// Calls an XRPC method on a shard; its error comes back as the caller's.
pub(crate) async fn shard_call(
    s: &AppState,
    shard: &Shard,
    get_method: bool,
    nsid: &str,
    query: &[(&str, &str)],
    body: Option<&Value>,
    auth: ShardAuth,
) -> XrpcResult<Value> {
    let client = crate::pipethrough::client(s, false)?;
    let url = format!("{}/xrpc/{nsid}", shard.url);
    let mut req = if get_method {
        client.get(url).query(query)
    } else {
        client
            .post(url)
            .query(query)
            .json(body.unwrap_or(&json!({})))
    };
    req = req.timeout(Duration::from_secs(30));
    if matches!(auth, ShardAuth::Admin) {
        let name = admin_password_secret(&shard.name);
        let v = s
            .store
            .get_secret(&name, &s.master_key)
            .await?
            .ok_or_else(|| XrpcError::internal(format!("no {name} in the key store")))?;
        req = req.basic_auth("admin", Some(String::from_utf8_lossy(&v).into_owned()));
    }
    let res = req.send().await.map_err(|e| {
        tracing::warn!(shard = %shard.name, nsid, "shard request failed: {e}");
        upstream(format!("The shard {} couldn't be reached", shard.name))
    })?;
    let status = res.status();
    let v: Value = res.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let code = if status.as_u16() >= 500 {
            StatusCode::BAD_GATEWAY
        } else {
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY)
        };
        return Err(XrpcError::new(
            code,
            v.get("error")
                .and_then(Value::as_str)
                .unwrap_or("UpstreamFailure"),
            v.get("message")
                .and_then(Value::as_str)
                .unwrap_or("The shard refused the call")
                .to_owned(),
        ));
    }
    Ok(v)
}

fn repo_ref(did: &str) -> Value {
    json!({"$type": "com.atproto.admin.defs#repoRef", "did": did})
}

/// Tells the account's shard its status (`admin.updateSubjectStatus`, as
/// the reference entryway does): taken down, deactivated or active.
pub async fn push_status(s: &AppState, did: &str) -> XrpcResult<()> {
    if s.role != locus_core::bootstrap::Role::Entryway {
        return Ok(());
    }
    let (Some(account), Some(shard)) = (
        s.store.account_by_did(did).await?,
        s.store.account_shard(did).await?,
    ) else {
        return Ok(());
    };
    let body = match account.status {
        Status::Takendown | Status::Suspended => json!({
            "subject": repo_ref(did),
            "takedown": {"applied": true, "ref": account.status_ref.clone().unwrap_or_default()},
        }),
        Status::Deactivated => json!({
            "subject": repo_ref(did),
            "takedown": {"applied": false},
            "deactivated": {"applied": true},
        }),
        Status::Active => json!({
            "subject": repo_ref(did),
            "takedown": {"applied": false},
            "deactivated": {"applied": false},
        }),
        Status::Deleted => return Ok(()),
    };
    shard_call(
        s,
        &shard,
        false,
        "com.atproto.admin.updateSubjectStatus",
        &[],
        Some(&body),
        ShardAuth::Admin,
    )
    .await
    .map(|_| ())
}

/// Tells the account's shard its new handle (`admin.updateAccountHandle`;
/// the shard checks the document names it).
pub async fn push_handle(s: &AppState, did: &str, handle: &str) -> XrpcResult<()> {
    if s.role != locus_core::bootstrap::Role::Entryway {
        return Ok(());
    }
    let Some(shard) = s.store.account_shard(did).await? else {
        return Ok(());
    };
    shard_call(
        s,
        &shard,
        false,
        "com.atproto.admin.updateAccountHandle",
        &[],
        Some(&json!({"did": did, "handle": handle})),
        ShardAuth::Admin,
    )
    .await
    .map(|_| ())
}

/// Has the account's shard delete it (`admin.deleteAccount`).
pub async fn push_delete(s: &AppState, did: &str) -> XrpcResult<()> {
    if s.role != locus_core::bootstrap::Role::Entryway {
        return Ok(());
    }
    let Some(shard) = s.store.account_shard(did).await? else {
        return Ok(());
    };
    shard_call(
        s,
        &shard,
        false,
        "com.atproto.admin.deleteAccount",
        &[],
        Some(&json!({"did": did})),
        ShardAuth::Admin,
    )
    .await
    .map(|_| ())
}

// ---------------------------------------------------------------- accounts

/// `createAccount` on the entryway: the account goes to a shard (the one
/// with the most room), which holds its repo and signing key; the entryway
/// keeps the email, password, invite and sessions.
pub async fn create_account(
    s: &AppState,
    input: crate::server::CreateAccountInput,
) -> XrpcResult<Value> {
    if input.did.is_some() {
        return Err(XrpcError::invalid_request(
            "Migrating an account in goes to a shard: create it there with the shard's createAccount",
        ));
    }
    let email = crate::server::check_email(
        s,
        input
            .email
            .as_deref()
            .ok_or_else(|| XrpcError::invalid_request("Email is required"))?,
    )?;
    let handle = crate::identity::validate_handle(s, &input.handle, None).await?;
    let password_hash = match &input.password {
        Some(p) if p.len() > password::MAX_NEW_PASSWORD => {
            return Err(XrpcError::invalid_request("Password too long"));
        }
        Some(p) => Some(password::hash(p).map_err(XrpcError::internal)?),
        None => None,
    };
    let snap = s.snapshot();
    if snap.bool("accounts.invite_required") && input.invite_code.is_none() {
        return Err(XrpcError::named(
            "InvalidInviteCode",
            "No invite code provided",
        ));
    }
    if s.store.account_by_handle(&handle).await?.is_some() {
        return Err(XrpcError::named(
            "HandleNotAvailable",
            "Handle already taken",
        ));
    }
    if s.store.account_by_email(&email).await?.is_some() {
        return Err(XrpcError::invalid_request("Email already taken"));
    }
    let shard = s
        .store
        .placement_shard()
        .await?
        .ok_or_else(|| XrpcError::invalid_request("No shard is taking new accounts"))?;

    // The shard's reserved key signs the repo; the entryway's PLC key
    // signs the genesis operation, which the shard submits.
    let reserved = shard_call(
        s,
        &shard,
        false,
        "com.atproto.server.reserveSigningKey",
        &[],
        Some(&json!({})),
        ShardAuth::None,
    )
    .await?;
    let signing_key = reserved
        .get("signingKey")
        .and_then(Value::as_str)
        .ok_or_else(|| upstream("the shard reserved no signing key"))?
        .to_owned();
    let mut rotation_keys = Vec::new();
    if let Some(k) = input.recovery_key.as_deref().filter(|k| !k.is_empty()) {
        rotation_keys.push(k.to_owned());
    }
    let operator_recovery = snap.text("identity.recovery_did_key");
    if !operator_recovery.is_empty() {
        rotation_keys.push(operator_recovery.to_owned());
    }
    let current = s
        .secrets
        .plc_keys
        .current()
        .ok_or_else(|| XrpcError::internal("no PLC rotation key to sign with"))?;
    rotation_keys.push(current.did());
    let op = Operation {
        rotation_keys: rotation_keys.clone(),
        verification_methods: [("atproto".to_owned(), signing_key)].into(),
        also_known_as: vec![format!("at://{handle}")],
        services: [(
            "atproto_pds".to_owned(),
            Service {
                kind: "AtprotoPersonalDataServer".into(),
                endpoint: shard.url.clone(),
            },
        )]
        .into(),
        prev: None,
    };
    op.validate()
        .map_err(|e| XrpcError::invalid_request(e.to_string()))?;
    let genesis = current.sign_op(&op).await.map_err(XrpcError::internal)?;
    let did = genesis
        .genesis_did()
        .map_err(|e| XrpcError::internal(e.to_string()))?;
    shard_call(
        s,
        &shard,
        false,
        "com.atproto.server.createAccount",
        &[],
        Some(&json!({"handle": handle, "did": did, "plcOp": genesis.to_json()})),
        ShardAuth::None,
    )
    .await?;

    let mut tx = s.store.begin().await?;
    tx.insert_account(&NewAccount {
        did: &did,
        handle: &handle,
        email: Some(&email),
        password_hash: password_hash.as_deref(),
        status: Status::Active,
        did_method: "plc",
    })
    .await
    .map_err(|e| match e {
        CoreError::Conflict(m) if m.starts_with("Handle") => {
            XrpcError::named("HandleNotAvailable", m)
        }
        other => other.into(),
    })?;
    tx.set_account_shard(&did, &shard.name).await?;
    if let Some(code) = &input.invite_code {
        tx.use_invite_code(code, &did).await.map_err(|e| match e {
            CoreError::Invalid(m) => XrpcError::named("InvalidInviteCode", m),
            other => other.into(),
        })?;
    }
    tx.commit().await?;
    let _ = s
        .store
        .set_plc_key_listing(&did, &s.secrets.plc_keys.listed(&rotation_keys))
        .await;
    let tokens = s.sessions.create(&did, SCOPE_ACCESS, None, true).await?;
    Ok(json!({
        "handle": handle,
        "did": did,
        "accessJwt": tokens.access_jwt,
        "refreshJwt": tokens.refresh_jwt,
    }))
}

/// `activateAccount` on the entryway: the document must name the
/// entryway's PLC rotation key and the account's shard; the shard is told.
pub async fn activate(s: &AppState, did: &str) -> XrpcResult<()> {
    let shard = s
        .store
        .account_shard(did)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("The account has no shard"))?;
    let raw = s
        .plc()?
        .last_op_json(did)
        .await
        .map_err(|e| XrpcError::invalid_request(format!("Could not resolve DID: {e}")))?
        .ok_or_else(|| XrpcError::invalid_request("Could not resolve DID"))?;
    let (state, _) = locus_identity::plc::last_op_state(&raw)
        .map_err(|e| XrpcError::invalid_request(e.to_string()))?;
    let op = state.ok_or_else(|| XrpcError::invalid_request("DID is tombstoned"))?;
    if !s
        .accepted_plc_keys()
        .iter()
        .any(|k| op.rotation_keys.contains(k))
    {
        return Err(XrpcError::invalid_request(
            "Server rotation key not included in PLC DID data",
        ));
    }
    if op
        .services
        .get("atproto_pds")
        .map(|p| p.endpoint.trim_end_matches('/'))
        != Some(shard.url.as_str())
    {
        return Err(XrpcError::invalid_request(
            "DID document atproto_pds service endpoint does not match the account's PDS",
        ));
    }
    let mut tx = s.store.begin().await?;
    if !tx.activate_account(did).await? {
        return Err(XrpcError::named("AccountNotFound", "user not found"));
    }
    tx.commit().await?;
    push_status(s, did).await
}

// ---------------------------------------------------------------- auth

/// A service JWT a shard re-signed as the account for a method it passed
/// on (the account's own checks were made there): `aud` = the entryway,
/// `lxm` = this method, signed with the account's key.
pub async fn verify_service(s: &AppState, headers: &HeaderMap) -> XrpcResult<Authed> {
    let lxm = crate::routing::REQUEST
        .try_with(|r| {
            r.uri
                .path()
                .strip_prefix("/xrpc/")
                .unwrap_or_default()
                .to_owned()
        })
        .map_err(|_| XrpcError::internal("no request context"))?;
    let did = crate::auth::user_service_auth(s, headers, &lxm).await?;
    let account = s
        .store
        .account_by_did(&did)
        .await?
        .filter(|a| a.status != Status::Deleted)
        .ok_or_else(|| {
            XrpcError::new(
                StatusCode::FORBIDDEN,
                "AccountNotFound",
                "Account not found",
            )
        })?;
    Ok(Authed {
        did,
        scope: SCOPE_ACCESS.to_owned(),
        session_id: String::new(),
        app_password_name: None,
        status: account.status,
        oauth: None,
    })
}

/// Whether a bearer token is a service JWT (no session token type, an
/// `lxm` claim).
#[must_use]
pub fn is_service_jwt(token: &str) -> bool {
    use base64::Engine;
    let mut parts = token.split('.');
    let (Some(h), Some(p)) = (parts.next(), parts.next()) else {
        return false;
    };
    let dec = |x: &str| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(x)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    };
    let (Some(h), Some(p)) = (dec(h), dec(p)) else {
        return false;
    };
    !matches!(
        h.get("typ").and_then(Value::as_str),
        Some("at+jwt" | "refresh+jwt" | "dpop+jwt")
    ) && p.get("lxm").is_some()
}

/// The account a forwarded request is for: its token's subject (or a
/// service JWT's issuer), else the `repo` or `did` parameter.
fn subject_of(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    use base64::Engine;
    if let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        let p = token.split('.').nth(1)?;
        let v: Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(p)
                .ok()?,
        )
        .ok()?;
        return v
            .get("sub")
            .or_else(|| v.get("iss"))
            .and_then(Value::as_str)
            .map(|d| d.split('#').next().unwrap_or(d).to_owned());
    }
    url::form_urlencoded::parse(query.unwrap_or("").as_bytes())
        .find(|(k, _)| k == "repo" || k == "did")
        .map(|(_, v)| v.into_owned())
}

// ---------------------------------------------------------------- routing

/// Methods the entryway answers itself; every other XRPC method goes to
/// the account's shard.
fn served_here(nsid: &str) -> bool {
    if nsid.starts_with(locus_core::LOCUS_NS) || nsid.starts_with('_') {
        return true;
    }
    matches!(
        nsid,
        "com.atproto.server.describeServer"
            | "com.atproto.server.createAccount"
            | "com.atproto.server.createSession"
            | "com.atproto.server.getSession"
            | "com.atproto.server.refreshSession"
            | "com.atproto.server.deleteSession"
            | "com.atproto.server.createAppPassword"
            | "com.atproto.server.listAppPasswords"
            | "com.atproto.server.revokeAppPassword"
            | "com.atproto.server.requestEmailConfirmation"
            | "com.atproto.server.confirmEmail"
            | "com.atproto.server.requestEmailUpdate"
            | "com.atproto.server.updateEmail"
            | "com.atproto.server.requestPasswordReset"
            | "com.atproto.server.resetPassword"
            | "com.atproto.server.getAccountInviteCodes"
            | "com.atproto.server.createInviteCode"
            | "com.atproto.server.createInviteCodes"
            | "com.atproto.server.deactivateAccount"
            | "com.atproto.server.activateAccount"
            | "com.atproto.server.requestAccountDelete"
            | "com.atproto.server.deleteAccount"
            | "com.atproto.server.reserveSigningKey"
            | "com.atproto.temp.checkSignupQueue"
            | "com.atproto.temp.dereferenceScope"
            | "com.atproto.identity.resolveHandle"
            | "com.atproto.identity.updateHandle"
            | "com.atproto.identity.requestPlcOperationSignature"
            | "com.atproto.identity.signPlcOperation"
    ) || nsid.starts_with("com.atproto.admin.")
}

/// The entryway's routing: methods it doesn't answer go to the account's
/// shard (bearer requests unchanged: their token is addressed to it;
/// DPoP-bound ones can't be, so OAuth clients use the PDS endpoint).
pub async fn intercept(State(s): St, req: Request<Body>, next: Next) -> Response {
    if s.role != locus_core::bootstrap::Role::Entryway {
        return next.run(req).await;
    }
    let Some(nsid) = req.uri().path().strip_prefix("/xrpc/").map(str::to_owned) else {
        return next.run(req).await;
    };
    if served_here(&nsid) {
        return next.run(req).await;
    }
    match forward(&s, &nsid, req).await {
        Ok(r) | Err(r) => r,
    }
}

async fn forward(s: &AppState, nsid: &str, req: Request<Body>) -> Result<Response, Response> {
    let fail = |e: XrpcError| e.into_response();
    let (parts, body) = req.into_parts();
    let auth = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if auth.to_ascii_lowercase().starts_with("dpop ") {
        return Err(fail(XrpcError::invalid_request(
            "OAuth requests go to the account's PDS (the endpoint in its DID document)",
        )));
    }
    let who = subject_of(&parts.headers, parts.uri.query()).ok_or_else(|| {
        fail(XrpcError::invalid_request(
            "This entryway's accounts are on several PDSes: call the account's PDS",
        ))
    })?;
    let did = if who.starts_with("did:") {
        who
    } else {
        s.store
            .account_by_handle(&who.to_ascii_lowercase())
            .await
            .map_err(|e| fail(e.into()))?
            .map(|a| a.did)
            .ok_or_else(|| fail(XrpcError::invalid_request("Unknown account")))?
    };
    let shard = s
        .store
        .account_shard(&did)
        .await
        .map_err(|e| fail(e.into()))?
        .ok_or_else(|| fail(XrpcError::invalid_request("Unknown account")))?;
    let bytes: Bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|_| fail(XrpcError::invalid_request("Couldn't read the request body")))?;
    let client = crate::pipethrough::client(s, false).map_err(fail)?;
    let query = parts
        .uri
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let m = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
        .unwrap_or(reqwest::Method::GET);
    let mut out = client.request(m, format!("{}/xrpc/{nsid}{query}", shard.url));
    for (name, value) in &parts.headers {
        if matches!(
            name.as_str(),
            "host" | "content-length" | "connection" | "transfer-encoding"
        ) {
            continue;
        }
        out = out.header(name.as_str(), value.as_bytes());
    }
    if let Some(ip) = crate::ratelimit::client_ip() {
        out = out.header("x-forwarded-for", ip.to_string());
    }
    if !bytes.is_empty() {
        out = out.body(bytes);
    }
    let res = out.send().await.map_err(|e| {
        tracing::warn!(shard = %shard.name, nsid, "forwarding failed: {e}");
        fail(upstream(format!(
            "The PDS {} couldn't be reached",
            shard.name
        )))
    })?;
    Ok(crate::pipethrough::stream_back(s, res))
}

/// `com.atproto.temp.dereferenceScope`: a locus entryway always sends full
/// scopes, so there's no reference to look up (served unauthenticated, as
/// reference shards call it).
pub async fn dereference_scope(State(s): St) -> XrpcResult<Json<Value>> {
    entryway_only(&s)?;
    Err(XrpcError::named(
        "InvalidScopeReference",
        "Unknown scope reference",
    ))
}

// ---------------------------------------------------------------- shard registry

async fn owner(s: &AppState, headers: &HeaderMap, lxm: &str) -> XrpcResult<admin_auth::Caller> {
    entryway_only(s)?;
    admin_auth::require(s, headers, lxm, Surface::Locus, Role::Owner).await
}

fn shard_view(sh: &Shard) -> Value {
    json!({
        "name": sh.name,
        "url": sh.url,
        "did": sh.did,
        "locus": sh.locus,
        "weight": sh.weight,
        "draining": sh.draining,
        "accounts": sh.accounts,
        "addedAt": locus_core::time::iso8601(sh.added_at),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddShardInput {
    url: String,
    admin_password: String,
    name: Option<String>,
    weight: Option<i64>,
    #[serde(default)]
    confirm_non_locus: bool,
}

/// What a shard's `describeCapabilities` answer says it is.
enum Probe {
    Locus,
    NonLocus,
}

/// `owner.shard.add`: probes the shard (`describeCapabilities`), checks
/// its admin password, and on a locus shard installs a fresh callback
/// token (a service JWT from the entryway's token key). Writes nothing if
/// any step fails. Re-adding a listed shard re-probes and re-issues.
pub async fn add_shard(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<AddShardInput>,
) -> XrpcResult<Json<Value>> {
    let who = owner(&s, &headers, locus_nsid!("owner.shard.add")).await?;
    let url = input.url.trim().trim_end_matches('/').to_owned();
    let parsed = url::Url::parse(&url)
        .map_err(|_| XrpcError::invalid_request("The shard URL is invalid"))?;
    if !matches!(parsed.scheme(), "https" | "http") || parsed.host_str().is_none() {
        return Err(XrpcError::invalid_request("The shard URL must be http(s)"));
    }
    let name = input
        .name
        .clone()
        .unwrap_or_else(|| parsed.host_str().unwrap_or_default().to_owned());
    let client = crate::pipethrough::client(&s, false)?;

    // 1. The probe: a definite answer, or nothing is written.
    let probe = match client
        .get(format!(
            "{url}/xrpc/{}",
            locus_nsid!("describeCapabilities")
        ))
        .timeout(Duration::from_secs(15))
        .send()
        .await
    {
        Err(e) => {
            return Err(upstream(format!("The shard didn't answer: {e}")));
        }
        Ok(r) if r.status().is_success() => {
            let v: Value = r.json().await.unwrap_or(Value::Null);
            if v.get("locus").and_then(Value::as_bool) == Some(true) {
                Probe::Locus
            } else {
                Probe::NonLocus
            }
        }
        Ok(r) if matches!(r.status().as_u16(), 400 | 401 | 404 | 501) => Probe::NonLocus,
        Ok(r) => {
            return Err(upstream(format!(
                "The shard answered {} to describeCapabilities; try again",
                r.status()
            )));
        }
    };
    let listed = s.store.shard(&name).await?;
    if matches!(probe, Probe::NonLocus)
        && listed.as_ref().is_some_and(|l| l.locus)
        && !input.confirm_non_locus
    {
        return Err(XrpcError::invalid_request(
            "This listed locus shard now answers as non-locus; give --confirm-non-locus to record it so",
        ));
    }

    // 2. Its DID, and the admin password.
    let probe_shard = Shard {
        name: name.clone(),
        url: url.clone(),
        did: String::new(),
        locus: matches!(probe, Probe::Locus),
        weight: 1,
        draining: false,
        added_at: 0,
        accounts: 0,
    };
    let desc = shard_call(
        &s,
        &probe_shard,
        true,
        "com.atproto.server.describeServer",
        &[],
        None,
        ShardAuth::None,
    )
    .await?;
    let did = desc
        .get("did")
        .and_then(Value::as_str)
        .filter(|d| d.starts_with("did:"))
        .ok_or_else(|| upstream("The shard's describeServer gives no DID"))?
        .to_owned();
    let check = client
        .get(format!("{url}/xrpc/com.atproto.admin.getSubjectStatus"))
        .query(&[("did", "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa")])
        .basic_auth("admin", Some(&input.admin_password))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| upstream(format!("The shard didn't answer: {e}")))?;
    if check.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(XrpcError::invalid_request(
            "The shard refused the admin password",
        ));
    }

    // 3. A locus shard gets a fresh callback token.
    let callback = if matches!(probe, Probe::Locus) {
        let token = Zeroizing::new(hex::encode(rand::random::<[u8; 32]>()));
        let key = token_key(&s).await?;
        let now = locus_core::time::now_ms() / 1000;
        let jwt = locus_auth::service_jwt::create(
            &key,
            &s.service_did,
            &did,
            Some(now + 60),
            Some(locus_nsid!("shard.setCallbackToken")),
            now,
        )
        .map_err(|e| XrpcError::internal(e.to_string()))?;
        let res = client
            .post(format!(
                "{url}/xrpc/{}",
                locus_nsid!("shard.setCallbackToken")
            ))
            .bearer_auth(jwt)
            .json(&json!({"token": *token}))
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| upstream(format!("The shard didn't answer: {e}")))?;
        if !res.status().is_success() {
            let v: Value = res.json().await.unwrap_or(Value::Null);
            return Err(XrpcError::invalid_request(format!(
                "The shard refused the callback token ({}); is it configured with this entryway's DID and token key?",
                v.get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("no reason given")
            )));
        }
        Some(token)
    } else {
        None
    };

    let mut tx = s.store.begin().await?;
    tx.put_secret(
        &admin_password_secret(&name),
        input.admin_password.as_bytes(),
        &s.master_key,
    )
    .await?;
    if let Some(t) = &callback {
        tx.put_secret(&callback_token_secret(&name), t.as_bytes(), &s.master_key)
            .await?;
    }
    tx.commit().await?;
    s.store
        .upsert_shard(
            &name,
            &url,
            &did,
            callback.is_some(),
            input.weight.unwrap_or(1).max(0),
        )
        .await?;
    admin_auth::audit(
        &s,
        &who,
        "shard.add",
        Some(&name),
        Some(json!({"url": url, "did": did, "locus": callback.is_some()})),
    )
    .await;
    let sh = s
        .store
        .shard(&name)
        .await?
        .ok_or_else(|| XrpcError::internal("the shard vanished"))?;
    Ok(Json(shard_view(&sh)))
}

pub async fn list_shards(State(s): St, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    owner(&s, &headers, locus_nsid!("owner.shard.list")).await?;
    let shards: Vec<Value> = s.store.shards().await?.iter().map(shard_view).collect();
    Ok(Json(json!({ "shards": shards })))
}

#[derive(Deserialize)]
pub struct ShardSetInput {
    name: String,
    weight: Option<i64>,
    draining: Option<bool>,
}

/// `owner.shard.update`: a shard's placement weight, or draining (no new
/// accounts).
pub async fn update_shard(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<ShardSetInput>,
) -> XrpcResult<Json<Value>> {
    let who = owner(&s, &headers, locus_nsid!("owner.shard.update")).await?;
    let missing = || XrpcError::invalid_request("No such shard");
    if let Some(w) = input.weight {
        if !(0..=10_000).contains(&w) {
            return Err(XrpcError::invalid_request("A weight is 0 to 10000"));
        }
        if !s.store.set_shard_weight(&input.name, w).await? {
            return Err(missing());
        }
    }
    if let Some(d) = input.draining
        && !s.store.set_shard_draining(&input.name, d).await?
    {
        return Err(missing());
    }
    admin_auth::audit(
        &s,
        &who,
        "shard.update",
        Some(&input.name),
        Some(json!({"weight": input.weight, "draining": input.draining})),
    )
    .await;
    let sh = s.store.shard(&input.name).await?.ok_or_else(missing)?;
    Ok(Json(shard_view(&sh)))
}

/// The entryway's routes (its own methods; refused in other roles).
pub fn routes() -> axum::Router<Arc<AppState>> {
    macro_rules! at {
        ($name:literal) => {
            concat!("/xrpc/", locus_nsid!($name))
        };
    }
    axum::Router::new()
        .route(at!("owner.shard.add"), post(add_shard))
        .route(at!("owner.shard.list"), get(list_shards))
        .route(at!("owner.shard.update"), post(update_shard))
        .route(
            "/xrpc/com.atproto.temp.dereferenceScope",
            get(dereference_scope),
        )
}
