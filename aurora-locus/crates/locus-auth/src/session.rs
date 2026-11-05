//! Legacy sessions (createSession / refreshSession / deleteSession).
//!
//! Unlike the reference, sessions are stateful: an access token carries its
//! session id, and every request checks that session and the account's
//! status, so logout, password reset, app-password revocation and takedown
//! take effect on the next request. Refresh rotation keeps the reference's
//! 2-hour replay grace: replaying a used refresh token within it returns the
//! same next token.

use std::time::{SystemTime, UNIX_EPOCH};

use locus_core::CoreError;
use locus_store::Store;
use locus_store::accounts::{RefreshToken, Session, SessionFilter, Status};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::jwt::{self, Claims, JwtError, TYP_ACCESS, TYP_REFRESH};

pub const SCOPE_ACCESS: &str = "com.atproto.access";
pub const SCOPE_APP_PASS_PRIVILEGED: &str = "com.atproto.appPassPrivileged";
pub const SCOPE_APP_PASS: &str = "com.atproto.appPass";
pub const SCOPE_TAKENDOWN: &str = "com.atproto.takendown";
pub const SCOPE_SIGNUP_QUEUED: &str = "com.atproto.signupQueued";
pub const SCOPE_REFRESH: &str = "com.atproto.refresh";

const ACCESS_SCOPES: &[&str] = &[
    SCOPE_ACCESS,
    SCOPE_APP_PASS_PRIVILEGED,
    SCOPE_APP_PASS,
    SCOPE_TAKENDOWN,
    SCOPE_SIGNUP_QUEUED,
];

/// Replaying a used refresh token within this window returns the same next token.
pub const REFRESH_GRACE_S: i64 = 2 * 60 * 60;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthError {
    /// The token is malformed, has the wrong type or audience, or its session is gone.
    #[error("Token could not be verified")]
    InvalidToken,
    #[error("Token has expired")]
    ExpiredToken,
    #[error("Token has been revoked")]
    RevokedToken,
    #[error("Account not found")]
    AccountNotFound,
    #[error("Account has been taken down")]
    AccountTakedown,
    #[error("{0}")]
    Storage(String),
}

impl From<CoreError> for AuthError {
    fn from(e: CoreError) -> Self {
        Self::Storage(e.to_string())
    }
}

impl From<JwtError> for AuthError {
    fn from(e: JwtError) -> Self {
        match e {
            JwtError::Expired => Self::ExpiredToken,
            _ => Self::InvalidToken,
        }
    }
}

/// A verified legacy access token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authed {
    pub did: String,
    pub scope: String,
    pub session_id: String,
    pub app_password_name: Option<String>,
    pub status: Status,
    /// Set for an OAuth access token (then `scope` is [`SCOPE_OAUTH`]).
    pub oauth: Option<OAuthGrant>,
}

/// The `scope` of an OAuth-authenticated request (not a legacy scope, so
/// the legacy scope groups never match it).
pub const SCOPE_OAUTH: &str = "oauth";

/// An OAuth grant behind a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthGrant {
    pub client_id: String,
    pub token_id: String,
    /// The token's scope (`include:` already expanded).
    pub scope: String,
}

impl OAuthGrant {
    #[must_use]
    pub fn permissions(&self) -> crate::scopes::Permissions {
        crate::scopes::Permissions::new(&self.scope)
    }
}

/// A freshly issued token pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access_jwt: String,
    /// None when no refresh token is stored (a taken-down account's login).
    pub refresh_jwt: Option<String>,
    pub session_id: String,
}

pub struct LegacySessions {
    store: Store,
    secret: Zeroizing<Vec<u8>>,
    service_did: String,
    pub access_ttl_s: i64,
    pub refresh_ttl_s: i64,
}

fn now_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn random_id() -> String {
    hex::encode(rand::random::<[u8; 16]>())
}

impl LegacySessions {
    pub fn new(store: Store, secret: Zeroizing<Vec<u8>>, service_did: String) -> Self {
        Self {
            store,
            secret,
            service_did,
            access_ttl_s: 120 * 60,
            refresh_ttl_s: 90 * 24 * 60 * 60,
        }
    }

    fn access_jwt(&self, did: &str, scope: &str, session_id: &str, now: i64) -> String {
        jwt::sign(
            &self.secret,
            TYP_ACCESS,
            &Claims {
                scope: scope.to_owned(),
                aud: self.service_did.clone(),
                sub: did.to_owned(),
                iat: now,
                exp: now + self.access_ttl_s,
                jti: Some(session_id.to_owned()),
                cnf: None,
                lxm: None,
            },
        )
    }

    fn refresh_jwt(&self, did: &str, refresh_id: &str, now: i64, exp: i64) -> String {
        jwt::sign(
            &self.secret,
            TYP_REFRESH,
            &Claims {
                scope: SCOPE_REFRESH.to_owned(),
                aud: self.service_did.clone(),
                sub: did.to_owned(),
                iat: now,
                exp,
                jti: Some(refresh_id.to_owned()),
                cnf: None,
                lxm: None,
            },
        )
    }

    /// Opens a session. `store_refresh` is false for a taken-down account's
    /// login (it gets an access token only, as the reference).
    pub async fn create(
        &self,
        did: &str,
        scope: &str,
        app_password_name: Option<&str>,
        store_refresh: bool,
    ) -> Result<Tokens, AuthError> {
        let mut tx = self.store.begin().await?;
        let tokens = self
            .create_in(&mut tx, did, scope, app_password_name, store_refresh)
            .await?;
        tx.commit().await?;
        Ok(tokens)
    }

    /// Opens a session inside the caller's transaction (account creation
    /// writes the account and its first session together).
    pub async fn create_in(
        &self,
        tx: &mut locus_store::Tx,
        did: &str,
        scope: &str,
        app_password_name: Option<&str>,
        store_refresh: bool,
    ) -> Result<Tokens, AuthError> {
        let now = now_s();
        let session_id = random_id();
        let refresh_exp = now + self.refresh_ttl_s;
        tx.insert_session(&Session {
            id: session_id.clone(),
            did: did.to_owned(),
            kind: "legacy".into(),
            scope: scope.to_owned(),
            app_password_name: app_password_name.map(str::to_owned),
            created_at: now * 1000,
            expires_at: refresh_exp * 1000,
            revoked_at: None,
        })
        .await?;
        let refresh_jwt = if store_refresh {
            let refresh_id = random_id();
            tx.insert_refresh_token(&RefreshToken {
                id: refresh_id.clone(),
                session_id: session_id.clone(),
                expires_at: refresh_exp * 1000,
                next_id: None,
            })
            .await?;
            Some(self.refresh_jwt(did, &refresh_id, now, refresh_exp))
        } else {
            None
        };
        Ok(Tokens {
            access_jwt: self.access_jwt(did, scope, &session_id, now),
            refresh_jwt,
            session_id,
        })
    }

    /// Verifies an access token against its session and the account.
    pub async fn verify_access(&self, token: &str) -> Result<Authed, AuthError> {
        let claims = jwt::verify(&self.secret, TYP_ACCESS, token, now_s())?;
        if claims.cnf.is_some() || claims.lxm.is_some() || claims.aud != self.service_did {
            return Err(AuthError::InvalidToken);
        }
        if !ACCESS_SCOPES.contains(&claims.scope.as_str()) || !claims.sub.starts_with("did:") {
            return Err(AuthError::InvalidToken);
        }
        let session_id = claims.jti.ok_or(AuthError::InvalidToken)?;
        let session = self
            .store
            .session(&session_id)
            .await?
            .ok_or(AuthError::InvalidToken)?;
        if session.did != claims.sub {
            return Err(AuthError::InvalidToken);
        }
        if session.revoked_at.is_some() {
            return Err(AuthError::RevokedToken);
        }
        let account = self
            .store
            .account_by_did(&claims.sub)
            .await?
            .ok_or(AuthError::AccountNotFound)?;
        match account.status {
            Status::Deleted => return Err(AuthError::AccountNotFound),
            Status::Takendown | Status::Suspended if claims.scope != SCOPE_TAKENDOWN => {
                return Err(AuthError::AccountTakedown);
            }
            _ => {}
        }
        Ok(Authed {
            did: claims.sub,
            scope: claims.scope,
            session_id,
            app_password_name: session.app_password_name,
            status: account.status,
            oauth: None,
        })
    }

    /// Rotates a refresh token. A replay of a used token within the grace
    /// period returns the same next refresh token.
    pub async fn refresh(&self, refresh_jwt: &str) -> Result<(Tokens, String), AuthError> {
        let now = now_s();
        let claims = jwt::verify(&self.secret, TYP_REFRESH, refresh_jwt, now)?;
        if claims.aud != self.service_did || claims.scope != SCOPE_REFRESH {
            return Err(AuthError::InvalidToken);
        }
        let id = claims.jti.ok_or(AuthError::InvalidToken)?;
        let mut tx = self.store.begin().await?;
        let row = tx
            .refresh_token_for_update(&id)
            .await?
            .ok_or(AuthError::ExpiredToken)?;
        if row.expires_at <= now * 1000 {
            return Err(AuthError::ExpiredToken);
        }
        let session = tx
            .session_for_update(&row.session_id)
            .await?
            .ok_or(AuthError::InvalidToken)?;
        if session.revoked_at.is_some() {
            return Err(AuthError::RevokedToken);
        }
        let account = self
            .store
            .account_by_did(&session.did)
            .await?
            .ok_or(AuthError::AccountNotFound)?;
        if matches!(
            account.status,
            Status::Takendown | Status::Suspended | Status::Deleted
        ) {
            return Err(AuthError::AccountTakedown);
        }

        let (next_id, next_exp) = match &row.next_id {
            Some(next) => {
                let next_row = tx
                    .refresh_token_for_update(next)
                    .await?
                    .ok_or(AuthError::ExpiredToken)?;
                (next.clone(), next_row.expires_at / 1000)
            }
            None => {
                let next = random_id();
                let exp = now + self.refresh_ttl_s;
                tx.mark_refresh_used(&id, &next, (now + REFRESH_GRACE_S) * 1000)
                    .await?;
                tx.insert_refresh_token(&RefreshToken {
                    id: next.clone(),
                    session_id: session.id.clone(),
                    expires_at: exp * 1000,
                    next_id: None,
                })
                .await?;
                (next, exp)
            }
        };
        tx.commit().await?;
        Ok((
            Tokens {
                access_jwt: self.access_jwt(&session.did, &session.scope, &session.id, now),
                refresh_jwt: Some(self.refresh_jwt(&session.did, &next_id, now, next_exp)),
                session_id: session.id,
            },
            session.did,
        ))
    }

    /// Ends the session a refresh token belongs to (an expired token is
    /// accepted, as the reference).
    pub async fn delete(&self, refresh_jwt: &str) -> Result<(), AuthError> {
        let claims = match jwt::verify(&self.secret, TYP_REFRESH, refresh_jwt, now_s()) {
            Ok(c) => c,
            Err(JwtError::Expired) => {
                jwt::verify(&self.secret, TYP_REFRESH, refresh_jwt, i64::MIN)?
            }
            Err(e) => return Err(e.into()),
        };
        let id = claims.jti.ok_or(AuthError::InvalidToken)?;
        let mut tx = self.store.begin().await?;
        if let Some(row) = tx.refresh_token_for_update(&id).await? {
            tx.revoke_sessions(&claims.sub, SessionFilter::One(&row.session_id))
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
