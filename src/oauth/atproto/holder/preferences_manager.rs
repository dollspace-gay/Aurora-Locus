//! Per-holder display preferences (Holder UI Phase 1, chainlink #424).
//!
//! Backs the `atproto_holder_preferences` table (sqlite migration `0032` / pg
//! `0033`). The first per-account preferences store in the codebase
//! (`runtime_settings` is operator-tier, not per-holder). Phase 1 stores one
//! preference: the holder's chosen display theme (`None` → the operator's active
//! theme). All SQL touching the table lives here.

use chrono::Utc;
use sqlx::{AnyPool, Row};

use crate::error::{PdsError, PdsResult};

/// A holder's display preferences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HolderPreferences {
    /// Chosen theme id, or `None` to follow the operator's active theme.
    pub theme: Option<String>,
    pub updated_at: String,
}

/// Per-holder preferences store.
#[derive(Clone)]
pub struct AtprotoHolderPreferencesManager {
    db: AnyPool,
}

impl AtprotoHolderPreferencesManager {
    pub fn new(db: AnyPool) -> Self {
        Self { db }
    }

    /// Read a holder's preferences. Returns a default (`theme: None`) when no
    /// row exists — the pre-first-save state.
    pub async fn get(&self, did: &str) -> PdsResult<HolderPreferences> {
        let row = sqlx::query(
            "SELECT theme, updated_at FROM atproto_holder_preferences WHERE did = $1",
        )
        .bind(did)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?;

        match row {
            Some(row) => Ok(HolderPreferences {
                theme: row.try_get("theme").map_err(PdsError::Database)?,
                updated_at: row.try_get("updated_at").map_err(PdsError::Database)?,
            }),
            None => Ok(HolderPreferences {
                theme: None,
                updated_at: Utc::now().to_rfc3339(),
            }),
        }
    }

    /// Set (or clear, with `None`) a holder's theme. Upserts the row.
    pub async fn set_theme(&self, did: &str, theme: Option<&str>) -> PdsResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO atproto_holder_preferences (did, theme, updated_at) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (did) DO UPDATE SET theme = $2, updated_at = $3",
        )
        .bind(did)
        .bind(theme)
        .bind(&now)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    async fn ctx() -> crate::context::AppContext {
        crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await
    }

    async fn seed_actor(ctx: &crate::context::AppContext, did: &str) {
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(did)
            .bind(format!("{}.example.com", did.replace(':', "-")))
            .bind("2026-01-01T00:00:00Z")
            .execute(&ctx.account_db)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn get_defaults_to_none_theme() {
        let ctx = ctx().await;
        let prefs = ctx
            .holder_preferences
            .get("did:web:nobody.example.com")
            .await
            .unwrap();
        assert_eq!(prefs.theme, None);
    }

    #[tokio::test]
    async fn set_then_get_roundtrips() {
        let ctx = ctx().await;
        let did = "did:web:alice.example.com";
        seed_actor(&ctx, did).await;
        ctx.holder_preferences.set_theme(did, Some("dark")).await.unwrap();
        assert_eq!(
            ctx.holder_preferences.get(did).await.unwrap().theme.as_deref(),
            Some("dark")
        );
        // Upsert (change) then clear.
        ctx.holder_preferences.set_theme(did, Some("ember")).await.unwrap();
        assert_eq!(
            ctx.holder_preferences.get(did).await.unwrap().theme.as_deref(),
            Some("ember")
        );
        ctx.holder_preferences.set_theme(did, None).await.unwrap();
        assert_eq!(ctx.holder_preferences.get(did).await.unwrap().theme, None);
    }
}
