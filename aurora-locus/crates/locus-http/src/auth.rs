//! Reading credentials from requests.

use axum::http::{HeaderMap, StatusCode, header};
use locus_auth::session::{Authed, SCOPE_ACCESS, SCOPE_APP_PASS, SCOPE_APP_PASS_PRIVILEGED};
use locus_store::accounts::{Account, Status};

use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

/// The bearer token, with the reference's errors: no header is 401
/// `AuthMissing`; another scheme is 400 `InvalidToken`.
pub fn bearer(headers: &HeaderMap) -> XrpcResult<&str> {
    let value = headers
        .get(header::AUTHORIZATION)
        .ok_or_else(|| {
            XrpcError::new(
                StatusCode::UNAUTHORIZED,
                "AuthMissing",
                "Authentication Required",
            )
        })?
        .to_str()
        .map_err(|_| XrpcError::named("InvalidToken", "Unexpected authorization type"))?;
    match value.split_once(' ') {
        Some((scheme, token)) if scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() => {
            Ok(token.trim())
        }
        _ => Err(XrpcError::named(
            "InvalidToken",
            "Unexpected authorization type",
        )),
    }
}

/// The reference's scope groups for legacy access tokens.
pub const FULL: &[&str] = &[SCOPE_ACCESS];
pub const PRIVILEGED: &[&str] = &[SCOPE_ACCESS, SCOPE_APP_PASS_PRIVILEGED];
pub const STANDARD: &[&str] = &[SCOPE_ACCESS, SCOPE_APP_PASS_PRIVILEGED, SCOPE_APP_PASS];

/// A verified access token (legacy session) whose scope is in `scopes`
/// or `extra` (e.g. the takendown scope, on the few endpoints the
/// reference allows it).
pub async fn access_with(
    state: &AppState,
    headers: &HeaderMap,
    scopes: &[&str],
    extra: &[&str],
) -> XrpcResult<Authed> {
    let token = bearer(headers)?;
    let authed = state.sessions.verify_access(token).await?;
    let scope = authed.scope.as_str();
    if scopes.contains(&scope) || extra.contains(&scope) {
        Ok(authed)
    } else {
        Err(XrpcError::named("InvalidToken", "Bad token scope"))
    }
}

/// A service JWT from a user for `lxm`, as the reference's `userServiceAuth`:
/// checked against the issuer's DID document (refreshed once if the
/// signature doesn't match), with this PDS as the audience. Returns the
/// issuer's DID.
pub async fn user_service_auth(
    state: &AppState,
    headers: &HeaderMap,
    lxm: &str,
) -> XrpcResult<String> {
    use locus_auth::service_jwt;
    let token = bearer(headers)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let unauthorized = |e: service_jwt::ServiceJwtError| {
        XrpcError::new(StatusCode::UNAUTHORIZED, e.name(), e.to_string())
    };
    let jwt = service_jwt::parse(token, Some(lxm), now).map_err(unauthorized)?;
    let (did, key_id) = jwt.issuer_key_ref();
    for force in [false, true] {
        let Some(doc) = state.resolve_did_doc(did, force).await? else {
            return Err(XrpcError::auth_required("could not resolve iss did"));
        };
        let key = proto_blue_common::did_doc::get_verification_material(&doc, key_id)
            .map(|k| format!("did:key:{}", k.public_key_multibase));
        if key.is_some_and(|k| jwt.verifies_with(&k)) {
            if !state.accepts_aud(&jwt.aud) {
                return Err(unauthorized(service_jwt::ServiceJwtError::BadJwtAudience));
            }
            return Ok(did.to_owned());
        }
    }
    Err(unauthorized(service_jwt::ServiceJwtError::BadJwtSignature))
}

/// A verified access token with a standard scope (full access or an app password).
pub async fn access(state: &AppState, headers: &HeaderMap) -> XrpcResult<Authed> {
    access_with(state, headers, STANDARD, &[]).await
}

/// The authenticated account, as the reference's `checkTakedown` (and,
/// with `deactivated`, `checkDeactivated`): a taken-down or suspended
/// account is refused with `AccountTakedown`.
pub async fn account_checked(
    state: &AppState,
    did: &str,
    deactivated: bool,
) -> XrpcResult<Account> {
    let account = state
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
    match account.status {
        Status::Takendown | Status::Suspended => Err(XrpcError::new(
            StatusCode::UNAUTHORIZED,
            "AccountTakedown",
            "Account has been taken down",
        )),
        Status::Deactivated if deactivated => Err(XrpcError::new(
            StatusCode::UNAUTHORIZED,
            "AccountDeactivated",
            "Account is deactivated",
        )),
        _ => Ok(account),
    }
}

/// A verified full-access token: no app passwords (as the reference's
/// `ACCESS_FULL` endpoints).
pub async fn full_access(state: &AppState, headers: &HeaderMap) -> XrpcResult<Authed> {
    access_with(state, headers, FULL, &[]).await
}

// ---------------------------------------------------------------- OAuth

use locus_auth::oauth::OAuthError;
use locus_auth::scopes::Permissions;

/// The scheme of the `Authorization` header (lowercased), if any.
fn scheme(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    Some(v.split(' ').next().unwrap_or("").to_ascii_lowercase())
}

/// An endpoint OAuth tokens may not use (as the reference's
/// `ForbiddenError` in an `authorize` callback).
pub fn forbid(message: &'static str) -> impl Fn(&Permissions) -> XrpcResult<()> {
    move |_| Err(XrpcError::new(StatusCode::FORBIDDEN, "Forbidden", message))
}

pub const OAUTH_UNSUPPORTED: &str = "OAuth credentials are not supported for this endpoint";

/// Any OAuth token with `atproto`.
pub fn any_scope(_: &Permissions) -> XrpcResult<()> {
    Ok(())
}

/// Legacy access tokens (with `scopes` or `extra`) or OAuth access tokens,
/// as the reference's `authorization()`: `check` is the endpoint's OAuth
/// permission check (legacy tokens skip it).
pub async fn authorization(
    state: &AppState,
    headers: &HeaderMap,
    scopes: &[&str],
    extra: &[&str],
    check: impl Fn(&Permissions) -> XrpcResult<()>,
) -> XrpcResult<Authed> {
    match scheme(headers).as_deref() {
        Some("dpop") => {
            let authed = oauth_access(state, headers).await?;
            if let Some(g) = &authed.oauth {
                check(&g.permissions())?;
            }
            Ok(authed)
        }
        _ => access_with(state, headers, scopes, extra).await,
    }
}

/// Verifies a DPoP-bound OAuth access token, statefully (the grant row is
/// checked on every request, so a revoked token stops at once).
pub async fn oauth_access(state: &AppState, headers: &HeaderMap) -> XrpcResult<Authed> {
    use locus_auth::session::{OAuthGrant, SCOPE_OAUTH};
    use serde_json::Value;
    let invalid = |m: &str| XrpcError::from(OAuthError::invalid_token("DPoP", m));
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' ').map(|(_, t)| t.trim().to_owned()))
        .filter(|t| !t.is_empty())
        .ok_or_else(|| invalid("Malformed token"))?;
    let jws =
        locus_auth::oauth::jose::Jws::decode(&token).map_err(|_| invalid("Malformed token"))?;
    state
        .secrets
        .oauth_key
        .verify(&jws)
        .map_err(|_| invalid("Invalid token"))?;
    let p = &jws.payload;
    // Tokens minted before a hostname switch carry the old issuer until
    // they expire.
    let iss_ok = p.get("iss").and_then(Value::as_str).is_some_and(|i| {
        i == crate::oauth::issuer(state)
            || state
                .hosts()
                .old_full
                .iter()
                .any(|h| i == format!("https://{h}"))
    });
    if !jws.typ_is("at+jwt") || !iss_ok {
        return Err(invalid("Invalid token"));
    }
    if !p
        .get("aud")
        .and_then(Value::as_str)
        .is_some_and(|a| state.accepts_aud(a))
    {
        return Err(invalid(&format!(
            "Invalid audience (got: {}, expected one of: {})",
            p.get("aud").and_then(Value::as_str).unwrap_or(""),
            state.service_did
        )));
    }
    let now = locus_core::time::now_ms();
    if p.get("exp")
        .and_then(Value::as_i64)
        .is_none_or(|e| e * 1000 <= now)
    {
        return Err(invalid("Token expired"));
    }
    let info = crate::routing::REQUEST
        .try_with(Clone::clone)
        .map_err(|_| XrpcError::internal("no request context"))?;
    let proof = crate::oauth::dpop_proof(state, &info.method, &info.uri, headers, Some(&token))
        .await
        .map_err(XrpcError::from)?;
    let jkt = p
        .get("cnf")
        .and_then(|c| c.get("jkt"))
        .and_then(Value::as_str);
    match (jkt, &proof) {
        (Some(_), None) => return Err(OAuthError::invalid_dpop_proof("DPoP proof required").into()),
        (Some(j), Some(pr)) if pr.jkt != j => {
            return Err(OAuthError::invalid_dpop_key_binding().into());
        }
        (None, _) => {
            return Err(OAuthError::invalid_token(
                "Bearer",
                "Bearer token type must be used without a DPoP proof",
            )
            .into());
        }
        _ => {}
    }
    let jti = p.get("jti").and_then(Value::as_str).unwrap_or("");
    let sub = p.get("sub").and_then(Value::as_str).unwrap_or("");
    let row = state
        .store
        .oauth_token(jti)
        .await?
        .ok_or_else(|| invalid("Invalid token"))?;
    let params: locus_auth::oauth::request::Parameters =
        serde_json::from_str(&row.parameters).unwrap_or_default();
    if params.dpop_jkt.as_deref() != jkt || row.did != sub {
        state.store.oauth_delete_token(row.id).await?;
        return Err(invalid("Invalid token"));
    }
    if row.expires_at < now {
        state.store.oauth_delete_token(row.id).await?;
        return Err(invalid("Token expired"));
    }
    let scope = row.scope.clone().or(params.scope).unwrap_or_default();
    if !scope.split(' ').any(|s| s == "atproto") {
        return Err(XrpcError::named(
            "InvalidToken",
            "OAuth token does not have \"atproto\" scope",
        ));
    }
    let account = state
        .store
        .account_by_did(&row.did)
        .await?
        .filter(|a| a.status != Status::Deleted)
        .ok_or_else(|| {
            XrpcError::new(
                StatusCode::FORBIDDEN,
                "AccountNotFound",
                "Account not found",
            )
        })?;
    if matches!(account.status, Status::Takendown | Status::Suspended) {
        return Err(XrpcError::new(
            StatusCode::UNAUTHORIZED,
            "AccountTakedown",
            "Account has been taken down",
        ));
    }
    Ok(Authed {
        did: row.did,
        scope: SCOPE_OAUTH.to_owned(),
        session_id: row.token_id.clone(),
        app_password_name: None,
        status: account.status,
        oauth: Some(OAuthGrant {
            client_id: row.client_id,
            token_id: row.token_id,
            scope,
        }),
    })
}

/// Fails an OAuth request lacking a permission (legacy requests pass).
pub fn require(
    authed: &Authed,
    check: impl Fn(&Permissions) -> Result<(), locus_auth::scopes::ScopeMissing>,
) -> XrpcResult<()> {
    if let Some(g) = &authed.oauth {
        check(&g.permissions())?;
    }
    Ok(())
}
