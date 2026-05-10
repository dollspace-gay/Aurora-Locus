/// Account Moderation System
use crate::account::AccountManager;
use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{AnyPool, Row};
use std::str::FromStr;
use std::sync::Arc;

/// Moderation action types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModerationAction {
    /// Remove content from public view
    Takedown,
    /// Temporarily suspend account
    Suspend,
    /// Flag for review
    Flag,
    /// Warning to user
    Warn,
    /// Restore after takedown/suspension
    Restore,
}

impl ModerationAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModerationAction::Takedown => "takedown",
            ModerationAction::Suspend => "suspend",
            ModerationAction::Flag => "flag",
            ModerationAction::Warn => "warn",
            ModerationAction::Restore => "restore",
        }
    }
}

impl FromStr for ModerationAction {
    type Err = PdsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "takedown" => Ok(ModerationAction::Takedown),
            "suspend" => Ok(ModerationAction::Suspend),
            "flag" => Ok(ModerationAction::Flag),
            "warn" => Ok(ModerationAction::Warn),
            "restore" => Ok(ModerationAction::Restore),
            _ => Err(PdsError::Validation(format!(
                "Invalid moderation action: {}",
                s
            ))),
        }
    }
}

/// Moderation record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModerationRecord {
    pub id: i64,
    pub did: String,
    pub action: ModerationAction,
    pub reason: String,
    pub moderated_by: String,
    pub moderated_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub reversed: bool,
    pub reversed_at: Option<DateTime<Utc>>,
    pub reversed_by: Option<String>,
    pub reversal_reason: Option<String>,
    pub report_id: Option<i64>,
    pub notes: Option<String>,
}

/// Parameters for applying a moderation action
pub struct ApplyActionParams<'a> {
    pub did: &'a str,
    pub action: ModerationAction,
    pub reason: &'a str,
    pub moderated_by: &'a str,
    pub expires_in: Option<Duration>,
    pub report_id: Option<i64>,
    pub notes: Option<String>,
}

/// Moderation manager
#[derive(Clone)]
pub struct ModerationManager {
    db: AnyPool,
    account_manager: Arc<AccountManager>,
}

impl ModerationManager {
    pub fn new(db: AnyPool, account_manager: Arc<AccountManager>) -> Self {
        Self {
            db,
            account_manager,
        }
    }

    /// Apply moderation action to an account
    pub async fn apply_action(&self, params: ApplyActionParams<'_>) -> PdsResult<ModerationRecord> {
        let mut tx = self.db.begin().await?;
        let record = Self::apply_action_in_tx(&mut tx, params).await?;
        tx.commit().await?;
        Ok(record)
    }

    /// Apply moderation action inside an existing transaction. LB-1 /
    /// chainlink #129 atomic-with-chain entry point.
    ///
    /// For Takedown actions, the actor's `takedown_ref` UPDATE happens
    /// inside the same transaction via [`AccountManager::takedown_account_in_tx`].
    /// Pre-LB-1, the account-level takedown was best-effort with a
    /// "moderation record is already created" log line on failure;
    /// post-LB-1 the two writes are atomic — if the actor UPDATE fails
    /// the moderation_event row also rolls back.
    pub async fn apply_action_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        params: ApplyActionParams<'_>,
    ) -> PdsResult<ModerationRecord> {
        let ApplyActionParams {
            did,
            action,
            reason,
            moderated_by,
            expires_in,
            report_id,
            notes,
        } = params;

        let now = Utc::now();
        let expires_at = expires_in.map(|d| now + d);

        let id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO account_moderation
            (did, action, reason, moderated_by, moderated_at, expires_at, report_id, notes)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING id
            "#,
        )
        .bind(did)
        .bind(action.as_str())
        .bind(reason)
        .bind(moderated_by)
        .bind(now.to_rfc3339())
        .bind(expires_at.map(|dt| dt.to_rfc3339()))
        .bind(report_id)
        .bind(&notes)
        .fetch_one(&mut **tx)
        .await?;

        // Apply account-level action if it's a takedown. Now in-tx: a
        // failure here aborts the transaction including the
        // account_moderation INSERT above, so the two stay consistent.
        if action == ModerationAction::Takedown {
            let takedown_ref = format!("mod_{}", id);
            crate::account::AccountManager::takedown_account_in_tx(tx, did, &takedown_ref)
                .await?;
        }

        Ok(ModerationRecord {
            id,
            did: did.to_string(),
            action,
            reason: reason.to_string(),
            moderated_by: moderated_by.to_string(),
            moderated_at: now,
            expires_at,
            reversed: false,
            reversed_at: None,
            reversed_by: None,
            reversal_reason: None,
            report_id,
            notes,
        })
    }

    /// Reverse a moderation action
    pub async fn reverse_action(
        &self,
        moderation_id: i64,
        reversed_by: &str,
        reason: &str,
    ) -> PdsResult<()> {
        let mut tx = self.db.begin().await?;
        Self::reverse_action_in_tx(&mut tx, moderation_id, reversed_by, reason).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Reverse a moderation action inside an existing transaction.
    /// LB-1 / chainlink #129 atomic-with-chain entry point.
    ///
    /// When reversing a takedown, the actor's `takedown_ref` clear
    /// happens inside the same transaction via
    /// [`AccountManager::activate_account_in_tx`]. Pre-LB-1 it was
    /// best-effort.
    pub async fn reverse_action_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        moderation_id: i64,
        reversed_by: &str,
        reason: &str,
    ) -> PdsResult<()> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE account_moderation
            SET reversed = true,
                reversed_at = $1,
                reversed_by = $2,
                reversal_reason = $3
            WHERE id = $4 AND NOT reversed
            "#,
        )
        .bind(now.to_rfc3339())
        .bind(reversed_by)
        .bind(reason)
        .bind(moderation_id)
        .execute(&mut **tx)
        .await?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!(
                "Moderation record {} not found or already reversed",
                moderation_id
            )));
        }

        // Read the action type that was reversed (in-tx for snapshot
        // consistency).
        let row = sqlx::query("SELECT action, did FROM account_moderation WHERE id = $1")
            .bind(moderation_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| {
                PdsError::NotFound(format!("Moderation record {} not found", moderation_id))
            })?;

        let action_str: String = row.get("action");
        let did: String = row.get("did");

        // If reversing a takedown, activate the account inside the
        // same tx. Atomic: actor UPDATE failure aborts the moderation
        // reversal too.
        if action_str == "takedown" {
            crate::account::AccountManager::activate_account_in_tx(tx, &did).await?;
        }

        Ok(())
    }

    /// Get active moderation actions for an account
    pub async fn get_active_actions(&self, did: &str) -> PdsResult<Vec<ModerationRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT id, did, action, reason, moderated_by, moderated_at,
                   expires_at, reversed, reversed_at, reversed_by,
                   reversal_reason, report_id, notes
            FROM account_moderation
            WHERE did = $1 AND NOT reversed
            ORDER BY moderated_at DESC
            "#,
        )
        .bind(did)
        .fetch_all(&self.db)
        .await?;

        self.parse_moderation_records(rows).await
    }

    /// Check if account is currently taken down
    pub async fn is_taken_down(&self, did: &str) -> PdsResult<bool> {
        let actions = self.get_active_actions(did).await?;
        Ok(actions
            .iter()
            .any(|a| a.action == ModerationAction::Takedown))
    }

    /// Check if account is currently suspended
    pub async fn is_suspended(&self, did: &str) -> PdsResult<bool> {
        let actions = self.get_active_actions(did).await?;
        let now = Utc::now();

        Ok(actions.iter().any(|a| {
            a.action == ModerationAction::Suspend && a.expires_at.is_none_or(|exp| exp > now)
        }))
    }

    /// Get moderation history for an account
    pub async fn get_history(&self, did: &str) -> PdsResult<Vec<ModerationRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT id, did, action, reason, moderated_by, moderated_at,
                   expires_at, reversed, reversed_at, reversed_by,
                   reversal_reason, report_id, notes
            FROM account_moderation
            WHERE did = $1
            ORDER BY moderated_at DESC
            "#,
        )
        .bind(did)
        .fetch_all(&self.db)
        .await?;

        self.parse_moderation_records(rows).await
    }

    /// Cleanup expired suspensions
    pub async fn cleanup_expired(&self) -> PdsResult<u64> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE account_moderation
            SET reversed = true,
                reversed_at = $1,
                reversed_by = 'system',
                reversal_reason = 'Expired'
            WHERE action = 'suspend'
              AND NOT reversed
              AND expires_at IS NOT NULL
              AND expires_at < $2
            "#,
        )
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.db)
        .await?;

        Ok(result.rows_affected())
    }

    /// Parse database rows into ModerationRecord objects
    async fn parse_moderation_records(
        &self,
        rows: Vec<sqlx::any::AnyRow>,
    ) -> PdsResult<Vec<ModerationRecord>> {
        let mut records = Vec::new();

        for row in rows {
            let action_str: String = row.get("action");
            let action = ModerationAction::from_str(&action_str)?;

            let moderated_at_str: String = row.get("moderated_at");
            let moderated_at = DateTime::parse_from_rfc3339(&moderated_at_str)
                .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
                .with_timezone(&Utc);

            let expires_at = row
                .try_get::<String, _>("expires_at")
                .ok()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));

            let reversed_at = row
                .try_get::<String, _>("reversed_at")
                .ok()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));

            records.push(ModerationRecord {
                id: row.get("id"),
                did: row.get("did"),
                action,
                reason: row.get("reason"),
                moderated_by: row.get("moderated_by"),
                moderated_at,
                expires_at,
                reversed: crate::db::read_bool(&row, "reversed")?,
                reversed_at,
                reversed_by: row.get("reversed_by"),
                reversal_reason: row.get("reversal_reason"),
                report_id: row.get("report_id"),
                notes: row.get("notes"),
            });
        }

        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use std::path::PathBuf;

    fn create_test_config() -> crate::config::ServerConfig {
        crate::config::ServerConfig {
            service: ServiceConfig {
                hostname: "localhost".to_string(),
                port: 2583,
                service_did: "did:web:localhost".to_string(),
                version: "0.1.0".to_string(),
                blob_upload_limit: 5242880,
            },
            storage: StorageConfig {
                data_directory: PathBuf::from("./data"),
                account_db: PathBuf::from(":memory:"),
                sequencer_db: PathBuf::from(":memory:"),
                did_cache_db: PathBuf::from(":memory:"),
                actor_store_directory: PathBuf::from("./data/actors"),
                blobstore: BlobstoreConfig::Disk {
                    location: PathBuf::from("./data/blobs"),
                    tmp_location: PathBuf::from("./data/tmp"),
                },
            },
            database: Default::default(),
            authentication: AuthConfig {
                jwt_secret: "test-secret-key".to_string(),
                repo_signing_key: "test-key".to_string(),
                plc_rotation_key: "test-rotation-key".to_string(),
                oauth: OAuthConfig {
                    client_id: "test-client".to_string(),
                    redirect_uri: "http://localhost:3000/oauth/callback".to_string(),
                    pds_url: "http://localhost:3000".to_string(),
                },
                jwt_sunset_date: "Sat, 31 Dec 2024 23:59:59 GMT".to_string(),
                oauth_migration_guide_url: "https://docs.example.com/oauth-migration".to_string(),
                oauth_features: Default::default(),
            },
            identity: IdentityConfig {
                did_plc_url: "https://plc.directory".to_string(),
                service_handle_domains: vec!["localhost".to_string()],
                did_cache_stale_ttl: 3600,
                did_cache_max_ttl: 86400,
            },
            email: None,
            invites: InviteConfig {
                required: false,
                interval: 604800,
                epoch: "2024-01-01T00:00:00Z".to_string(),
            },
            rate_limit: RateLimitConfig {
                enabled: true,
                global_requests_per_minute: 3000,
                redis_url: None,
                use_redis: false,
                exempt_admin_assets: true,
            },
            logging: LoggingConfig {
                level: "info".to_string(),
            },
            federation: crate::config::FederationConfig {
                enabled: false,
                relay_urls: vec![],
                appview_url: None,
                firehose_enabled: false,
                crawl_enabled: false,
                public_url: None,
                auto_stream_events: false,
            },
            validation_mode: crate::validation::ValidationMode::Optimistic,
        }
    }

    #[test]
    fn test_action_from_str() {
        assert_eq!(
            ModerationAction::from_str("takedown").unwrap(),
            ModerationAction::Takedown
        );
        assert_eq!(
            ModerationAction::from_str("suspend").unwrap(),
            ModerationAction::Suspend
        );
        assert!(ModerationAction::from_str("invalid").is_err());
    }

    #[tokio::test]
    async fn test_apply_and_get_action() {
        {
            use std::sync::Once;
            static INSTALL: Once = Once::new();
            INSTALL.call_once(sqlx::any::install_default_drivers);
        }
        let db = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE account_moderation (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                did TEXT NOT NULL,
                action TEXT NOT NULL,
                reason TEXT NOT NULL,
                moderated_by TEXT NOT NULL,
                moderated_at TEXT NOT NULL,
                expires_at TEXT,
                reversed INTEGER NOT NULL DEFAULT 0,
                reversed_at TEXT,
                reversed_by TEXT,
                reversal_reason TEXT,
                report_id INTEGER,
                notes TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();
        // LB-1 Session 12: apply_action's takedown branch now writes
        // to actor.takedown_ref + DELETEs from session/refresh_token,
        // all in the same transaction as the moderation row. Tests
        // need those tables.
        sqlx::query(
            "CREATE TABLE actor (did TEXT PRIMARY KEY, handle TEXT, takedown_ref TEXT, \
             deactivated_at TEXT, delete_after TEXT)",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query("CREATE TABLE session (id INTEGER PRIMARY KEY, did TEXT)")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE refresh_token (id INTEGER PRIMARY KEY, did TEXT)")
            .execute(&db)
            .await
            .unwrap();

        // Create minimal account manager for tests
        let config = create_test_config();
        let account_manager = Arc::new(crate::account::AccountManager::new(
            db.clone(),
            Arc::new(config),
        ));

        // LB-1 Session 12 / chainlink #129: takedown side-effect now
        // runs in-tx; the actor row must exist before apply_action.
        sqlx::query("INSERT INTO actor (did, handle) VALUES ('did:plc:spam123', 'spam.test')")
            .execute(&db)
            .await
            .unwrap();

        let manager = ModerationManager::new(db, account_manager);

        // Apply takedown
        let record = manager
            .apply_action(ApplyActionParams {
                did: "did:plc:spam123",
                action: ModerationAction::Takedown,
                reason: "Spam content",
                moderated_by: "did:plc:admin",
                expires_in: None,
                report_id: None,
                notes: Some("Automated detection".to_string()),
            })
            .await
            .unwrap();

        assert_eq!(record.action, ModerationAction::Takedown);
        assert!(!record.reversed);

        // Check if taken down
        assert!(manager.is_taken_down("did:plc:spam123").await.unwrap());

        // Get active actions
        let actions = manager.get_active_actions("did:plc:spam123").await.unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].action, ModerationAction::Takedown);
    }

    #[tokio::test]
    async fn test_suspend_with_expiration() {
        {
            use std::sync::Once;
            static INSTALL: Once = Once::new();
            INSTALL.call_once(sqlx::any::install_default_drivers);
        }
        let db = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE account_moderation (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                did TEXT NOT NULL,
                action TEXT NOT NULL,
                reason TEXT NOT NULL,
                moderated_by TEXT NOT NULL,
                moderated_at TEXT NOT NULL,
                expires_at TEXT,
                reversed INTEGER NOT NULL DEFAULT 0,
                reversed_at TEXT,
                reversed_by TEXT,
                reversal_reason TEXT,
                report_id INTEGER,
                notes TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();
        // LB-1 Session 12: apply_action's takedown branch now writes
        // to actor.takedown_ref + DELETEs from session/refresh_token,
        // all in the same transaction as the moderation row. Tests
        // need those tables.
        sqlx::query(
            "CREATE TABLE actor (did TEXT PRIMARY KEY, handle TEXT, takedown_ref TEXT, \
             deactivated_at TEXT, delete_after TEXT)",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query("CREATE TABLE session (id INTEGER PRIMARY KEY, did TEXT)")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE refresh_token (id INTEGER PRIMARY KEY, did TEXT)")
            .execute(&db)
            .await
            .unwrap();

        // Create minimal account manager for tests
        let config = create_test_config();
        let account_manager = Arc::new(crate::account::AccountManager::new(
            db.clone(),
            Arc::new(config),
        ));

        let manager = ModerationManager::new(db, account_manager);

        // Suspend for 7 days
        manager
            .apply_action(ApplyActionParams {
                did: "did:plc:bad123",
                action: ModerationAction::Suspend,
                reason: "Repeated violations",
                moderated_by: "did:plc:admin",
                expires_in: Some(Duration::days(7)),
                report_id: None,
                notes: None,
            })
            .await
            .unwrap();

        assert!(manager.is_suspended("did:plc:bad123").await.unwrap());
    }

    #[tokio::test]
    async fn test_reverse_action() {
        {
            use std::sync::Once;
            static INSTALL: Once = Once::new();
            INSTALL.call_once(sqlx::any::install_default_drivers);
        }
        let db = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE account_moderation (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                did TEXT NOT NULL,
                action TEXT NOT NULL,
                reason TEXT NOT NULL,
                moderated_by TEXT NOT NULL,
                moderated_at TEXT NOT NULL,
                expires_at TEXT,
                reversed INTEGER NOT NULL DEFAULT 0,
                reversed_at TEXT,
                reversed_by TEXT,
                reversal_reason TEXT,
                report_id INTEGER,
                notes TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();
        // LB-1 Session 12: apply_action's takedown branch now writes
        // to actor.takedown_ref + DELETEs from session/refresh_token,
        // all in the same transaction as the moderation row. Tests
        // need those tables.
        sqlx::query(
            "CREATE TABLE actor (did TEXT PRIMARY KEY, handle TEXT, takedown_ref TEXT, \
             deactivated_at TEXT, delete_after TEXT)",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query("CREATE TABLE session (id INTEGER PRIMARY KEY, did TEXT)")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE refresh_token (id INTEGER PRIMARY KEY, did TEXT)")
            .execute(&db)
            .await
            .unwrap();

        // Create minimal account manager for tests
        let config = create_test_config();
        let account_manager = Arc::new(crate::account::AccountManager::new(
            db.clone(),
            Arc::new(config),
        ));

        // Seed actor row for the in-tx takedown side effect.
        sqlx::query("INSERT INTO actor (did, handle) VALUES ('did:plc:false', 'false.test')")
            .execute(&db)
            .await
            .unwrap();

        let manager = ModerationManager::new(db, account_manager);

        // Apply and reverse
        let record = manager
            .apply_action(ApplyActionParams {
                did: "did:plc:false",
                action: ModerationAction::Takedown,
                reason: "Mistake",
                moderated_by: "did:plc:admin",
                expires_in: None,
                report_id: None,
                notes: None,
            })
            .await
            .unwrap();

        manager
            .reverse_action(record.id, "did:plc:superadmin", "False positive")
            .await
            .unwrap();

        // Should no longer be taken down
        assert!(!manager.is_taken_down("did:plc:false").await.unwrap());
    }

    // LB-1 Session 12 / chainlink #129: ModerationManager `_in_tx`
    // variants must be rollback-safe and must thread the transaction
    // through to AccountManager so the actor-table mutation rolls
    // back together with the moderation_event INSERT.

    async fn build_moderation_test_pool() -> (AnyPool, Arc<AccountManager>) {
        {
            use std::sync::Once;
            static INSTALL: Once = Once::new();
            INSTALL.call_once(sqlx::any::install_default_drivers);
        }
        let db = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE actor (
                did TEXT PRIMARY KEY,
                handle TEXT,
                takedown_ref TEXT,
                deactivated_at TEXT,
                delete_after TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE account_moderation (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                did TEXT NOT NULL,
                action TEXT NOT NULL,
                reason TEXT NOT NULL,
                moderated_by TEXT NOT NULL,
                moderated_at TEXT NOT NULL,
                expires_at TEXT,
                reversed INTEGER NOT NULL DEFAULT 0,
                reversed_at TEXT,
                reversed_by TEXT,
                reversal_reason TEXT,
                report_id INTEGER,
                notes TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query("CREATE TABLE session (id INTEGER PRIMARY KEY, did TEXT)")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE refresh_token (id INTEGER PRIMARY KEY, did TEXT)")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO actor (did, handle) VALUES ('did:plc:victim', 'v.test')",
        )
        .execute(&db)
        .await
        .unwrap();
        let config = create_test_config();
        let account_manager = Arc::new(AccountManager::new(db.clone(), Arc::new(config)));
        (db, account_manager)
    }

    #[tokio::test]
    async fn apply_action_in_tx_rolls_back_with_actor_mutation() {
        let (db, _) = build_moderation_test_pool().await;
        // takedown via the in-tx variant; rollback; assert neither
        // the moderation_event row nor the actor.takedown_ref landed.
        {
            let mut tx = db.begin().await.unwrap();
            ModerationManager::apply_action_in_tx(
                &mut tx,
                ApplyActionParams {
                    did: "did:plc:victim",
                    action: ModerationAction::Takedown,
                    reason: "test",
                    moderated_by: "did:plc:admin",
                    expires_in: None,
                    report_id: None,
                    notes: None,
                },
            )
            .await
            .unwrap();
            tx.rollback().await.unwrap();
        }

        let mod_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM account_moderation")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(mod_count, 0, "moderation_event rolled back");
        let takedown: Option<String> = sqlx::query_scalar(
            "SELECT takedown_ref FROM actor WHERE did = 'did:plc:victim'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert!(
            takedown.is_none(),
            "actor.takedown_ref rolled back together with moderation row"
        );
    }

    #[tokio::test]
    async fn reverse_action_in_tx_rolls_back_with_actor_activate() {
        let (db, _) = build_moderation_test_pool().await;
        // Pre-seed: takedown the account via the existing in_tx path,
        // commit, then exercise reverse_action_in_tx with rollback.
        let mut setup_tx = db.begin().await.unwrap();
        let record = ModerationManager::apply_action_in_tx(
            &mut setup_tx,
            ApplyActionParams {
                did: "did:plc:victim",
                action: ModerationAction::Takedown,
                reason: "initial",
                moderated_by: "did:plc:admin",
                expires_in: None,
                report_id: None,
                notes: None,
            },
        )
        .await
        .unwrap();
        setup_tx.commit().await.unwrap();
        // Confirm takedown_ref is set.
        let takedown_pre: Option<String> = sqlx::query_scalar(
            "SELECT takedown_ref FROM actor WHERE did = 'did:plc:victim'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert!(takedown_pre.is_some());

        // Now reverse_action_in_tx + rollback. takedown_ref should
        // remain set (the activate UPDATE rolled back too).
        {
            let mut tx = db.begin().await.unwrap();
            ModerationManager::reverse_action_in_tx(
                &mut tx,
                record.id,
                "did:plc:superadmin",
                "false positive",
            )
            .await
            .unwrap();
            tx.rollback().await.unwrap();
        }

        let takedown_post: Option<String> = sqlx::query_scalar(
            "SELECT takedown_ref FROM actor WHERE did = 'did:plc:victim'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert!(
            takedown_post.is_some(),
            "actor.takedown_ref must NOT clear when reverse rolled back"
        );
    }
}
