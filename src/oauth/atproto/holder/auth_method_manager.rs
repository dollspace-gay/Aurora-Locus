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

use crate::error::{PdsError, PdsResult};

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
