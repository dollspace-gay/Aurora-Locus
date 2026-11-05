//! Storage: one logical schema with a SQLite backend (single node) and a
//! Postgres backend (one or many nodes). Each backend has its own migration
//! tree and query text, so each can use its own strengths; callers only see
//! [`Store`] and [`Tx`].

pub mod accounts;
pub mod admin;
pub mod blob;
pub mod branding;
pub mod bulk;
pub mod console;
pub mod email;
pub mod env;
pub mod identity;
pub mod import;
pub mod invites;
pub mod jobs;
pub mod kryphocron;
pub mod lease;
pub mod oauth;
pub mod operators;
pub mod portal;
pub mod prefs;
pub mod repo;
pub mod runtime;
pub mod scan;
pub mod secrets;
pub mod seq;
pub mod settings;
pub mod setup;

#[cfg(test)]
mod tests;

use std::str::FromStr;
use std::time::Duration;

use locus_core::bootstrap::DatabaseUrl;
use locus_core::lease::LeaseToken;
use locus_core::{CoreError, CoreResult};
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx::{Postgres, Sqlite, Transaction};

static SQLITE_MIGRATIONS: Migrator = sqlx::migrate!("./migrations/sqlite");
static POSTGRES_MIGRATIONS: Migrator = sqlx::migrate!("./migrations/postgres");

/// Current time in ms since the epoch, from the database clock.
/// Postgres uses `clock_timestamp()` (wall time now, not transaction start).
pub(crate) const PG_NOW_MS: &str = "(extract(epoch from clock_timestamp()) * 1000)::bigint";
pub(crate) const SQLITE_NOW_MS: &str = "CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)";

/// A connection pool for one backend.
#[derive(Clone, Debug)]
pub enum Store {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}

/// A write transaction. On SQLite it's opened with `BEGIN IMMEDIATE`, so it
/// holds the database write lock from the start; keep it short.
pub enum Tx {
    Sqlite(Transaction<'static, Sqlite>),
    Postgres(Transaction<'static, Postgres>),
}

pub(crate) fn db_err(e: sqlx::Error) -> CoreError {
    CoreError::Storage(e.to_string())
}

impl Store {
    /// Connects, applies pending migrations, and refuses a schema newer than
    /// this build knows (an old node must not run against a newer schema).
    pub async fn open(url: &DatabaseUrl) -> CoreResult<Self> {
        let store = Self::connect(url).await?;
        store.migrate().await?;
        Ok(store)
    }

    /// Connects without migrating.
    pub async fn connect(url: &DatabaseUrl) -> CoreResult<Self> {
        match url {
            DatabaseUrl::Sqlite(path) => {
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        CoreError::Storage(format!("create {}: {e}", parent.display()))
                    })?;
                }
                let opts = SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .synchronous(SqliteSynchronous::Normal)
                    .busy_timeout(Duration::from_secs(5))
                    .foreign_keys(true);
                let pool = SqlitePoolOptions::new()
                    .max_connections(8)
                    .connect_with(opts)
                    .await
                    .map_err(db_err)?;
                Ok(Self::Sqlite(pool))
            }
            DatabaseUrl::Postgres(url) => {
                let opts = PgConnectOptions::from_str(url).map_err(db_err)?;
                let pool = PgPoolOptions::new()
                    .max_connections(32)
                    .acquire_timeout(Duration::from_secs(10))
                    .connect_with(opts)
                    .await
                    .map_err(db_err)?;
                Ok(Self::Postgres(pool))
            }
        }
    }

    fn migrator(&self) -> &'static Migrator {
        match self {
            Self::Sqlite(_) => &SQLITE_MIGRATIONS,
            Self::Postgres(_) => &POSTGRES_MIGRATIONS,
        }
    }

    /// Highest migration version this build knows.
    pub fn known_schema_version(&self) -> i64 {
        self.migrator().iter().map(|m| m.version).max().unwrap_or(0)
    }

    /// Highest migration version applied to the database (0 if none).
    pub async fn applied_schema_version(&self) -> CoreResult<i64> {
        let sql = "SELECT MAX(version) FROM _sqlx_migrations WHERE success";
        let res = match self {
            Self::Sqlite(p) => sqlx::query_scalar::<_, Option<i64>>(sql).fetch_one(p).await,
            Self::Postgres(p) => sqlx::query_scalar::<_, Option<i64>>(sql).fetch_one(p).await,
        };
        match res {
            Ok(v) => Ok(v.unwrap_or(0)),
            // No migrations table yet: a fresh database.
            Err(sqlx::Error::Database(e)) if is_missing_table(e.as_ref()) => Ok(0),
            Err(e) => Err(db_err(e)),
        }
    }

    /// Applies pending migrations. Postgres serializes concurrent nodes with
    /// an advisory lock (taken by the migrator); SQLite has one writer.
    pub async fn migrate(&self) -> CoreResult<()> {
        let applied = self.applied_schema_version().await?;
        let known = self.known_schema_version();
        if applied > known {
            return Err(CoreError::Config(format!(
                "database schema version {applied} is newer than this build knows ({known}); \
                 upgrade this node before starting it"
            )));
        }
        let res = match self {
            Self::Sqlite(p) => SQLITE_MIGRATIONS.run(p).await,
            Self::Postgres(p) => POSTGRES_MIGRATIONS.run(p).await,
        };
        res.map_err(|e| CoreError::Storage(format!("migration failed: {e}")))
    }

    /// Opens a write transaction.
    pub async fn begin(&self) -> CoreResult<Tx> {
        match self {
            Self::Sqlite(p) => Ok(Tx::Sqlite(
                p.begin_with("BEGIN IMMEDIATE").await.map_err(db_err)?,
            )),
            Self::Postgres(p) => Ok(Tx::Postgres(p.begin().await.map_err(db_err)?)),
        }
    }

    /// Current database time in ms since the epoch.
    pub async fn now_ms(&self) -> CoreResult<i64> {
        match self {
            Self::Sqlite(p) => sqlx::query_scalar(&format!("SELECT {SQLITE_NOW_MS}"))
                .fetch_one(p)
                .await
                .map_err(db_err),
            Self::Postgres(p) => sqlx::query_scalar(&format!("SELECT {PG_NOW_MS}"))
                .fetch_one(p)
                .await
                .map_err(db_err),
        }
    }

    pub fn is_postgres(&self) -> bool {
        matches!(self, Self::Postgres(_))
    }

    pub async fn close(&self) {
        match self {
            Self::Sqlite(p) => p.close().await,
            Self::Postgres(p) => p.close().await,
        }
    }
}

fn is_missing_table(e: &dyn sqlx::error::DatabaseError) -> bool {
    let msg = e.message();
    // SQLite: "no such table"; Postgres: SQLSTATE 42P01 (undefined_table).
    msg.contains("no such table") || e.code().as_deref() == Some("42P01")
}

impl Tx {
    pub async fn commit(self) -> CoreResult<()> {
        match self {
            Self::Sqlite(t) => t.commit().await.map_err(db_err),
            Self::Postgres(t) => t.commit().await.map_err(db_err),
        }
    }

    pub async fn rollback(self) -> CoreResult<()> {
        match self {
            Self::Sqlite(t) => t.rollback().await.map_err(db_err),
            Self::Postgres(t) => t.rollback().await.map_err(db_err),
        }
    }

    /// Fencing: fails with [`CoreError::LeaseLost`] unless `token` is still
    /// the live holder of its lease. Every transaction a leased job commits
    /// calls this first, so a stale holder never commits. On Postgres the
    /// lease row is read `FOR SHARE`, so it can't change before this
    /// transaction ends; on SQLite the transaction already holds the write lock.
    pub async fn fence(&mut self, token: &LeaseToken) -> CoreResult<()> {
        let held: Option<i64> = match self {
            Self::Sqlite(t) => sqlx::query_scalar(&format!(
                "SELECT 1 FROM job_lease WHERE name = ?1 AND holder = ?2 AND epoch = ?3 \
                 AND expires_at > {SQLITE_NOW_MS}"
            ))
            .bind(&token.name)
            .bind(&token.holder)
            .bind(token.epoch)
            .fetch_optional(&mut **t)
            .await
            .map_err(db_err)?,
            Self::Postgres(t) => sqlx::query_scalar(&format!(
                "SELECT 1::bigint FROM job_lease WHERE name = $1 AND holder = $2 AND epoch = $3 \
                 AND expires_at > {PG_NOW_MS} FOR SHARE"
            ))
            .bind(&token.name)
            .bind(&token.holder)
            .bind(token.epoch)
            .fetch_optional(&mut **t)
            .await
            .map_err(db_err)?,
        };
        match held {
            Some(_) => Ok(()),
            None => Err(CoreError::LeaseLost(format!(
                "{} (epoch {}) is no longer held by {}",
                token.name, token.epoch, token.holder
            ))),
        }
    }

    /// Appends an audit-log row.
    pub async fn audit(
        &mut self,
        actor: &str,
        action: &str,
        target: Option<&str>,
        detail: Option<&serde_json::Value>,
    ) -> CoreResult<()> {
        let detail = detail.map(|d| d.to_string());
        match self {
            Self::Sqlite(t) => sqlx::query(&format!(
                "INSERT INTO audit_log (at, actor, action, target, detail_json) \
                 VALUES ({SQLITE_NOW_MS}, ?1, ?2, ?3, ?4)"
            ))
            .bind(actor)
            .bind(action)
            .bind(target)
            .bind(detail)
            .execute(&mut **t)
            .await
            .map(|_| ())
            .map_err(db_err),
            Self::Postgres(t) => sqlx::query(&format!(
                "INSERT INTO audit_log (at, actor, action, target, detail_json) \
                 VALUES ({PG_NOW_MS}, $1, $2, $3, $4)"
            ))
            .bind(actor)
            .bind(action)
            .bind(target)
            .bind(detail)
            .execute(&mut **t)
            .await
            .map(|_| ())
            .map_err(db_err),
        }
    }
}
