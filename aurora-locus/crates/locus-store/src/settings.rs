//! Runtime settings storage. Every change bumps `settings_version` and writes
//! an audit row in the same transaction; on Postgres it also sends a
//! `NOTIFY` on [`SETTINGS_CHANNEL`] (delivered at commit) so other nodes
//! reload at once.

use std::collections::BTreeMap;

use locus_core::settings::{self, Snapshot};
use locus_core::{CoreError, CoreResult};
use serde_json::Value;

use crate::{PG_NOW_MS, SQLITE_NOW_MS, Store, Tx, db_err};

/// Postgres notification channel for settings changes.
pub const SETTINGS_CHANNEL: &str = "locus_settings";

impl Store {
    /// The current settings version.
    pub async fn settings_version(&self) -> CoreResult<i64> {
        let sql = "SELECT version FROM settings_version WHERE id = 1";
        match self {
            Self::Sqlite(p) => sqlx::query_scalar(sql).fetch_one(p).await,
            Self::Postgres(p) => sqlx::query_scalar(sql).fetch_one(p).await,
        }
        .map_err(db_err)
    }

    /// Every stored setting and the version they were read at.
    pub async fn load_settings(&self) -> CoreResult<(i64, BTreeMap<String, Value>)> {
        let mut tx = match self {
            Self::Sqlite(p) => Tx::Sqlite(p.begin().await.map_err(db_err)?),
            Self::Postgres(p) => {
                let mut t = p.begin().await.map_err(db_err)?;
                // One consistent view of the version and the rows.
                sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
                    .execute(&mut *t)
                    .await
                    .map_err(db_err)?;
                Tx::Postgres(t)
            }
        };
        let version: i64;
        let rows: Vec<(String, String)>;
        match &mut tx {
            Tx::Sqlite(t) => {
                version = sqlx::query_scalar("SELECT version FROM settings_version WHERE id = 1")
                    .fetch_one(&mut **t)
                    .await
                    .map_err(db_err)?;
                rows = sqlx::query_as("SELECT key, value_json FROM setting")
                    .fetch_all(&mut **t)
                    .await
                    .map_err(db_err)?;
            }
            Tx::Postgres(t) => {
                version = sqlx::query_scalar("SELECT version FROM settings_version WHERE id = 1")
                    .fetch_one(&mut **t)
                    .await
                    .map_err(db_err)?;
                rows = sqlx::query_as("SELECT key, value_json FROM setting")
                    .fetch_all(&mut **t)
                    .await
                    .map_err(db_err)?;
            }
        }
        tx.rollback().await?;
        let mut map = BTreeMap::new();
        for (key, json) in rows {
            let value = serde_json::from_str(&json).map_err(|e| {
                CoreError::Storage(format!("setting {key} holds invalid JSON: {e}"))
            })?;
            map.insert(key, value);
        }
        Ok((version, map))
    }

    /// Loads a [`Snapshot`], logging stored values that no longer validate.
    pub async fn load_snapshot(&self) -> CoreResult<Snapshot> {
        let (version, stored) = self.load_settings().await?;
        let (snap, problems) = Snapshot::build(version, &stored);
        for p in problems {
            tracing::warn!("stored setting ignored, using default: {p}");
        }
        Ok(snap)
    }

    /// Validates and stores one setting, audited as `actor`. Returns the new
    /// settings version.
    pub async fn set_setting(&self, key: &str, value: Value, actor: &str) -> CoreResult<i64> {
        let mut tx = self.begin().await?;
        let version = tx.set_setting(key, value, actor).await?;
        tx.commit().await?;
        Ok(version)
    }
}

impl Tx {
    /// Stores a setting from env or a config file. An endpoint URL there that
    /// uses plain HTTP is allowed for that endpoint (`http.allow_insecure_http`),
    /// as a reference `pds.env` with a LAN AppView or a local PLC expects;
    /// the console warns about it. Returns whether that happened.
    pub async fn set_env_setting(
        &mut self,
        key: &str,
        value: Value,
        actor: &str,
    ) -> CoreResult<bool> {
        let insecure =
            settings::ENDPOINT_URL_SETTINGS.contains(&key) && settings::plain_http(&value);
        self.set_setting(key, value, actor).await?;
        if !insecure {
            return Ok(false);
        }
        let list_key = "http.allow_insecure_http";
        let current: Option<String> = match self {
            Tx::Sqlite(t) => {
                sqlx::query_scalar("SELECT value_json FROM setting WHERE key = ?1")
                    .bind(list_key)
                    .fetch_optional(&mut **t)
                    .await
            }
            Tx::Postgres(t) => {
                sqlx::query_scalar("SELECT value_json FROM setting WHERE key = $1")
                    .bind(list_key)
                    .fetch_optional(&mut **t)
                    .await
            }
        }
        .map_err(db_err)?;
        let mut list: Vec<String> = current
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        if !list.iter().any(|k| k == key) {
            list.push(key.to_owned());
            self.set_setting(list_key, serde_json::json!(list), actor)
                .await?;
        }
        Ok(true)
    }

    /// Validates and stores one setting inside this transaction. Returns the
    /// new settings version.
    pub async fn set_setting(&mut self, key: &str, value: Value, actor: &str) -> CoreResult<i64> {
        let def = settings::def(key)?;
        let value = def.validate(value)?;
        let json = value.to_string();
        let old: Option<String>;
        let version: i64;
        match self {
            Tx::Sqlite(t) => {
                old = sqlx::query_scalar("SELECT value_json FROM setting WHERE key = ?1")
                    .bind(key)
                    .fetch_optional(&mut **t)
                    .await
                    .map_err(db_err)?;
                version = sqlx::query_scalar(
                    "UPDATE settings_version SET version = version + 1 WHERE id = 1 RETURNING version",
                )
                .fetch_one(&mut **t)
                .await
                .map_err(db_err)?;
                sqlx::query(&format!(
                    "INSERT INTO setting (key, value_json, version, updated_at, updated_by) \
                     VALUES (?1, ?2, ?3, {SQLITE_NOW_MS}, ?4) \
                     ON CONFLICT (key) DO UPDATE SET value_json = excluded.value_json, \
                     version = excluded.version, updated_at = excluded.updated_at, \
                     updated_by = excluded.updated_by"
                ))
                .bind(key)
                .bind(&json)
                .bind(version)
                .bind(actor)
                .execute(&mut **t)
                .await
                .map_err(db_err)?;
            }
            Tx::Postgres(t) => {
                old = sqlx::query_scalar("SELECT value_json FROM setting WHERE key = $1")
                    .bind(key)
                    .fetch_optional(&mut **t)
                    .await
                    .map_err(db_err)?;
                version = sqlx::query_scalar(
                    "UPDATE settings_version SET version = version + 1 WHERE id = 1 RETURNING version",
                )
                .fetch_one(&mut **t)
                .await
                .map_err(db_err)?;
                sqlx::query(&format!(
                    "INSERT INTO setting (key, value_json, version, updated_at, updated_by) \
                     VALUES ($1, $2, $3, {PG_NOW_MS}, $4) \
                     ON CONFLICT (key) DO UPDATE SET value_json = excluded.value_json, \
                     version = excluded.version, updated_at = excluded.updated_at, \
                     updated_by = excluded.updated_by"
                ))
                .bind(key)
                .bind(&json)
                .bind(version)
                .bind(actor)
                .execute(&mut **t)
                .await
                .map_err(db_err)?;
                sqlx::query("SELECT pg_notify($1, $2)")
                    .bind(SETTINGS_CHANNEL)
                    .bind(version.to_string())
                    .execute(&mut **t)
                    .await
                    .map_err(db_err)?;
            }
        }
        let detail = serde_json::json!({
            "before": old.and_then(|o| serde_json::from_str::<Value>(&o).ok()),
            "after": value,
            "version": version,
        });
        self.audit(actor, "setting.set", Some(key), Some(&detail))
            .await?;
        Ok(version)
    }
}
