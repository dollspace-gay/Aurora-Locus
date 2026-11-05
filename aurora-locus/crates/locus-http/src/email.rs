//! Emailed tokens and the endpoints built on them: email confirmation and
//! change, password reset, and the sign-in second factor.
//!
//! Tokens are `XXXXX-XXXXX`, valid 15 minutes, one per account and purpose,
//! stored as a keyed hash. When email isn't configured these endpoints say
//! so, rather than pretending to send.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use hmac::{Hmac, Mac};
use locus_auth::password;
use locus_mail::{MailError, Template};
use locus_store::accounts::SessionFilter;
use locus_store::email::{Sender, TokenPurpose};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Sha256;

use crate::auth;
use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

type St = State<Arc<AppState>>;

const TOKEN_TTL_MS: i64 = 15 * 60 * 1000;

/// A new token: 10 random base32 characters, uppercase, as `XXXXX-XXXXX`.
#[must_use]
pub fn new_token() -> String {
    let raw = locus_identity::plc::base32_lower(&rand::random::<[u8; 8]>()).to_ascii_uppercase();
    format!("{}-{}", &raw[0..5], &raw[5..10])
}

fn token_hash(s: &AppState, token: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(&s.secrets.email_token_pepper).expect("any key length");
    mac.update(token.trim().to_ascii_uppercase().as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn not_configured() -> XrpcError {
    XrpcError::invalid_request("Email is not configured on this server")
}

/// Refuses early when the main sender can't send.
pub async fn require_email(s: &AppState) -> XrpcResult<()> {
    if s.mailer.configured(Sender::Main).await? {
        Ok(())
    } else {
        Err(not_configured())
    }
}

/// Issues a token for (did, purpose), replacing any earlier one.
pub async fn issue(s: &AppState, did: &str, purpose: TokenPurpose) -> XrpcResult<String> {
    let token = new_token();
    s.store
        .put_email_token(did, purpose, &token_hash(s, &token))
        .await?;
    Ok(token)
}

/// Checks a token for (did, purpose), as the reference's
/// `assertValidToken`: `InvalidToken` or `ExpiredToken`.
pub async fn check(s: &AppState, did: &str, purpose: TokenPurpose, token: &str) -> XrpcResult<()> {
    let given = token_hash(s, token);
    match s.store.email_token(did, purpose).await? {
        Some((stored, age)) if constant_time_eq(stored.as_bytes(), given.as_bytes()) => {
            if age > TOKEN_TTL_MS {
                Err(XrpcError::named("ExpiredToken", "Token is expired"))
            } else {
                Ok(())
            }
        }
        _ => Err(XrpcError::named("InvalidToken", "Token is invalid")),
    }
}

/// The account a token was issued to (for password reset, where the
/// request names no account).
pub async fn owner(s: &AppState, purpose: TokenPurpose, token: &str) -> XrpcResult<String> {
    match s
        .store
        .email_token_owner(purpose, &token_hash(s, token))
        .await?
    {
        Some((_, age)) if age > TOKEN_TTL_MS => {
            Err(XrpcError::named("ExpiredToken", "Token is expired"))
        }
        Some((did, _)) => Ok(did),
        None => Err(XrpcError::named("InvalidToken", "Token is invalid")),
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Sends a templated account email.
pub async fn send(
    s: &AppState,
    template: Template,
    to: &str,
    token: &str,
    handle: &str,
) -> XrpcResult<()> {
    let snap = s.snapshot();
    let service = match snap.text("server.service_name") {
        "" => format!("{} PDS", s.hostname),
        name => name.to_owned(),
    };
    let from = match snap.text("email.from_address") {
        "" => format!("noreply@{}", s.hostname),
        f => f.to_owned(),
    };
    let email = template.render(&[("service", &service), ("token", token), ("handle", handle)]);
    match s.mailer.send(Sender::Main, &from, to, email).await {
        Ok(()) => Ok(()),
        Err(MailError::NotConfigured) => Err(not_configured()),
        Err(MailError::Address(m)) => Err(XrpcError::invalid_request(m)),
        Err(MailError::Send(_)) => Err(XrpcError::internal(
            "Couldn't send the email; try again later",
        )),
    }
}

pub async fn request_email_confirmation(State(s): St, headers: HeaderMap) -> XrpcResult<()> {
    let authed = auth::access(&s, &headers).await?;
    let account = auth::account_checked(&s, &authed.did, false).await?;
    let email = account
        .email
        .ok_or_else(|| XrpcError::invalid_request("account does not have an email address"))?;
    require_email(&s).await?;
    let token = issue(&s, &account.did, TokenPurpose::ConfirmEmail).await?;
    send(&s, Template::ConfirmEmail, &email, &token, &account.handle).await
}

#[derive(Deserialize)]
pub struct ConfirmEmailInput {
    email: String,
    token: String,
}

pub async fn confirm_email(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<ConfirmEmailInput>,
) -> XrpcResult<()> {
    let authed = auth::access(&s, &headers).await?;
    let account = auth::account_checked(&s, &authed.did, false).await?;
    if account.email.as_deref() != Some(input.email.trim().to_ascii_lowercase().as_str()) {
        return Err(XrpcError::named("InvalidEmail", "invalid email"));
    }
    check(&s, &account.did, TokenPurpose::ConfirmEmail, &input.token).await?;
    let mut tx = s.store.begin().await?;
    tx.delete_email_token(&account.did, TokenPurpose::ConfirmEmail)
        .await?;
    tx.set_email_confirmed(&account.did).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn request_email_update(State(s): St, headers: HeaderMap) -> XrpcResult<Json<Value>> {
    let authed = auth::full_access(&s, &headers).await?;
    let account = auth::account_checked(&s, &authed.did, false).await?;
    let email = account
        .email
        .ok_or_else(|| XrpcError::invalid_request("account does not have an email address"))?;
    // A confirmed address must approve the change by email.
    let token_required = account.email_confirmed_at.is_some();
    if token_required {
        require_email(&s).await?;
        let token = issue(&s, &account.did, TokenPurpose::UpdateEmail).await?;
        send(&s, Template::UpdateEmail, &email, &token, &account.handle).await?;
    }
    Ok(Json(json!({ "tokenRequired": token_required })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateEmailInput {
    email: String,
    email_auth_factor: Option<bool>,
    token: Option<String>,
}

pub async fn update_email(
    State(s): St,
    headers: HeaderMap,
    Json(input): Json<UpdateEmailInput>,
) -> XrpcResult<()> {
    let authed = auth::full_access(&s, &headers).await?;
    let account = auth::account_checked(&s, &authed.did, false).await?;
    let did = account.did.as_str();
    let email = input.email.trim().to_ascii_lowercase();
    let same_email = account.email.as_deref() == Some(email.as_str());
    let token = input.token.as_deref().filter(|t| !t.is_empty());

    // Turning the sign-in second factor on or off, as the reference.
    match input.email_auth_factor {
        Some(true) if same_email && account.email_confirmed_at.is_some() => {
            if account.email_auth_factor_at.is_none() {
                let mut tx = s.store.begin().await?;
                let changed = tx.set_email_auth_factor(did, &email, true).await?;
                tx.commit().await?;
                if !changed {
                    return Err(XrpcError::invalid_request(
                        "A confirmed email address is required to enable email-based two-factor authentication",
                    ));
                }
            }
            return Ok(());
        }
        Some(true) => {
            return Err(XrpcError::invalid_request(
                "Please change and verify your email before enabling OTP",
            ));
        }
        Some(false) if same_email => {
            if account.email_auth_factor_at.is_none() {
                return Ok(());
            }
            // Turning a factor off needs proof of the inbox.
            let Some(token) = token else {
                require_email(&s).await?;
                let otp = issue(&s, did, TokenPurpose::UpdateEmail).await?;
                send(&s, Template::UpdateEmail, &email, &otp, &account.handle).await?;
                return Err(XrpcError::named(
                    "TokenRequired",
                    "confirmation token required",
                ));
            };
            check(&s, did, TokenPurpose::UpdateEmail, token).await?;
            let mut tx = s.store.begin().await?;
            tx.delete_email_token(did, TokenPurpose::UpdateEmail)
                .await?;
            tx.set_email_auth_factor(did, &email, false).await?;
            tx.commit().await?;
            return Ok(());
        }
        // A new address turns the factor off anyway.
        Some(false) | None => {}
    }

    crate::server::check_email(&email)?;
    if let Some(token) = token {
        check(&s, did, TokenPurpose::UpdateEmail, token).await?;
    } else if account.email_confirmed_at.is_some() {
        return Err(XrpcError::named(
            "TokenRequired",
            "confirmation token required",
        ));
    }
    let mut tx = s.store.begin().await?;
    tx.set_email(did, &email).await?;
    tx.commit().await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct RequestPasswordResetInput {
    email: String,
}

/// **[deviation]** Answers the same whether or not the address belongs to
/// an account (the reference says when it doesn't).
pub async fn request_password_reset(
    State(s): St,
    Json(input): Json<RequestPasswordResetInput>,
) -> XrpcResult<()> {
    require_email(&s).await?;
    let email = input.email.trim().to_ascii_lowercase();
    if let Some(account) = s.store.account_by_email(&email).await?
        && account.status != locus_store::accounts::Status::Deleted
    {
        let token = issue(&s, &account.did, TokenPurpose::ResetPassword).await?;
        send(&s, Template::ResetPassword, &email, &token, &account.handle).await?;
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct ResetPasswordInput {
    token: String,
    password: String,
}

pub async fn reset_password(State(s): St, Json(input): Json<ResetPasswordInput>) -> XrpcResult<()> {
    if input.password.len() > password::MAX_NEW_PASSWORD {
        return Err(XrpcError::invalid_request("Invalid password length."));
    }
    let did = owner(&s, TokenPurpose::ResetPassword, &input.token).await?;
    let hash = password::hash(&input.password).map_err(XrpcError::internal)?;
    let mut tx = s.store.begin().await?;
    tx.set_password_hash(&did, &hash).await?;
    tx.delete_email_token(&did, TokenPurpose::ResetPassword)
        .await?;
    // Every session ends, app-password ones included.
    tx.revoke_sessions(&did, SessionFilter::All).await?;
    tx.commit().await?;
    Ok(())
}

/// The sign-in second factor: with a code, checks and spends it; without
/// one, when the account uses email codes and the login wasn't an app
/// password, emails a code and asks for it.
pub async fn sign_in_factor(
    s: &AppState,
    account: &locus_store::accounts::Account,
    app_password: bool,
    code: Option<&str>,
) -> XrpcResult<()> {
    if let Some(code) = code.filter(|c| !c.is_empty()) {
        check(s, &account.did, TokenPurpose::AuthFactor, code).await?;
        let mut tx = s.store.begin().await?;
        tx.delete_email_token(&account.did, TokenPurpose::AuthFactor)
            .await?;
        tx.commit().await?;
        return Ok(());
    }
    let (Some(email), Some(_)) = (&account.email, account.email_auth_factor_at) else {
        return Ok(());
    };
    if app_password {
        return Ok(());
    }
    let required = |m: &str| XrpcError::new(StatusCode::UNAUTHORIZED, "AuthFactorTokenRequired", m);
    if !s.mailer.configured(Sender::Main).await? {
        // The operator can clear the factor from the console.
        return Err(required(
            "A sign-in code is required, but email is not available on this server; contact its operator",
        ));
    }
    let token = issue(s, &account.did, TokenPurpose::AuthFactor).await?;
    send(s, Template::SignInCode, email, &token, &account.handle).await?;
    Err(required(
        "A sign in code has been sent to your email address",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_look_like_the_reference() {
        for _ in 0..50 {
            let t = new_token();
            assert_eq!(t.len(), 11);
            assert_eq!(&t[5..6], "-");
            assert!(
                t.chars()
                    .all(|c| c == '-' || c.is_ascii_uppercase() || ('2'..='7').contains(&c)),
                "{t}"
            );
        }
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
