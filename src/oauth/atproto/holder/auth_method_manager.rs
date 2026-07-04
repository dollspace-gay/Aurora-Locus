//! Holder auth-method registry (Holder UI Phase 1, chainlink #424; SD-A5 =
//! flexible).
//!
//! Backs the `holder_auth_method` table (sqlite migration `0033` / pg `0034`) —
//! a did-keyed registry of the auth methods a holder registered for the
//! authorization-server / holder-UI login step. SD-A5 offers three method types
//! (`password`, `passkey`, `login_alpha`); Phase 1 ships the **password** path
//! (argon2id via [`crate::auth::PasswordHasher`], the same hashing as did:plc
//! app-passwords) and defers passkey + login-α at the auth layer.
//!
//! This module owns all SQL touching `holder_auth_method`. Phase 1 lands the
//! verify side (this commit); the registration + management side
//! (`register_password`, `remove` with the last-method-remaining safety, …)
//! lands with its consumer — the holder auth-methods page.

use chrono::Utc;
use sqlx::{AnyPool, Row};
use uuid::Uuid;
use webauthn_rs::prelude::Passkey;

use crate::error::{PdsError, PdsResult};

/// The three SD-A5 holder auth-method types. `Passkey` and `LoginAlpha` are
/// substrate-modelled in Phase 1 but deferred at the auth layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethodType {
    Password,
    Passkey,
    LoginAlpha,
}

impl AuthMethodType {
    /// Parse a `method_type` column value.
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "password" => Some(AuthMethodType::Password),
            "passkey" => Some(AuthMethodType::Passkey),
            "login_alpha" => Some(AuthMethodType::LoginAlpha),
            _ => None,
        }
    }

    /// A short human label for the management UI.
    pub fn label(self) -> &'static str {
        match self {
            AuthMethodType::Password => "Password",
            AuthMethodType::Passkey => "Passkey",
            AuthMethodType::LoginAlpha => "Security key (key-signing)",
        }
    }
}

/// A holder auth-method row, projected to the fields the UI + auth layer need
/// (the passkey BLOB columns are not read in Phase 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthMethod {
    pub id: String,
    pub did: String,
    pub method_type: AuthMethodType,
    pub is_primary: bool,
    /// Passkey device label; `None` for other method types.
    pub device_name: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// Registry of holder auth methods.
#[derive(Clone)]
pub struct HolderAuthMethodManager {
    db: AnyPool,
}

impl HolderAuthMethodManager {
    pub fn new(db: AnyPool) -> Self {
        Self { db }
    }

    /// Verify `plaintext` against the holder's registered **password** methods
    /// (did:web). Returns the matched method's `id` on success (so the caller
    /// can [`touch`](Self::touch) it), or `None` if no password method matches.
    ///
    /// did:web only: a did:plc holder's password lives in the legacy
    /// `app_password` table and is verified there, not here. A holder may have
    /// more than one password method registered; each stored argon2id hash is
    /// tried until one matches.
    pub async fn verify_password(&self, did: &str, plaintext: &str) -> PdsResult<Option<String>> {
        let rows = sqlx::query(
            "SELECT id, password_hash FROM holder_auth_method \
             WHERE did = $1 AND method_type = 'password' AND password_hash IS NOT NULL",
        )
        .bind(did)
        .fetch_all(&self.db)
        .await
        .map_err(PdsError::Database)?;

        for row in &rows {
            let hash: String = row.try_get("password_hash").map_err(PdsError::Database)?;
            if let Ok(true) = crate::auth::PasswordHasher::verify(plaintext, &hash) {
                let id: String = row.try_get("id").map_err(PdsError::Database)?;
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    /// Mark a method used, stamping `last_used_at = now`. Best-effort: a missing
    /// `id` is not an error (the method may have been removed concurrently).
    pub async fn touch(&self, id: &str) -> PdsResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE holder_auth_method SET last_used_at = $1 WHERE id = $2")
            .bind(&now)
            .bind(id)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;
        Ok(())
    }

    /// List a holder's registered methods, oldest first (stable render order).
    pub async fn list_for_did(&self, did: &str) -> PdsResult<Vec<AuthMethod>> {
        let rows = sqlx::query(
            "SELECT id, did, method_type, is_primary, passkey_device_name, created_at, last_used_at \
             FROM holder_auth_method WHERE did = $1 ORDER BY created_at ASC",
        )
        .bind(did)
        .fetch_all(&self.db)
        .await
        .map_err(PdsError::Database)?;

        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let mt: String = row.try_get("method_type").map_err(PdsError::Database)?;
            let method_type = AuthMethodType::from_db(&mt).ok_or_else(|| {
                PdsError::Internal(format!("unknown holder auth method_type: {mt}"))
            })?;
            out.push(AuthMethod {
                id: row.try_get("id").map_err(PdsError::Database)?,
                did: row.try_get("did").map_err(PdsError::Database)?,
                method_type,
                is_primary: crate::db::read_bool(row, "is_primary").map_err(PdsError::Database)?,
                device_name: row.try_get("passkey_device_name").map_err(PdsError::Database)?,
                created_at: row.try_get("created_at").map_err(PdsError::Database)?,
                last_used_at: row.try_get("last_used_at").map_err(PdsError::Database)?,
            });
        }
        Ok(out)
    }

    /// Count a holder's registered methods (any type).
    async fn count_for_did(&self, did: &str) -> PdsResult<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS c FROM holder_auth_method WHERE did = $1")
            .bind(did)
            .fetch_one(&self.db)
            .await
            .map_err(PdsError::Database)?;
        row.try_get::<i64, _>("c").map_err(PdsError::Database)
    }

    /// Register an argon2id password method for `did`. The holder's first method
    /// becomes their primary. Returns the new method id.
    pub async fn register_password(&self, did: &str, plaintext: &str) -> PdsResult<String> {
        if plaintext.len() < 8 {
            return Err(PdsError::Validation(
                "password must be at least 8 characters".to_string(),
            ));
        }
        let hash = crate::auth::PasswordHasher::hash(plaintext)?;
        let is_primary = self.count_for_did(did).await? == 0;
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO holder_auth_method \
             (id, did, method_type, is_primary, password_hash, password_algo, created_at) \
             VALUES ($1, $2, 'password', $3, $4, 'argon2id', $5)",
        )
        .bind(&id)
        .bind(did)
        .bind(is_primary)
        .bind(&hash)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;
        Ok(id)
    }

    /// Register a login-α (`#atproto`-key challenge) method for `did`. Carries no
    /// method-specific credential (the key is `did_web_account.identity_public_key`).
    /// Rejects a second login-α registration for the same holder (`Conflict`).
    /// The holder's first method becomes their primary. Returns the new id.
    pub async fn register_login_alpha(&self, did: &str) -> PdsResult<String> {
        let existing = sqlx::query(
            "SELECT COUNT(*) AS c FROM holder_auth_method \
             WHERE did = $1 AND method_type = 'login_alpha'",
        )
        .bind(did)
        .fetch_one(&self.db)
        .await
        .map_err(PdsError::Database)?
        .try_get::<i64, _>("c")
        .map_err(PdsError::Database)?;
        if existing > 0 {
            return Err(PdsError::Conflict(
                "login-α is already registered for this account".to_string(),
            ));
        }
        let is_primary = self.count_for_did(did).await? == 0;
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO holder_auth_method (id, did, method_type, is_primary, created_at) \
             VALUES ($1, $2, 'login_alpha', $3, $4)",
        )
        .bind(&id)
        .bind(did)
        .bind(is_primary)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;
        Ok(id)
    }

    /// Remove a method, DID-scoped. Enforces the SD-A5 safety invariant: never
    /// delete a holder's last remaining method (that would lock them out) —
    /// returns `Validation` instead. A missing/other-holder id is `NotFound`.
    pub async fn remove(&self, did: &str, id: &str) -> PdsResult<()> {
        // Confirm the method belongs to this holder (no cross-account removal).
        let owned = sqlx::query("SELECT id FROM holder_auth_method WHERE id = $1 AND did = $2")
            .bind(id)
            .bind(did)
            .fetch_optional(&self.db)
            .await
            .map_err(PdsError::Database)?;
        if owned.is_none() {
            return Err(PdsError::NotFound("auth method not found".to_string()));
        }
        if self.count_for_did(did).await? <= 1 {
            return Err(PdsError::Validation(
                "cannot remove your last remaining sign-in method".to_string(),
            ));
        }
        sqlx::query("DELETE FROM holder_auth_method WHERE id = $1 AND did = $2")
            .bind(id)
            .bind(did)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;
        Ok(())
    }

    /// Make `id` the holder's primary method, clearing any prior primary. Atomic
    /// (a transaction) so the partial-unique primary index never sees two
    /// primaries. DID-scoped: an id not owned by `did` is `NotFound`.
    pub async fn set_primary(&self, did: &str, id: &str) -> PdsResult<()> {
        let owned = sqlx::query("SELECT id FROM holder_auth_method WHERE id = $1 AND did = $2")
            .bind(id)
            .bind(did)
            .fetch_optional(&self.db)
            .await
            .map_err(PdsError::Database)?;
        if owned.is_none() {
            return Err(PdsError::NotFound("auth method not found".to_string()));
        }
        let mut tx = self.db.begin().await.map_err(PdsError::Database)?;
        // Clear the current primary first so setting the new one cannot collide
        // with the partial-unique-on-primary index.
        sqlx::query("UPDATE holder_auth_method SET is_primary = $1 WHERE did = $2")
            .bind(false)
            .bind(did)
            .execute(&mut *tx)
            .await
            .map_err(PdsError::Database)?;
        sqlx::query("UPDATE holder_auth_method SET is_primary = $1 WHERE id = $2 AND did = $3")
            .bind(true)
            .bind(id)
            .bind(did)
            .execute(&mut *tx)
            .await
            .map_err(PdsError::Database)?;
        tx.commit().await.map_err(PdsError::Database)?;
        Ok(())
    }

    /// Register a WebAuthn passkey method for `did`. The `Passkey` is stored
    /// whole (serde_json) in `passkey_data`; its credential id
    /// ([`Passkey::cred_id`]) is extracted into `passkey_credential_id` for the
    /// partial-unique index + authentication lookup. The holder's first method
    /// becomes primary. Returns the new method id.
    pub async fn register_passkey(
        &self,
        did: &str,
        passkey: &Passkey,
        device_name: Option<&str>,
    ) -> PdsResult<String> {
        let data = serde_json::to_string(passkey)
            .map_err(|e| PdsError::Internal(format!("failed to serialize passkey: {e}")))?;
        let credential_id: Vec<u8> = passkey.cred_id().to_vec();
        let is_primary = self.count_for_did(did).await? == 0;
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO holder_auth_method \
             (id, did, method_type, is_primary, passkey_credential_id, passkey_device_name, \
              passkey_data, created_at) \
             VALUES ($1, $2, 'passkey', $3, $4, $5, $6, $7)",
        )
        .bind(&id)
        .bind(did)
        .bind(is_primary)
        .bind(&credential_id)
        .bind(device_name)
        .bind(&data)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;
        Ok(id)
    }

    /// Load a holder's registered passkeys (deserialized). Used to build the
    /// `exclude_credentials` list at registration and the allow-list at
    /// authentication.
    pub async fn list_passkeys_for_did(&self, did: &str) -> PdsResult<Vec<Passkey>> {
        let rows = sqlx::query(
            "SELECT passkey_data FROM holder_auth_method \
             WHERE did = $1 AND method_type = 'passkey' AND passkey_data IS NOT NULL",
        )
        .bind(did)
        .fetch_all(&self.db)
        .await
        .map_err(PdsError::Database)?;

        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let data: String = row.try_get("passkey_data").map_err(PdsError::Database)?;
            let passkey: Passkey = serde_json::from_str(&data)
                .map_err(|e| PdsError::Internal(format!("corrupt passkey_data: {e}")))?;
            out.push(passkey);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ctx() -> crate::context::AppContext {
        crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await
    }

    /// Seed an actor + a did:web password method, returning the method id.
    async fn seed_password(ctx: &crate::context::AppContext, did: &str, plaintext: &str) -> String {
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(did)
            .bind(format!("{}.example.com", did.rsplit(':').next().unwrap()))
            .bind("2026-01-01T00:00:00Z")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        let hash = crate::auth::PasswordHasher::hash(plaintext).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO holder_auth_method \
             (id, did, method_type, is_primary, password_hash, password_algo, created_at) \
             VALUES ($1, $2, 'password', $3, $4, 'argon2id', $5)",
        )
        .bind(&id)
        .bind(did)
        .bind(true)
        .bind(&hash)
        .bind("2026-01-01T00:00:00Z")
        .execute(&ctx.account_db)
        .await
        .unwrap();
        id
    }

    #[tokio::test]
    async fn verify_password_matches_correct_secret() {
        let ctx = ctx().await;
        let did = "did:web:alice.example.com";
        let id = seed_password(&ctx, did, "correct horse battery").await;
        let got = ctx
            .holder_auth_methods
            .verify_password(did, "correct horse battery")
            .await
            .unwrap();
        assert_eq!(got.as_deref(), Some(id.as_str()));
    }

    #[tokio::test]
    async fn verify_password_rejects_wrong_secret() {
        let ctx = ctx().await;
        let did = "did:web:bob.example.com";
        seed_password(&ctx, did, "the-real-one").await;
        let got = ctx
            .holder_auth_methods
            .verify_password(did, "a-wrong-guess")
            .await
            .unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn verify_password_unknown_did_is_none() {
        let ctx = ctx().await;
        let got = ctx
            .holder_auth_methods
            .verify_password("did:web:ghost.example.com", "anything")
            .await
            .unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn touch_updates_last_used_at() {
        let ctx = ctx().await;
        let did = "did:web:carol.example.com";
        let id = seed_password(&ctx, did, "pw").await;
        // last_used_at starts NULL.
        let before: Option<String> =
            sqlx::query("SELECT last_used_at FROM holder_auth_method WHERE id = $1")
                .bind(&id)
                .fetch_one(&ctx.account_db)
                .await
                .unwrap()
                .try_get("last_used_at")
                .unwrap();
        assert!(before.is_none());

        ctx.holder_auth_methods.touch(&id).await.unwrap();

        let after: Option<String> =
            sqlx::query("SELECT last_used_at FROM holder_auth_method WHERE id = $1")
                .bind(&id)
                .fetch_one(&ctx.account_db)
                .await
                .unwrap()
                .try_get("last_used_at")
                .unwrap();
        assert!(after.is_some());
    }

    #[tokio::test]
    async fn touch_missing_id_is_ok() {
        let ctx = ctx().await;
        assert!(ctx.holder_auth_methods.touch("no-such-id").await.is_ok());
    }
}
