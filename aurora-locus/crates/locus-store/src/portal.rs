//! The account portal's sign-ins and each account's look of it.

use locus_core::CoreResult;

use crate::blob::sql;
use crate::console::ThemeRow;
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

impl Store {
    pub async fn create_portal_session(
        &self,
        id_hash: &str,
        did: &str,
        session_id: &str,
        ttl_ms: i64,
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO portal_session (id_hash, did, session_id, created_at, last_seen_at, expires_at) \
             VALUES ({1}, {2}, {3}, {now}, {now}, {now} + {4})",
            self.is_postgres(),
        );
        exec!(self, q, id_hash, did, session_id, ttl_ms)?;
        Ok(())
    }

    /// A live portal sign-in: (did, account session id). Unexpired, used
    /// within `idle_ms`; marks it used. Whether the account session itself
    /// still stands is the caller's check.
    pub async fn portal_session(
        &self,
        id_hash: &str,
        idle_ms: i64,
    ) -> CoreResult<Option<(String, String)>> {
        let q = sql(
            "SELECT did, session_id FROM portal_session WHERE id_hash = {1} \
             AND expires_at > {now} AND last_seen_at > {now} - {2}",
            self.is_postgres(),
        );
        let row: Option<(String, String)> = run!(self, q, fetch_optional, id_hash, idle_ms)?;
        if row.is_some() {
            let q = sql(
                "UPDATE portal_session SET last_seen_at = {now} WHERE id_hash = {1}",
                self.is_postgres(),
            );
            exec!(self, q, id_hash)?;
        }
        Ok(row)
    }

    pub async fn delete_portal_session(&self, id_hash: &str) -> CoreResult<()> {
        let q = sql(
            "DELETE FROM portal_session WHERE id_hash = {1}",
            self.is_postgres(),
        );
        exec!(self, q, id_hash)?;
        Ok(())
    }

    /// Drops expired portal sign-ins.
    pub async fn prune_portal_sessions(&self) -> CoreResult<u64> {
        let q = sql(
            "DELETE FROM portal_session WHERE expires_at <= {now}",
            self.is_postgres(),
        );
        exec!(self, q,)
    }

    pub async fn account_theme(&self, did: &str) -> CoreResult<Option<ThemeRow>> {
        let q = sql(
            "SELECT theme_mode, theme_scheme, theme_font FROM account WHERE did = {1}",
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, did)
    }

    pub async fn set_account_theme(
        &self,
        did: &str,
        mode: &str,
        scheme: &str,
        font: &str,
    ) -> CoreResult<()> {
        let q = sql(
            "UPDATE account SET theme_mode = {2}, theme_scheme = {3}, theme_font = {4} WHERE did = {1}",
            self.is_postgres(),
        );
        exec!(self, q, did, mode, scheme, font)?;
        Ok(())
    }
}
