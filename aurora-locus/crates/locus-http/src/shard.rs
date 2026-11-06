//! The shard role: a PDS behind an entryway, to the reference's contract.
//!
//! The entryway owns accounts, logins, OAuth and handles; the shard keeps
//! repos, blobs and the firehose. Access tokens are ES256K JWTs from the
//! entryway, checked against its verify keys. Account-management methods
//! go to the entryway: some with the client's own credentials, the rest
//! re-signed as service auth from the account (bearer and DPoP tokens
//! can't be forwarded). The entryway calls back with the admin password
//! (`admin.updateAccountHandle`, `admin.updateSubjectStatus`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use locus_auth::oauth::jose::{Jws, PublicJwk};
use locus_auth::session::{
    Authed, OAuthGrant, SCOPE_ACCESS, SCOPE_APP_PASS, SCOPE_APP_PASS_PRIVILEGED, SCOPE_OAUTH,
    SCOPE_REFRESH, SCOPE_SIGNUP_QUEUED, SCOPE_TAKENDOWN,
};
use locus_core::keys::MasterKey;
use locus_core::settings::Snapshot;
use locus_core::{CoreError, CoreResult};
use locus_identity::plc::SignedOp;
use locus_repo::event::{account_body, identity_body, sync_body};
use locus_store::Store;
use locus_store::accounts::{NewAccount, Status};
use proto_blue_crypto::{ExportableKeypair, K256Keypair, Keypair};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::auth;
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

/// The secret holding the admin token the shard uses toward the entryway.
pub const ADMIN_TOKEN_SECRET: &str = "entryway_admin_token";
/// The longest legacy access-token lifetime: a replaced verify key stays
/// accepted this long.
pub const VERIFY_KEY_OVERLAP_MS: i64 = 2 * 60 * 60 * 1000;
/// Unused reserved signing keys expire after this.
pub const RESERVED_KEY_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// The entryway this shard is behind.
pub struct Entryway {
    /// Its URL, without a trailing slash (also the OAuth issuer).
    pub url: String,
    pub did: String,
    /// Accepted token verify keys (did:key, key, accepted until in ms).
    verify_keys: Vec<(String, PublicJwk, Option<i64>)>,
    /// Accepted PLC rotation keys for genesis operations, current first.
    pub plc_rotation_keys: Vec<String>,
    admin_token: Zeroizing<String>,
    scopes: Mutex<HashMap<String, (Instant, String)>>,
}

impl std::fmt::Debug for Entryway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entryway")
            .field("url", &self.url)
            .field("did", &self.did)
            .finish_non_exhaustive()
    }
}

/// A K256 public key as hex (compressed or uncompressed SEC1) → did:key.
pub fn hex_to_did_key(hex_key: &str) -> Result<String, String> {
    let bytes = hex::decode(hex_key.trim()).map_err(|e| format!("not hex: {e}"))?;
    let key = k256::PublicKey::from_sec1_bytes(&bytes)
        .map_err(|_| "not a secp256k1 public key".to_owned())?;
    let compressed = k256::elliptic_curve::sec1::ToEncodedPoint::to_encoded_point(&key, true);
    Ok(proto_blue_crypto::format_did_key(
        "ES256K",
        compressed.as_bytes(),
    ))
}

/// A K256 did:key as a JOSE verifier.
pub fn did_key_verifier(did_key: &str) -> Result<PublicJwk, String> {
    let parsed = proto_blue_crypto::parse_did_key(did_key).map_err(|e| e.to_string())?;
    if parsed.jwt_alg != "ES256K" {
        return Err(format!("{did_key} isn't a secp256k1 key"));
    }
    let key = k256::ecdsa::VerifyingKey::from_sec1_bytes(&parsed.key_bytes)
        .map_err(|_| format!("{did_key} isn't a valid key"))?;
    Ok(PublicJwk::K256(key))
}

/// A K256 did:key, as given or from hex.
pub fn parse_k256_key(value: &str) -> Result<String, String> {
    let v = value.trim();
    if v.starts_with("did:key:") {
        did_key_verifier(v)?;
        Ok(v.to_owned())
    } else {
        hex_to_did_key(v)
    }
}

fn env_value<'a>(env: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    env.get(k).map(|v| v.trim()).filter(|v| !v.is_empty())
}

impl Entryway {
    /// Loads the shard's entryway settings and keys. On the first boot the
    /// verify key, PLC rotation key and admin token come from env; after
    /// that they change with `keys set`, and a different env value is
    /// ignored with a warning.
    pub async fn load(
        store: &Store,
        key: &MasterKey,
        env: &HashMap<String, String>,
        snap: &Snapshot,
        admin_password: &[u8],
    ) -> CoreResult<Self> {
        let missing = |what: &str| {
            CoreError::Config(format!(
                "the shard role needs {what} (all of PDS_ENTRYWAY_URL, PDS_ENTRYWAY_DID, \
                 PDS_ENTRYWAY_JWT_VERIFY_KEY_K256_PUBLIC_KEY_HEX and PDS_ENTRYWAY_PLC_ROTATION_KEY)"
            ))
        };
        let url = snap.text("entryway.url").trim_end_matches('/').to_owned();
        if url.is_empty() {
            return Err(missing("the entryway URL"));
        }
        let did = snap.text("entryway.did").to_owned();
        if !did.starts_with("did:") {
            return Err(missing("the entryway DID"));
        }

        // Verify keys: seeded from env once, then `keys set`.
        let env_verify = env_value(env, "PDS_ENTRYWAY_JWT_VERIFY_KEY_K256_PUBLIC_KEY_HEX")
            .map(hex_to_did_key)
            .transpose()
            .map_err(|e| {
                CoreError::Config(format!(
                    "PDS_ENTRYWAY_JWT_VERIFY_KEY_K256_PUBLIC_KEY_HEX: {e}"
                ))
            })?;
        if !store.has_shard_verify_keys().await? {
            let k = env_verify
                .clone()
                .ok_or_else(|| missing("the entryway's token verify key"))?;
            let mut tx = store.begin().await?;
            tx.replace_shard_verify_keys(&[k]).await?;
            tx.commit().await?;
        }
        let rows = store.shard_verify_keys().await?;
        if let Some(k) = &env_verify
            && !rows.iter().any(|r| &r.did_key == k)
        {
            tracing::warn!(
                "PDS_ENTRYWAY_JWT_VERIFY_KEY_K256_PUBLIC_KEY_HEX differs from the stored verify key and is ignored; \
                 change it with `aurora-locus keys set entryway-verify-key`"
            );
        }
        let verify_keys = rows
            .into_iter()
            .map(|r| {
                did_key_verifier(&r.did_key)
                    .map(|v| (r.did_key.clone(), v, r.expires_at))
                    .map_err(CoreError::Config)
            })
            .collect::<CoreResult<Vec<_>>>()?;
        if verify_keys.is_empty() {
            return Err(CoreError::Config(
                "no entryway verify key is accepted any more; set one with `aurora-locus keys set entryway-verify-key`".into(),
            ));
        }

        // PLC rotation keys: a plain setting, seeded from env once.
        let env_plc = env_value(env, "PDS_ENTRYWAY_PLC_ROTATION_KEY").map(str::to_owned);
        let mut plc_rotation_keys: Vec<String> = snap
            .list("entryway.plc_rotation_keys")
            .into_iter()
            .map(str::to_owned)
            .collect();
        if plc_rotation_keys.is_empty() {
            let k = env_plc
                .clone()
                .ok_or_else(|| missing("the entryway's PLC rotation key"))?;
            did_key_verifier(&k)
                .map_err(|e| CoreError::Config(format!("PDS_ENTRYWAY_PLC_ROTATION_KEY: {e}")))?;
            store
                .set_setting("entryway.plc_rotation_keys", json!([k]), "env")
                .await?;
            plc_rotation_keys = vec![k];
        } else if let Some(k) = &env_plc
            && !plc_rotation_keys.contains(k)
        {
            tracing::warn!(
                "PDS_ENTRYWAY_PLC_ROTATION_KEY isn't an accepted entryway PLC rotation key and is ignored; \
                 change it with `aurora-locus keys set entryway-plc-rotation-key`"
            );
        }

        // The admin token: PDS_ENTRYWAY_ADMIN_TOKEN, else the admin password.
        let admin_token = match store.get_secret(ADMIN_TOKEN_SECRET, key).await? {
            Some(v) => v,
            None => {
                let v = env_value(env, "PDS_ENTRYWAY_ADMIN_TOKEN")
                    .map_or_else(|| admin_password.to_vec(), |t| t.as_bytes().to_vec());
                let mut tx = store.begin().await?;
                tx.put_secret_if_absent(ADMIN_TOKEN_SECRET, &v, key).await?;
                tx.commit().await?;
                store
                    .get_secret(ADMIN_TOKEN_SECRET, key)
                    .await?
                    .ok_or_else(|| CoreError::Storage("the entryway admin token vanished".into()))?
            }
        };
        Ok(Self {
            url,
            did,
            verify_keys,
            plc_rotation_keys,
            admin_token: Zeroizing::new(String::from_utf8_lossy(&admin_token).into_owned()),
            scopes: Mutex::new(HashMap::new()),
        })
    }

    /// For tests: an entryway with these keys.
    #[must_use]
    pub fn for_test(url: &str, did: &str, verify: &[String], plc: &[String]) -> Self {
        Self {
            url: url.trim_end_matches('/').to_owned(),
            did: did.to_owned(),
            verify_keys: verify
                .iter()
                .map(|k| (k.clone(), did_key_verifier(k).expect("a k256 did:key"), None))
                .collect(),
            plc_rotation_keys: plc.to_vec(),
            admin_token: Zeroizing::new("entryway-admin".into()),
            scopes: Mutex::new(HashMap::new()),
        }
    }

    /// The accepted verify keys (did:key).
    #[must_use]
    pub fn verify_key_ids(&self) -> Vec<String> {
        self.verify_keys.iter().map(|(k, ..)| k.clone()).collect()
    }

    /// Checks an ES256K JWS against the accepted verify keys. A previous
    /// key stops being accepted when its overlap ends, without a reload.
    fn verify(&self, jws: &Jws) -> bool {
        let now = locus_core::time::now_ms();
        jws.alg() == "ES256K"
            && self.verify_keys.iter().any(|(_, k, until)| {
                until.is_none_or(|t| t > now)
                    && k.verify("ES256K", &jws.signing_input, &jws.signature)
                        .is_ok()
            })
    }

    /// Basic auth for the entryway's admin API, with the admin token (the
    /// reference holds it for that too, and makes no such call in 0.5).
    #[must_use]
    pub fn admin_authorization(&self) -> String {
        use base64::Engine;
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD
                .encode(format!("admin:{}", *self.admin_token))
        )
    }
}

// ---------------------------------------------------------------- tokens

fn invalid() -> XrpcError {
    XrpcError::named("InvalidToken", "Token could not be verified")
}

fn now_s() -> i64 {
    locus_core::time::now_ms() / 1000
}

/// A verified entryway JWT: (typ checked, signature, not expired).
fn entryway_jwt(ew: &Entryway, token: &str, typ: &str, allow_expired: bool) -> XrpcResult<Jws> {
    let jws = Jws::decode(token).map_err(|_| invalid())?;
    if !jws.typ_is(typ) || !ew.verify(&jws) {
        return Err(invalid());
    }
    let exp = jws.payload.get("exp").and_then(Value::as_i64).unwrap_or(0);
    if !allow_expired && exp <= now_s() {
        return Err(XrpcError::named("ExpiredToken", "Token has expired"));
    }
    Ok(jws)
}

fn claim<'a>(jws: &'a Jws, k: &str) -> Option<&'a str> {
    jws.payload.get(k).and_then(Value::as_str)
}

/// The account behind a token, checked as the reference's `verifyStatus`.
async fn token_account(
    s: &AppState,
    did: &str,
    scope: &str,
) -> XrpcResult<locus_store::accounts::Account> {
    let account = s
        .store
        .account_by_did(did)
        .await?
        .filter(|a| a.status != Status::Deleted)
        .ok_or_else(|| {
            XrpcError::new(
                StatusCode::FORBIDDEN,
                "AccountNotFound",
                "Account not found",
            )
        })?;
    if matches!(account.status, Status::Takendown | Status::Suspended) && scope != SCOPE_TAKENDOWN {
        return Err(XrpcError::new(
            StatusCode::UNAUTHORIZED,
            "AccountTakedown",
            "Account has been taken down",
        ));
    }
    Ok(account)
}

/// A legacy access token from the entryway: ES256K, `typ: at+jwt`, `aud`
/// = this shard's DID, a reference scope. No session to look up (the
/// entryway keeps them); the account's status is checked here.
pub async fn verify_access(s: &AppState, ew: &Entryway, token: &str) -> XrpcResult<Authed> {
    let jws = entryway_jwt(ew, token, "at+jwt", false)?;
    let p = &jws.payload;
    if p.contains_key("lxm") || p.contains_key("cnf") {
        return Err(XrpcError::named("InvalidToken", "Malformed token"));
    }
    let sub = claim(&jws, "sub")
        .filter(|d| d.starts_with("did:"))
        .ok_or_else(|| XrpcError::named("InvalidToken", "Malformed token"))?;
    if claim(&jws, "aud") != Some(s.service_did.as_str()) {
        return Err(invalid());
    }
    let scope = claim(&jws, "scope").unwrap_or("");
    if ![
        SCOPE_ACCESS,
        SCOPE_APP_PASS_PRIVILEGED,
        SCOPE_APP_PASS,
        SCOPE_TAKENDOWN,
        SCOPE_SIGNUP_QUEUED,
    ]
    .contains(&scope)
    {
        return Err(XrpcError::named("InvalidToken", "Bad token scope"));
    }
    let account = token_account(s, sub, scope).await?;
    let sid = claim(&jws, "sid").unwrap_or("").to_owned();
    check_revoked(s, sub, &sid, &jws).await?;
    Ok(Authed {
        did: sub.to_owned(),
        scope: scope.to_owned(),
        session_id: if sid.is_empty() {
            claim(&jws, "jti").unwrap_or("").to_owned()
        } else {
            sid
        },
        app_password_name: None,
        status: account.status,
        oauth: None,
    })
}

/// A refresh token for the entryway (checked before it's passed on):
/// ES256K, `typ: refresh+jwt`, `aud` = the entryway's DID, with a `jti`.
fn verify_refresh(ew: &Entryway, headers: &HeaderMap, allow_expired: bool) -> XrpcResult<()> {
    let token = auth::bearer(headers)?;
    let jws = entryway_jwt(ew, token, "refresh+jwt", allow_expired)?;
    if claim(&jws, "aud") != Some(ew.did.as_str())
        || claim(&jws, "scope") != Some(SCOPE_REFRESH)
        || !claim(&jws, "sub").is_some_and(|d| d.starts_with("did:"))
    {
        return Err(invalid());
    }
    if claim(&jws, "jti").is_none() {
        return Err(XrpcError::new(
            StatusCode::UNAUTHORIZED,
            "MissingTokenId",
            "Unexpected missing refresh token id",
        ));
    }
    Ok(())
}

/// An OAuth access token from the entryway, checked statelessly: ES256K
/// with its key, `iss` = the entryway, `aud` = this shard, DPoP-bound.
/// A `ref:<cid>` scope is dereferenced through the entryway.
pub async fn oauth_access(s: &AppState, ew: &Entryway, headers: &HeaderMap) -> XrpcResult<Authed> {
    use locus_auth::oauth::OAuthError;
    let bad = |m: &str| XrpcError::from(OAuthError::invalid_token("DPoP", m));
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' ').map(|(_, t)| t.trim().to_owned()))
        .filter(|t| !t.is_empty())
        .ok_or_else(|| bad("Malformed token"))?;
    let jws = Jws::decode(&token).map_err(|_| bad("Malformed token"))?;
    if !jws.typ_is("at+jwt") || !ew.verify(&jws) || claim(&jws, "iss") != Some(ew.url.as_str()) {
        return Err(bad("Invalid token"));
    }
    if claim(&jws, "aud") != Some(s.service_did.as_str()) {
        return Err(bad("Invalid audience"));
    }
    if jws.payload.get("exp").and_then(Value::as_i64).unwrap_or(0) <= now_s() {
        return Err(bad("Token expired"));
    }
    let info = crate::routing::REQUEST
        .try_with(Clone::clone)
        .map_err(|_| XrpcError::internal("no request context"))?;
    let proof = crate::oauth::dpop_proof(s, &info.method, &info.uri, headers, Some(&token))
        .await
        .map_err(XrpcError::from)?;
    let jkt = jws
        .payload
        .get("cnf")
        .and_then(|c| c.get("jkt"))
        .and_then(Value::as_str);
    match (jkt, &proof) {
        (Some(j), Some(pr)) if pr.jkt == j => {}
        (Some(_), None) => return Err(OAuthError::invalid_dpop_proof("DPoP proof required").into()),
        (Some(_), Some(_)) => return Err(OAuthError::invalid_dpop_key_binding().into()),
        (None, _) => {
            return Err(OAuthError::invalid_token(
                "Bearer",
                "Bearer token type must be used without a DPoP proof",
            )
            .into());
        }
    }
    let sub = claim(&jws, "sub")
        .filter(|d| d.starts_with("did:"))
        .ok_or_else(|| bad("Invalid token"))?
        .to_owned();
    let scope = dereference_scope(s, ew, claim(&jws, "scope").unwrap_or("")).await?;
    if !scope.split(' ').any(|x| x == "atproto") {
        return Err(XrpcError::named(
            "InvalidToken",
            "OAuth token does not have \"atproto\" scope",
        ));
    }
    let account = token_account(s, &sub, "").await?;
    check_revoked(s, &sub, claim(&jws, "sid").unwrap_or(""), &jws).await?;
    let jti = claim(&jws, "jti").unwrap_or("").to_owned();
    let sid = claim(&jws, "sid").map_or_else(|| jti.clone(), str::to_owned);
    Ok(Authed {
        did: sub,
        scope: SCOPE_OAUTH.to_owned(),
        session_id: sid.clone(),
        app_password_name: None,
        status: account.status,
        oauth: Some(OAuthGrant {
            client_id: claim(&jws, "client_id").unwrap_or("").to_owned(),
            token_id: sid,
            scope,
        }),
    })
}

/// A `ref:<cid>` scope from the entryway's `dereferenceScope`, cached a
/// day; other scopes as they are.
async fn dereference_scope(s: &AppState, ew: &Entryway, scope: &str) -> XrpcResult<String> {
    if !scope.starts_with("ref:") || scope.contains(' ') {
        return Ok(scope.to_owned());
    }
    const DAY: Duration = Duration::from_secs(86_400);
    if let Some((at, v)) = ew
        .scopes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(scope)
        && at.elapsed() < DAY
    {
        return Ok(v.clone());
    }
    let url = format!(
        "{}/xrpc/com.atproto.temp.dereferenceScope?scope={}",
        ew.url,
        url::form_urlencoded::byte_serialize(scope.as_bytes()).collect::<String>()
    );
    let client = crate::pipethrough::client(s, false)?;
    let res = client
        .get(url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| upstream("Failed to fetch token permissions"))?;
    let status = res.status();
    let body: Value = res.json().await.unwrap_or_default();
    if !status.is_success() {
        if body.get("error").and_then(Value::as_str) == Some("InvalidScopeReference") {
            return Err(locus_auth::oauth::OAuthError::invalid_token(
                "DPoP",
                "Invalid scope reference",
            )
            .into());
        }
        return Err(upstream("Failed to fetch token permissions"));
    }
    let full = body
        .get("scope")
        .and_then(Value::as_str)
        .ok_or_else(|| upstream("Failed to fetch token permissions"))?
        .to_owned();
    let mut cache = ew
        .scopes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.len() >= 1000 {
        cache.clear();
    }
    cache.insert(scope.to_owned(), (Instant::now(), full.clone()));
    Ok(full)
}

fn upstream(m: &str) -> XrpcError {
    XrpcError::new(StatusCode::BAD_GATEWAY, "UpstreamFailure", m)
}

/// Whether a service JWT's audience is acceptable: this shard or the
/// entryway (the reference's `verifyServiceJwt`).
#[must_use]
pub fn accepts_service_aud(s: &AppState, aud: &str) -> bool {
    s.entryway.as_ref().is_some_and(|ew| ew.did == aud)
}

// ---------------------------------------------------------------- methods

/// How a shard handles an account-management method.
#[derive(Clone, Copy)]
enum Rule {
    /// Sent on with the client's own credentials, after the local check.
    Pass(PassCheck),
    /// Authenticated here, then sent as service auth from the account.
    Resign {
        scopes: &'static [&'static str],
        extra: &'static [&'static str],
        oauth: OAuthRule,
    },
    /// `admin.sendEmail`: a moderator here, re-signed as the recipient.
    SendEmail,
    /// Refused (the entryway manages it).
    Refused(&'static str),
}

#[derive(Clone, Copy)]
enum PassCheck {
    None,
    Refresh {
        allow_expired: bool,
    },
    /// Full access, OAuth refused with this message.
    Full(&'static str),
    SignupQueue,
    /// The account named by the body's `did` must be on this shard.
    LocalAccount,
    Admin,
}

#[derive(Clone, Copy)]
enum OAuthRule {
    Any,
    Forbid(&'static str),
    Account(&'static str, &'static str),
    Identity(&'static str),
}

const INVITES: &str = "Account invites are managed by the entryway service";

fn rule(nsid: &str) -> Option<Rule> {
    use PassCheck as P;
    let resign = |scopes, extra, oauth| Rule::Resign {
        scopes,
        extra,
        oauth,
    };
    let unsupported = OAuthRule::Forbid(auth::OAUTH_UNSUPPORTED);
    Some(match nsid {
        "com.atproto.server.createSession"
        | "com.atproto.server.resetPassword"
        | "com.atproto.server.requestPasswordReset" => Rule::Pass(P::None),
        "com.atproto.server.refreshSession" => Rule::Pass(P::Refresh {
            allow_expired: false,
        }),
        "com.atproto.server.deleteSession" => Rule::Pass(P::Refresh {
            allow_expired: true,
        }),
        "com.atproto.server.activateAccount" => Rule::Pass(P::Full(
            "Account reactivation is not available with OAuth credentials. Sign in to your account management page to reactivate.",
        )),
        "com.atproto.server.deleteAccount" => Rule::Pass(P::LocalAccount),
        "com.atproto.temp.checkSignupQueue" => Rule::Pass(P::SignupQueue),
        "com.atproto.admin.updateAccountEmail" | "com.atproto.admin.updateAccountPassword" => {
            Rule::Pass(P::Admin)
        }
        "com.atproto.server.getSession" => {
            resign(auth::STANDARD, &[SCOPE_SIGNUP_QUEUED], OAuthRule::Any)
        }
        "com.atproto.server.createAppPassword"
        | "com.atproto.server.requestAccountDelete"
        | "com.atproto.server.getAccountInviteCodes"
        | "com.atproto.server.updateEmail" => resign(auth::FULL, &[], unsupported),
        "com.atproto.server.listAppPasswords"
        | "com.atproto.server.revokeAppPassword"
        | "com.atproto.server.confirmEmail" => resign(auth::STANDARD, &[], unsupported),
        "com.atproto.server.requestEmailConfirmation" => {
            resign(auth::STANDARD, &[], OAuthRule::Account("email", "manage"))
        }
        "com.atproto.server.requestEmailUpdate" => resign(
            auth::FULL,
            &[],
            OAuthRule::Forbid(
                "Use the account manager interface to update email address associated with an account",
            ),
        ),
        "com.atproto.server.deactivateAccount" => resign(
            auth::FULL,
            &[SCOPE_TAKENDOWN],
            OAuthRule::Account("status", "manage"),
        ),
        "com.atproto.identity.updateHandle" => {
            resign(auth::STANDARD, &[], OAuthRule::Identity("handle"))
        }
        "com.atproto.identity.requestPlcOperationSignature"
        | "com.atproto.identity.signPlcOperation" => {
            resign(auth::FULL, &[SCOPE_TAKENDOWN], OAuthRule::Identity("*"))
        }
        "com.atproto.admin.sendEmail" => Rule::SendEmail,
        "com.atproto.server.createInviteCode"
        | "com.atproto.server.createInviteCodes"
        | "com.atproto.admin.getInviteCodes"
        | "com.atproto.admin.disableAccountInvites"
        | "com.atproto.admin.enableAccountInvites"
        | "com.atproto.admin.disableInviteCodes" => Rule::Refused(INVITES),
        _ => return None,
    })
}

/// Paths the shard doesn't serve: the local OAuth authorization server
/// and `/.well-known/atproto-did`.
fn disabled_path(path: &str) -> bool {
    path == "/.well-known/oauth-authorization-server"
        || path == "/.well-known/atproto-did"
        || path == "/oauth/jwks"
        || path == "/oauth/par"
        || path == "/oauth/token"
        || path == "/oauth/revoke"
        || path.starts_with("/oauth/authorize")
}

/// The shard's routing: in the shard role, account-management methods go
/// to the entryway (or are refused); everything else is served here.
pub async fn intercept(State(s): State<Arc<AppState>>, req: Request<Body>, next: Next) -> Response {
    let Some(ew) = s.entryway.clone() else {
        return next.run(req).await;
    };
    let path = req.uri().path().to_owned();
    if disabled_path(&path) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(nsid) = path.strip_prefix("/xrpc/").map(str::to_owned) else {
        return next.run(req).await;
    };
    if nsid == "com.atproto.server.createAccount" {
        let (parts, body) = req.into_parts();
        let bytes = match axum::body::to_bytes(body, 1 << 20).await {
            Ok(b) => b,
            Err(_) => return XrpcError::invalid_request("Request body too large").into_response(),
        };
        return match create_account(&s, &ew, &parts.headers, &bytes).await {
            Ok(v) => Json(v).into_response(),
            Err(e) => e.into_response(),
        };
    }
    let Some(r) = rule(&nsid) else {
        return next.run(req).await;
    };
    match handle(&s, &ew, &nsid, r, req).await {
        Ok(res) | Err(res) => res,
    }
}

type Handled = Result<Response, Response>;

fn fail(e: XrpcError) -> Response {
    e.into_response()
}

async fn handle(s: &AppState, ew: &Entryway, nsid: &str, r: Rule, req: Request<Body>) -> Handled {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20)
        .await
        .map_err(|_| fail(XrpcError::invalid_request("Request body too large")))?;
    let headers = &parts.headers;
    let query = parts
        .uri
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    match r {
        Rule::Refused(m) => Err(fail(XrpcError::invalid_request(m))),
        Rule::Pass(check) => {
            pass_check(s, ew, nsid, check, headers, &bytes)
                .await
                .map_err(fail)?;
            let auth = headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            if let Some(a) = &auth
                && (a.to_ascii_lowercase().starts_with("dpop ") || headers.contains_key("dpop"))
            {
                return Err(fail(XrpcError::invalid_request(
                    "DPoP requests cannot be proxied",
                )));
            }
            forward(s, ew, &parts.method, nsid, &query, headers, auth, bytes).await
        }
        Rule::Resign {
            scopes,
            extra,
            oauth,
        } => {
            let authed = auth::authorization(s, headers, scopes, extra, |p| match oauth {
                OAuthRule::Any => Ok(()),
                OAuthRule::Forbid(m) => Err(XrpcError::new(StatusCode::FORBIDDEN, "Forbidden", m)),
                OAuthRule::Account(attr, action) => Ok(p.assert_account(attr, action)?),
                OAuthRule::Identity(attr) => Ok(p.assert_identity(attr)?),
            })
            .await
            .map_err(fail)?;
            let token = crate::pipethrough::service_token(s, &authed.did, &ew.did, nsid)
                .await
                .map_err(fail)?;
            let res = forward(
                s,
                ew,
                &parts.method,
                nsid,
                &query,
                headers,
                Some(format!("Bearer {token}")),
                bytes,
            )
            .await?;
            if nsid == "com.atproto.server.getSession" {
                return Ok(filter_session(res, authed.oauth.as_ref()).await);
            }
            Ok(res)
        }
        Rule::SendEmail => {
            crate::admin_auth::require(
                s,
                headers,
                nsid,
                crate::admin_auth::Surface::Reference { mod_service: true },
                crate::admin_auth::Role::Moderator,
            )
            .await
            .map_err(fail)?;
            let body: Value = serde_json::from_slice(&bytes)
                .map_err(|_| fail(XrpcError::invalid_request("Invalid request body")))?;
            let recipient = body
                .get("recipientDid")
                .and_then(Value::as_str)
                .ok_or_else(|| fail(XrpcError::invalid_request("recipientDid is required")))?;
            if s.store
                .account_by_did(recipient)
                .await
                .map_err(|e| fail(e.into()))?
                .is_none()
            {
                return Err(fail(XrpcError::invalid_request("Recipient not found")));
            }
            let token = crate::pipethrough::service_token(s, recipient, &ew.did, nsid)
                .await
                .map_err(fail)?;
            forward(
                s,
                ew,
                &parts.method,
                nsid,
                &query,
                headers,
                Some(format!("Bearer {token}")),
                bytes,
            )
            .await
        }
    }
}

async fn pass_check(
    s: &AppState,
    ew: &Entryway,
    nsid: &str,
    check: PassCheck,
    headers: &HeaderMap,
    body: &[u8],
) -> XrpcResult<()> {
    match check {
        PassCheck::None => {
            if nsid == "com.atproto.server.createSession" {
                use crate::ratelimit::buckets::{CREATE_SESSION_5M, CREATE_SESSION_DAY};
                let id = serde_json::from_slice::<Value>(body)
                    .ok()
                    .and_then(|v| {
                        v.get("identifier")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_default();
                let ip = crate::ratelimit::client_ip()
                    .map_or_else(|| "unknown".to_owned(), |i| i.to_string());
                let key = format!("{id}-{ip}");
                crate::ratelimit::limit(
                    s,
                    &[(CREATE_SESSION_DAY, &key, 1), (CREATE_SESSION_5M, &key, 1)],
                )
                .await?;
            }
            Ok(())
        }
        PassCheck::Refresh { allow_expired } => verify_refresh(ew, headers, allow_expired),
        PassCheck::Full(m) => auth::authorization(s, headers, auth::FULL, &[], auth::forbid(m))
            .await
            .map(|_| ()),
        PassCheck::SignupQueue => auth::authorization(
            s,
            headers,
            auth::STANDARD,
            &[SCOPE_SIGNUP_QUEUED],
            auth::forbid(auth::OAUTH_UNSUPPORTED),
        )
        .await
        .map(|_| ()),
        PassCheck::LocalAccount => {
            let did = serde_json::from_slice::<Value>(body)
                .ok()
                .and_then(|v| v.get("did").and_then(Value::as_str).map(str::to_owned))
                .unwrap_or_default();
            if s.store.account_by_did(&did).await?.is_none() {
                return Err(XrpcError::invalid_request("account not found"));
            }
            Ok(())
        }
        PassCheck::Admin => crate::admin_auth::require(
            s,
            headers,
            nsid,
            crate::admin_auth::Surface::Reference { mod_service: false },
            crate::admin_auth::Role::Admin,
        )
        .await
        .map(|_| ()),
    }
}

/// Sends a request to the entryway with `authorization` and the client's
/// IP, and hands back its answer (its error as the reference's client
/// gives it).
#[allow(clippy::too_many_arguments)]
async fn forward(
    s: &AppState,
    ew: &Entryway,
    method: &Method,
    nsid: &str,
    query: &str,
    headers: &HeaderMap,
    authorization: Option<String>,
    body: Bytes,
) -> Handled {
    let client = crate::pipethrough::client(s, false).map_err(fail)?;
    let url = format!("{}/xrpc/{nsid}{query}", ew.url);
    let m = reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut req = client.request(m, url).timeout(Duration::from_secs(30));
    if let Some(a) = authorization {
        req = req.header(reqwest::header::AUTHORIZATION, a);
    }
    if let Some(ip) = crate::ratelimit::client_ip() {
        req = req.header("x-forwarded-for", ip.to_string());
    }
    for name in ["content-type", "accept-language", "user-agent"] {
        if let Some(v) = headers.get(name).and_then(|v| v.to_str().ok()) {
            req = req.header(name, v);
        }
    }
    if *method == Method::POST {
        req = req.body(body);
    }
    let upstream = req.send().await.map_err(|e| {
        tracing::warn!(nsid, "entryway request failed: {e}");
        fail(upstream("Upstream service unreachable"))
    })?;
    if upstream.status().as_u16() >= 400 {
        return Err(crate::pipethrough::UpstreamError::read(s, upstream)
            .await
            .into_response());
    }
    Ok(crate::pipethrough::stream_back(s, upstream))
}

/// `getSession` from the entryway, without the email fields for an OAuth
/// grant that may not read them.
async fn filter_session(res: Response, grant: Option<&OAuthGrant>) -> Response {
    let Some(g) = grant else { return res };
    if g.permissions().allows_account("email", "read") {
        return res;
    }
    let (parts, body) = res.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 1 << 20).await else {
        return upstream("upstream response too large").into_response();
    };
    let Ok(mut v) = serde_json::from_slice::<Value>(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    if let Some(m) = v.as_object_mut() {
        for k in ["email", "emailConfirmed", "emailAuthFactor"] {
            m.remove(k);
        }
    }
    let mut out = Json(v).into_response();
    *out.status_mut() = parts.status;
    out
}

// ---------------------------------------------------------------- accounts

/// `createAccount` on a shard, as the entryway calls it: `did` and a
/// genesis `plcOp` that names the entryway's PLC rotation key and is signed
/// by it, a document naming the handle, this shard and the reserved
/// signing key. The shard submits the op itself; no email, password or
/// invite is stored, and the DID is never tombstoned here.
async fn create_account(
    s: &AppState,
    ew: &Entryway,
    _headers: &HeaderMap,
    body: &[u8],
) -> XrpcResult<Value> {
    crate::ratelimit::limit_ip(s, &[crate::ratelimit::buckets::CREATE_ACCOUNT]).await?;
    let incompatible = |m: &str| XrpcError::named("IncompatibleDidDoc", m);
    let input: Value = serde_json::from_slice(body)
        .map_err(|_| XrpcError::invalid_request("Invalid request body"))?;
    let handle = input
        .get("handle")
        .and_then(Value::as_str)
        .ok_or_else(|| XrpcError::invalid_request("handle is required"))?;
    let handle = locus_identity::handle::normalize_and_validate(handle)
        .map_err(|e| XrpcError::named("InvalidHandle", e.to_string()))?;
    let (Some(did), Some(op_json)) = (
        input.get("did").and_then(Value::as_str),
        input.get("plcOp").filter(|v| v.is_object()),
    ) else {
        return Err(XrpcError::invalid_request(
            "non-entryway pds requires bringing a DID and plcOp",
        ));
    };
    let op = SignedOp::from_json(op_json).map_err(|_| incompatible("invalid plc operation"))?;
    let SignedOp::Operation { op: fields, .. } = &op else {
        return Err(incompatible("invalid plc operation"));
    };
    // Signed by an accepted entryway key that the op names.
    let signer = ew
        .plc_rotation_keys
        .iter()
        .find(|k| fields.rotation_keys.contains(k) && op.verify(k));
    if !ew
        .plc_rotation_keys
        .iter()
        .any(|k| fields.rotation_keys.contains(k))
    {
        return Err(incompatible(
            "PLC DID does not include service rotation key",
        ));
    }
    if signer.is_none() || op.genesis_did().ok().as_deref() != Some(did) {
        return Err(incompatible("invalid plc operation"));
    }
    if s.store.account_by_did(did).await?.is_some() {
        return Err(XrpcError::invalid_request("Account already exists"));
    }
    let doc_key = fields.verification_methods.get("atproto").cloned();
    let reserved = match s.store.reserved_key(did, &s.master_key).await? {
        Some(k) => Some(k),
        None => match &doc_key {
            Some(k) => s.store.reserved_key(k, &s.master_key).await?,
            None => None,
        },
    };
    let (key_did, private) = reserved
        .ok_or_else(|| XrpcError::invalid_request("reserved signing key does not exist"))?;
    let signing = K256Keypair::from_private_key(&private)
        .map_err(|e| XrpcError::internal(format!("reserved key: {e}")))?;
    if fields.also_known_as.first().map(String::as_str) != Some(&format!("at://{handle}"))
        || fields.also_known_as.is_empty()
    {
        return Err(incompatible("DID document handle does not match"));
    }
    let pds = fields
        .services
        .get("atproto_pds")
        .filter(|svc| svc.kind == "AtprotoPersonalDataServer")
        .map(|svc| svc.endpoint.trim_end_matches('/').to_owned());
    if pds.as_deref() != Some(s.public_url.trim_end_matches('/')) {
        return Err(incompatible(
            "DID document pds endpoint does not match service",
        ));
    }
    if doc_key.as_deref() != Some(key_did.as_str()) {
        return Err(incompatible(
            "DID document signing key does not match service",
        ));
    }
    if s.store.account_by_handle(&handle).await?.is_some() {
        return Err(XrpcError::named(
            "HandleNotAvailable",
            "Handle already taken",
        ));
    }

    // The op goes to PLC first, as the reference.
    let plc = s.plc()?;
    if let Err(e) = plc.submit(did, &op).await {
        match e {
            locus_identity::plc_client::PlcClientError::Refused(m) => {
                return Err(XrpcError::invalid_request(format!(
                    "The PLC directory refused the DID: {m}"
                )));
            }
            other => match plc.document(did).await {
                Ok(Some(_)) => {}
                _ => return Err(upstream(&other.to_string())),
            },
        }
    }

    let enc = |e: proto_blue_repo::error::RepoError| XrpcError::internal(e.to_string());
    let mut tx = s.store.begin().await?;
    tx.insert_account(&NewAccount {
        did,
        handle: &handle,
        email: None,
        password_hash: None,
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
    tx.put_signing_key(did, "k256", &signing.export_private_key(), &s.master_key)
        .await?;
    tx.insert_outbox(
        did,
        "identity",
        &identity_body(did, Some(&handle)).map_err(enc)?,
    )
    .await?;
    tx.insert_outbox(did, "account", &account_body(did, true, None).map_err(enc)?)
        .await?;
    let repo = s
        .repo
        .create_repo_in(&mut tx, did, &signing, true)
        .await
        .map_err(|e| XrpcError::internal(e.to_string()))?;
    tx.insert_outbox(
        did,
        "sync",
        &sync_body(did, &repo.rev, &repo.commit_cid, &repo.commit_bytes).map_err(enc)?,
    )
    .await?;
    tx.clear_reserved_key(did, &key_did).await?;
    tx.commit().await?;
    // The entryway issues the account's tokens; the reference's output
    // still carries a pair, which nothing here accepts.
    Ok(json!({
        "handle": handle,
        "did": did,
        "accessJwt": "",
        "refreshJwt": "",
    }))
}

/// `/.well-known/change-password`: the entryway's, on a shard.
pub async fn change_password(State(s): State<Arc<AppState>>) -> Response {
    match &s.entryway {
        Some(ew) => {
            Redirect::to(&format!("{}/.well-known/change-password", ew.url)).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `com.atproto.server.reserveSigningKey` (every role, unauthenticated, as
/// the reference): a signing key for an account about to be created here,
/// kept under the DID given (or the key's did:key) for a day.
pub async fn reserve_signing_key(
    State(s): State<Arc<AppState>>,
    body: Bytes,
) -> XrpcResult<Json<Value>> {
    crate::ratelimit::limit_ip(&s, &[crate::ratelimit::buckets::RESERVE_SIGNING_KEY]).await?;
    if let Err(e) = s.store.prune_reserved_keys().await {
        tracing::warn!("couldn't prune reserved signing keys: {e}");
    }
    let did = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| v.get("did").and_then(Value::as_str).map(str::to_owned));
    if let Some(d) = &did
        && !d.starts_with("did:")
    {
        return Err(XrpcError::invalid_request("did must be a DID"));
    }
    if let Some(d) = &did
        && let Some((k, _)) = s.store.reserved_key(d, &s.master_key).await?
    {
        return Ok(Json(json!({ "signingKey": k })));
    }
    let key = K256Keypair::generate();
    let did_key = key.did();
    s.store
        .reserve_key(
            did.as_deref().unwrap_or(&did_key),
            &did_key,
            &key.export_private_key(),
            &s.master_key,
            RESERVED_KEY_TTL_MS,
        )
        .await?;
    Ok(Json(json!({ "signingKey": did_key })))
}

// ---------------------------------------------------------------- callbacks

/// How long a revocation is kept: past the longest access-token lifetime
/// (2 h) plus clock skew.
pub const REVOCATION_KEEP_MS: i64 = VERIFY_KEY_OVERLAP_MS + 60_000;

fn callback_token_hash(token: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(token.as_bytes()))
}

/// Refuses a token whose session the entryway revoked, or that was
/// issued before the account's `not_before`.
async fn check_revoked(s: &AppState, did: &str, sid: &str, jws: &Jws) -> XrpcResult<()> {
    let iat_ms = jws.payload.get("iat").and_then(Value::as_i64).unwrap_or(0) * 1000;
    if s.store.shard_revoked(did, sid, iat_ms).await? {
        return Err(XrpcError::named("ExpiredToken", "Token has been revoked"));
    }
    Ok(())
}

fn shard_only(s: &AppState) -> XrpcResult<&Entryway> {
    s.entryway.as_deref().ok_or_else(|| {
        XrpcError::new(
            StatusCode::NOT_IMPLEMENTED,
            "MethodNotImplemented",
            "Only a shard takes this call",
        )
    })
}

/// The shard-callback token, checked against the installed hash.
async fn callback_auth<'a>(s: &'a AppState, headers: &HeaderMap) -> XrpcResult<&'a Entryway> {
    let ew = shard_only(s)?;
    let token = auth::bearer(headers)?;
    let ok = match s.store.callback_token_hash().await? {
        Some(stored) => bool::from(subtle::ConstantTimeEq::ct_eq(
            stored.as_bytes(),
            callback_token_hash(token).as_bytes(),
        )),
        None => false,
    };
    if !ok {
        return Err(XrpcError::new(
            StatusCode::UNAUTHORIZED,
            "AuthenticationRequired",
            "Invalid shard-callback token",
        ));
    }
    Ok(ew)
}

#[derive(serde::Deserialize)]
pub struct CallbackTokenInput {
    token: String,
}

/// `.shard.setCallbackToken`: installs the token the entryway's locus
/// callbacks carry, replacing any. Only a service JWT from the entryway,
/// signed with an accepted verify key, is accepted (never the admin
/// password): `iss` = the entryway DID, `aud` = this shard, `lxm` = this
/// method, single-use `jti`, at most an hour's lifetime.
pub async fn set_callback_token(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<CallbackTokenInput>,
) -> XrpcResult<Json<Value>> {
    use locus_auth::service_jwt;
    const LXM: &str = locus_core::locus_nsid!("shard.setCallbackToken");
    let ew = shard_only(&s)?;
    let now = now_s();
    let refuse = |m: &str| XrpcError::new(StatusCode::UNAUTHORIZED, "AuthenticationRequired", m);
    let token = auth::bearer(&headers)?;
    let jwt = service_jwt::parse(token, Some(LXM), now)
        .map_err(|e| XrpcError::new(StatusCode::UNAUTHORIZED, e.name(), e.to_string()))?;
    if jwt.iss != ew.did {
        return Err(refuse("Untrusted issuer"));
    }
    if jwt.aud != s.service_did {
        return Err(refuse("jwt audience does not match service did"));
    }
    if jwt.exp - now > 3600 + 60 {
        return Err(refuse("jwt lifetime too long"));
    }
    if !ew.verify_key_ids().iter().any(|k| jwt.verifies_with(k)) {
        return Err(refuse("jwt signature does not match the entryway's key"));
    }
    let jti = jwt.jti.as_deref().ok_or_else(|| refuse("jwt has no jti"))?;
    let ttl = (jwt.exp - now + 60).max(1) * 1000;
    if !s
        .unique(&format!("ServiceAuth@{}", jwt.iss), jti, ttl)
        .await?
    {
        return Err(refuse("jwt replayed"));
    }
    if input.token.len() < 32 {
        return Err(XrpcError::invalid_request(
            "The callback token is at least 32 characters",
        ));
    }
    s.store
        .set_callback_token_hash(&callback_token_hash(&input.token))
        .await?;
    tracing::info!(entryway = %ew.did, "shard-callback token installed");
    Ok(Json(json!({})))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokeInput {
    did: String,
    sid: Option<String>,
    not_before: Option<String>,
}

/// `.shard.revokeSessions`: a session (`sid`) or every token issued before
/// `notBefore`, refused on every request until the tokens have expired.
pub async fn revoke_sessions(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<RevokeInput>,
) -> XrpcResult<Json<Value>> {
    callback_auth(&s, &headers).await?;
    if !input.did.starts_with("did:") {
        return Err(XrpcError::invalid_request("did must be a DID"));
    }
    match (
        input.sid.as_deref().filter(|x| !x.is_empty()),
        &input.not_before,
    ) {
        (Some(sid), None) => {
            s.store
                .revoke_shard_session(&input.did, sid, REVOCATION_KEEP_MS)
                .await?;
        }
        (None, Some(t)) => {
            let at = locus_core::time::parse_rfc3339_ms(t)
                .ok_or_else(|| XrpcError::invalid_request("notBefore must be a datetime"))?;
            s.store
                .revoke_shard_account(&input.did, at, REVOCATION_KEEP_MS)
                .await?;
        }
        _ => {
            return Err(XrpcError::invalid_request("Give either sid or notBefore"));
        }
    }
    if let Err(e) = s.store.prune_shard_revocations().await {
        tracing::warn!("couldn't prune shard revocations: {e}");
    }
    Ok(Json(json!({})))
}

#[derive(serde::Deserialize)]
pub struct VerifyKeysInput {
    keys: Vec<String>,
}

/// `.shard.setVerifyKeys`: the entryway's accepted token keys, exactly
/// (a rotation pushes `{old, new}`, then `{new}`). Every node reloads.
pub async fn set_verify_keys(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<VerifyKeysInput>,
) -> XrpcResult<Json<Value>> {
    callback_auth(&s, &headers).await?;
    if input.keys.is_empty() || input.keys.len() > 4 {
        return Err(XrpcError::invalid_request("Give one to four keys"));
    }
    let keys = input
        .keys
        .iter()
        .map(|k| parse_k256_key(k))
        .collect::<Result<Vec<_>, _>>()
        .map_err(XrpcError::invalid_request)?;
    let mut tx = s.store.begin().await?;
    tx.replace_shard_verify_keys(&keys).await?;
    tx.bump_key_generation().await?;
    tx.commit().await?;
    crate::keys::reload_soon(&s);
    Ok(Json(json!({ "keys": keys })))
}

#[derive(serde::Deserialize)]
pub struct BlockInput {
    did: String,
    blocked: bool,
}

/// `.shard.setPrivateTierBlock`: the entryway's local operator block for
/// an account on this shard.
pub async fn set_private_tier_block(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<BlockInput>,
) -> XrpcResult<Json<Value>> {
    callback_auth(&s, &headers).await?;
    s.store
        .account_by_did(&input.did)
        .await?
        .ok_or_else(|| XrpcError::invalid_request("Account not found"))?;
    s.store
        .set_private_tier_blocked(&input.did, input.blocked)
        .await?;
    Ok(Json(json!({})))
}

/// Whether the private-tier block is the entryway's to set (a locus
/// entryway installed a callback token): the shard's own console and API
/// then only show it.
pub async fn block_is_entryways(s: &AppState) -> XrpcResult<bool> {
    Ok(s.entryway.is_some() && s.store.callback_token_hash().await?.is_some())
}
