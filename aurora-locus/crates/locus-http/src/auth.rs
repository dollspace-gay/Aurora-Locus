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
