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
            if jwt.aud != state.service_did {
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
