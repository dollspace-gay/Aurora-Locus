//! Reading credentials from requests.

use axum::http::{HeaderMap, StatusCode, header};
use locus_auth::session::{Authed, SCOPE_ACCESS};
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

/// A verified access token (legacy session).
pub async fn access(state: &AppState, headers: &HeaderMap) -> XrpcResult<Authed> {
    let token = bearer(headers)?;
    Ok(state.sessions.verify_access(token).await?)
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
    let authed = access(state, headers).await?;
    if authed.scope != SCOPE_ACCESS {
        return Err(XrpcError::named("InvalidToken", "Bad token scope"));
    }
    Ok(authed)
}
