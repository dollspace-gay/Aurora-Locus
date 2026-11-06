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

/// The entryway role's token keys: tokens are ES256K, access tokens
/// addressed to the account's shard.
pub struct EntrywayKeys {
    pub signing: proto_blue_crypto::K256Keypair,
    /// Keys whose tokens are accepted (the signing key, and during a
    /// rotation the other one).
    pub accepted: Vec<crate::oauth::jose::PublicJwk>,
}

pub struct LegacySessions {
    store: Store,
    entryway: Option<EntrywayKeys>,
    secret: Zeroizing<Vec<u8>>,
    service_did: String,
    /// Other audiences accepted (old hostnames' service DIDs).
    also_accepted: Vec<String>,
    /// When the install was imported from a reference PDS (seconds): its
    /// access tokens, which name no session, are accepted if issued before.
    imported_at_s: Option<i64>,
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
            entryway: None,
            secret,
            service_did,
            also_accepted: Vec::new(),
            imported_at_s: None,
            access_ttl_s: 120 * 60,
            refresh_ttl_s: 90 * 24 * 60 * 60,
        }
    }

    /// In the entryway role: tokens are ES256K with these keys.
    #[must_use]
    pub fn with_entryway(mut self, keys: EntrywayKeys) -> Self {
        self.entryway = Some(keys);
        self
    }

    /// Whether a JWS verifies with an accepted entryway key (the entryway's
    /// own OAuth tokens).
    #[must_use]
    pub fn entryway_verifies(&self, jws: &crate::oauth::jose::Jws) -> bool {
        self.entryway.as_ref().is_some_and(|e| {
            jws.alg() == "ES256K"
                && e.accepted.iter().any(|k| {
                    k.verify("ES256K", &jws.signing_input, &jws.signature)
                        .is_ok()
                })
        })
    }

    /// Whether this is the entryway role.
    #[must_use]
    pub fn is_entryway(&self) -> bool {
        self.entryway.is_some()
    }

    fn sign(&self, typ: &str, claims: &Claims) -> String {
        match &self.entryway {
            Some(e) => jwt::sign_es256k(&e.signing, typ, claims),
            None => jwt::sign(&self.secret, typ, claims),
        }
    }

    fn verify(&self, typ: &str, token: &str, now: i64) -> Result<Claims, JwtError> {
        match &self.entryway {
            Some(e) => jwt::verify_es256k(&e.accepted, typ, token, now),
            None => jwt::verify(&self.secret, typ, token, now),
        }
    }

    /// Where an account's access tokens are addressed: this PDS, or in the
    /// entryway role the account's shard.
    async fn access_aud(&self, did: &str) -> String {
        if self.entryway.is_some()
            && let Ok(Some(aud)) = self.store.account_shard_did(did).await
        {
            return aud;
        }
        self.service_did.clone()
    }

    fn access_jwt(
        &self,
        did: &str,
        scope: &str,
        session_id: &str,
        now: i64,
        aud: String,
    ) -> String {
        // An entryway token names its session in `sid` (shards revoke by
        // it) and gets its own `jti`.
        let (jti, sid) = if self.entryway.is_some() {
            (random_id(), Some(session_id.to_owned()))
        } else {
            (session_id.to_owned(), None)
        };
        self.sign(
            TYP_ACCESS,
            &Claims {
                scope: scope.to_owned(),
                aud,
                sub: did.to_owned(),
                iat: now,
                exp: now + self.access_ttl_s,
                jti: Some(jti),
                cnf: None,
                lxm: None,
                sid,
            },
        )
    }

    fn refresh_jwt(&self, did: &str, refresh_id: &str, now: i64, exp: i64) -> String {
        self.sign(
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
                sid: None,
            },
        )
    }

    /// A fresh access token for an existing session (the account portal
    /// signs its own calls with one; it holds no refresh token).
    #[must_use]
    pub fn mint_access(&self, did: &str, scope: &str, session_id: &str) -> String {
        self.access_jwt(did, scope, session_id, now_s(), self.service_did.clone())
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
        let aud = self.access_aud(did).await;
        Ok(Tokens {
            access_jwt: self.access_jwt(did, scope, &session_id, now, aud),
            refresh_jwt,
            session_id,
        })
    }

    /// Also accepts tokens whose `aud` is one of `auds` (a hostname
    /// migration's old service DIDs).
    #[must_use]
    pub fn accepting(mut self, auds: Vec<String>) -> Self {
        self.also_accepted = auds;
        self
    }

    /// Also accepts the access tokens a reference PDS issued before the
    /// install was imported from it (`imported_at_ms`), until they expire.
    #[must_use]
    pub fn accepting_imported(mut self, imported_at_ms: Option<i64>) -> Self {
        self.imported_at_s = imported_at_ms.map(|t| t / 1000);
        self
    }

    /// When the install was imported from a reference PDS (ms), if it was.
    #[must_use]
    pub fn imported_at_ms(&self) -> Option<i64> {
        self.imported_at_s.map(|t| t * 1000)
    }

    /// The legacy JWT secret (the reference's, after an import).
    #[must_use]
    pub fn secret(&self) -> &[u8] {
        &self.secret
    }

    fn aud_ok(&self, aud: &str) -> bool {
        aud == self.service_did || self.also_accepted.iter().any(|a| a == aud)
    }

    /// Verifies an access token against its session and the account.
    pub async fn verify_access(&self, token: &str) -> Result<Authed, AuthError> {
        let claims = self.verify(TYP_ACCESS, token, now_s())?;
        // An entryway's tokens are addressed to the account's shard; it
        // takes them too when a client calls it directly.
        let aud_ok = self.aud_ok(&claims.aud)
            || (self.entryway.is_some() && self.access_aud(&claims.sub).await == claims.aud);
        if claims.cnf.is_some() || claims.lxm.is_some() || !aud_ok {
            return Err(AuthError::InvalidToken);
        }
        if !ACCESS_SCOPES.contains(&claims.scope.as_str()) || !claims.sub.starts_with("did:") {
            return Err(AuthError::InvalidToken);
        }
        let named = if self.entryway.is_some() {
            claims.sid.clone()
        } else {
            claims.jti.clone()
        };
        let Some(session_id) = named else {
            return self.verify_imported(claims).await;
        };
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

    /// An access token a reference PDS issued before the import (it names
    /// no session): the account's status is checked, but there's no
    /// session to revoke until it expires (at most the access lifetime).
    async fn verify_imported(&self, claims: Claims) -> Result<Authed, AuthError> {
        if self.imported_at_s.is_none_or(|t| claims.iat > t) {
            return Err(AuthError::InvalidToken);
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
            session_id: String::new(),
            app_password_name: None,
            status: account.status,
            oauth: None,
        })
    }

    /// Rotates a refresh token. A replay of a used token within the grace
    /// period returns the same next refresh token.
    pub async fn refresh(&self, refresh_jwt: &str) -> Result<(Tokens, String), AuthError> {
        let now = now_s();
        let claims = self.verify(TYP_REFRESH, refresh_jwt, now)?;
        if !self.aud_ok(&claims.aud) || claims.scope != SCOPE_REFRESH {
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
        let aud = self.access_aud(&session.did).await;
        Ok((
            Tokens {
                access_jwt: self.access_jwt(&session.did, &session.scope, &session.id, now, aud),
                refresh_jwt: Some(self.refresh_jwt(&session.did, &next_id, now, next_exp)),
                session_id: session.id,
            },
            session.did,
        ))
    }

    /// Ends the session a refresh token belongs to (an expired token is
    /// accepted, as the reference).
    pub async fn delete(&self, refresh_jwt: &str) -> Result<(), AuthError> {
        let claims = match self.verify(TYP_REFRESH, refresh_jwt, now_s()) {
            Ok(c) => c,
            Err(JwtError::Expired) => self.verify(TYP_REFRESH, refresh_jwt, i64::MIN)?,
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
