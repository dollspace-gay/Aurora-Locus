//! The operator console's storage: sign-in sessions, each operator's look
//! of the console and sign-in code state, and the dashboard's counts.

use locus_core::CoreResult;

use crate::blob::sql;
use crate::operators::OperatorRow;
use crate::{Store, db_err};

macro_rules! run {
    ($store:expr, $q:expr, $method:ident, $($arg:expr),*) => {
        match $store {
            Store::Sqlite(p) => sqlx::query_as(&$q)$(.bind($arg))*.$method(p).await,
            Store::Postgres(p) => sqlx::query_as(&$q)$(.bind($arg))*.$method(p).await,
        }
        .map_err(db_err)
    };
}

macro_rules! exec {
    ($store:expr, $q:expr, $($arg:expr),*) => {
        match $store {
            Store::Sqlite(p) => sqlx::query(&$q)$(.bind($arg))*.execute(p).await.map(|r| r.rows_affected()),
            Store::Postgres(p) => sqlx::query(&$q)$(.bind($arg))*.execute(p).await.map(|r| r.rows_affected()),
        }
        .map_err(db_err)
    };
}

/// A console session.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SessionRow {
    pub id_hash: String,
    pub operator: String,
    pub stage: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub expires_at: i64,
    pub confirmed_at: i64,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

/// An operator's look of the console.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ThemeRow {
    pub theme_mode: String,
    pub theme_scheme: String,
    pub theme_font: String,
}

/// The dashboard's counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    /// Accounts by status.
    pub accounts: Vec<(String, i64)>,
    pub blobs: i64,
    pub blob_bytes: i64,
    pub quarantined: i64,
    pub records: i64,
    pub last_seq: i64,
    pub outbox: i64,
}

const SESSION_COLS: &str =
    "id_hash, operator, stage, created_at, last_seen_at, expires_at, confirmed_at, ip, user_agent";

impl Store {
    /// Starts a session at `stage` ("code" until the sign-in code is
    /// entered, else "full").
    pub async fn create_operator_session(
        &self,
        id_hash: &str,
        operator: &str,
        stage: &str,
        ttl_ms: i64,
        ip: Option<&str>,
        user_agent: Option<&str>,
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO operator_session (id_hash, operator, stage, created_at, last_seen_at, expires_at, \
             confirmed_at, ip, user_agent) VALUES ({1}, {2}, {3}, {now}, {now}, {now} + {4}, {now}, {5}, {6})",
            self.is_postgres(),
        );
        exec!(self, q, id_hash, operator, stage, ttl_ms, ip, user_agent)?;
        Ok(())
    }

    /// A live session and its operator: unexpired, used within `idle_ms`,
    /// and its operator enabled. Marks it used.
    pub async fn operator_session(
        &self,
        id_hash: &str,
        idle_ms: i64,
    ) -> CoreResult<Option<(SessionRow, OperatorRow)>> {
        let q = sql(
            &format!(
                "SELECT {SESSION_COLS} FROM operator_session WHERE id_hash = {{1}} \
                 AND expires_at > {{now}} AND last_seen_at > {{now}} - {{2}}"
            ),
            self.is_postgres(),
        );
        let row: Option<SessionRow> = run!(self, q, fetch_optional, id_hash, idle_ms)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let Some(op) = self.operator(&row.operator).await?.filter(|o| !o.disabled) else {
            return Ok(None);
        };
        let q = sql(
            "UPDATE operator_session SET last_seen_at = {now} WHERE id_hash = {1}",
            self.is_postgres(),
        );
        exec!(self, q, id_hash)?;
        Ok(Some((row, op)))
    }

    /// The sign-in code was entered: the session is full, and confirmed now.
    pub async fn upgrade_operator_session(&self, id_hash: &str) -> CoreResult<bool> {
        let q = sql(
            "UPDATE operator_session SET stage = 'full', confirmed_at = {now} WHERE id_hash = {1} AND stage = 'code'",
            self.is_postgres(),
        );
        Ok(exec!(self, q, id_hash)? == 1)
    }

    /// The operator confirmed their password (and code) again.
    pub async fn confirm_operator_session(&self, id_hash: &str) -> CoreResult<()> {
        let q = sql(
            "UPDATE operator_session SET confirmed_at = {now} WHERE id_hash = {1}",
            self.is_postgres(),
        );
        exec!(self, q, id_hash)?;
        Ok(())
    }

    pub async fn delete_operator_session(&self, id_hash: &str) -> CoreResult<()> {
        let q = sql(
            "DELETE FROM operator_session WHERE id_hash = {1}",
            self.is_postgres(),
        );
        exec!(self, q, id_hash)?;
        Ok(())
    }

    /// Ends an operator's sessions, except `keep` (after a password change,
    /// or when the operator is disabled).
    pub async fn delete_operator_sessions(
        &self,
        operator: &str,
        keep: Option<&str>,
    ) -> CoreResult<u64> {
        let q = sql(
            "DELETE FROM operator_session WHERE operator = {1} AND ({2} IS NULL OR id_hash <> {2})",
            self.is_postgres(),
        );
        exec!(self, q, operator, keep)
    }

    pub async fn operator_sessions(&self, operator: &str) -> CoreResult<Vec<SessionRow>> {
        let q = sql(
            &format!(
                "SELECT {SESSION_COLS} FROM operator_session WHERE operator = {{1}} AND expires_at > {{now}} \
                 ORDER BY last_seen_at DESC"
            ),
            self.is_postgres(),
        );
        run!(self, q, fetch_all, operator)
    }

    /// Drops expired sessions.
    pub async fn prune_operator_sessions(&self) -> CoreResult<u64> {
        let q = sql(
            "DELETE FROM operator_session WHERE expires_at <= {now}",
            self.is_postgres(),
        );
        exec!(self, q,)
    }

    pub async fn operator_theme(&self, operator: &str) -> CoreResult<Option<ThemeRow>> {
        let q = sql(
            "SELECT theme_mode, theme_scheme, theme_font FROM operator WHERE username = {1}",
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, operator)
    }

    pub async fn set_operator_theme(
        &self,
        operator: &str,
        mode: &str,
        scheme: &str,
        font: &str,
    ) -> CoreResult<()> {
        let q = sql(
            "UPDATE operator SET theme_mode = {2}, theme_scheme = {3}, theme_font = {4} WHERE username = {1}",
            self.is_postgres(),
        );
        exec!(self, q, operator, mode, scheme, font)?;
        Ok(())
    }

    /// Marks the operator's sign-in code app on (the secret itself is in
    /// the key store) or off.
    pub async fn set_operator_totp(&self, operator: &str, on: bool) -> CoreResult<()> {
        let q = sql(
            "UPDATE operator SET totp_secret = {2}, totp_last_step = NULL WHERE username = {1}",
            self.is_postgres(),
        );
        exec!(self, q, operator, on.then_some("keystore"))?;
        Ok(())
    }

    /// Accepts a TOTP time step once: false when it (or a later one) was
    /// already used.
    pub async fn use_totp_step(&self, operator: &str, step: i64) -> CoreResult<bool> {
        let q = sql(
            "UPDATE operator SET totp_last_step = {2} WHERE username = {1} \
             AND (totp_last_step IS NULL OR totp_last_step < {2})",
            self.is_postgres(),
        );
        Ok(exec!(self, q, operator, step)? == 1)
    }

    /// Every job lease: (name, holder, expires at), by name.
    pub async fn job_leases(&self) -> CoreResult<Vec<(String, String, i64)>> {
        run!(
            self,
            "SELECT name, holder, expires_at FROM job_lease ORDER BY name",
            fetch_all,
        )
    }

    /// An account's records, blobs and blob bytes.
    pub async fn account_stats(&self, did: &str) -> CoreResult<(i64, i64, i64)> {
        let q = sql(
            "SELECT COUNT(*) FROM record WHERE did = {1}",
            self.is_postgres(),
        );
        let (records,): (i64,) = run!(self, q, fetch_one, did)?;
        let q = sql(
            "SELECT COUNT(*), CAST(COALESCE(SUM(size), 0) AS BIGINT) FROM blob WHERE did = {1}",
            self.is_postgres(),
        );
        let (blobs, bytes): (i64, i64) = run!(self, q, fetch_one, did)?;
        Ok((records, blobs, bytes))
    }

    /// The dashboard's counts.
    pub async fn console_counts(&self) -> CoreResult<Counts> {
        let accounts: Vec<(String, i64)> = run!(
            self,
            "SELECT status, COUNT(*) FROM account GROUP BY status ORDER BY status",
            fetch_all,
        )?;
        let (blobs, blob_bytes, quarantined): (i64, i64, i64) = run!(
            self,
            "SELECT COUNT(*), CAST(COALESCE(SUM(size), 0) AS BIGINT), \
             CAST(COALESCE(SUM(CASE WHEN state = 'quarantined' THEN 1 ELSE 0 END), 0) AS BIGINT) FROM blob",
            fetch_one,
        )?;
        let (records,): (i64,) = run!(self, "SELECT COUNT(*) FROM record", fetch_one,)?;
        let (outbox,): (i64,) = run!(self, "SELECT COUNT(*) FROM outbox", fetch_one,)?;
        let last_seq = self.seq_state().await?.last_seq;
        Ok(Counts {
            accounts,
            blobs,
            blob_bytes,
            quarantined,
            records,
            last_seq,
            outbox,
        })
    }
}
