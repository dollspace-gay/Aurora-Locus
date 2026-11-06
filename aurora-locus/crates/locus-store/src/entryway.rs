//! The entryway role's shard registry and each account's shard.

use locus_core::CoreResult;

use crate::blob::sql;
use crate::{Store, Tx, db_err};

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

macro_rules! tx_exec {
    ($tx:expr, $q:expr, $($arg:expr),*) => {
        match $tx {
            Tx::Sqlite(t) => sqlx::query(&$q)$(.bind($arg))*.execute(&mut **t).await.map(|r| r.rows_affected()),
            Tx::Postgres(t) => sqlx::query(&$q)$(.bind($arg))*.execute(&mut **t).await.map(|r| r.rows_affected()),
        }
        .map_err(db_err)
    };
}

/// A shard behind the entryway.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Shard {
    pub name: String,
    pub url: String,
    pub did: String,
    /// It answered `describeCapabilities` as locus at the last `shard add`.
    pub locus: bool,
    pub weight: i64,
    pub draining: bool,
    pub added_at: i64,
    /// Accounts assigned to it.
    pub accounts: i64,
}

const COLS: &str = "s.name, s.url, s.did, s.locus, s.weight, s.draining, s.added_at, \
     (SELECT COUNT(*) FROM account a WHERE a.shard = s.name) AS accounts";

/// The secret holding a shard's admin password.
#[must_use]
pub fn admin_password_secret(name: &str) -> String {
    format!("shard_admin_password:{name}")
}

/// The secret holding a locus shard's callback token.
#[must_use]
pub fn callback_token_secret(name: &str) -> String {
    format!("shard_callback_token:{name}")
}

impl Store {
    /// Every shard, by name.
    pub async fn shards(&self) -> CoreResult<Vec<Shard>> {
        let q = format!("SELECT {COLS} FROM shard s ORDER BY s.name");
        run!(self, q, fetch_all,)
    }

    pub async fn shard(&self, name: &str) -> CoreResult<Option<Shard>> {
        let q = sql(
            &format!("SELECT {COLS} FROM shard s WHERE s.name = {{1}}"),
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, name)
    }

    /// The shard an account is on.
    pub async fn account_shard(&self, did: &str) -> CoreResult<Option<Shard>> {
        let q = sql(
            &format!(
                "SELECT {COLS} FROM shard s JOIN account x ON x.shard = s.name WHERE x.did = {{1}}"
            ),
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, did)
    }

    /// The DID of the shard an account is on.
    pub async fn account_shard_did(&self, did: &str) -> CoreResult<Option<String>> {
        let q = sql(
            "SELECT s.did FROM shard s JOIN account a ON a.shard = s.name WHERE a.did = {1}",
            self.is_postgres(),
        );
        let r: Option<(String,)> = run!(self, q, fetch_optional, did)?;
        Ok(r.map(|(d,)| d))
    }

    /// The shard new accounts go to: the one with the most room for its
    /// weight (fewest accounts per unit of weight), not draining.
    pub async fn placement_shard(&self) -> CoreResult<Option<Shard>> {
        let mut best: Option<Shard> = None;
        for s in self.shards().await? {
            if s.draining || s.weight <= 0 {
                continue;
            }
            let better = match &best {
                None => true,
                // a/w < b/v  ⇔  a·v < b·w
                Some(b) => s.accounts * b.weight < b.accounts * s.weight,
            };
            if better {
                best = Some(s);
            }
        }
        Ok(best)
    }

    /// Adds a shard, or updates a listed one's URL, DID and kind.
    pub async fn upsert_shard(
        &self,
        name: &str,
        url: &str,
        did: &str,
        locus: bool,
        weight: i64,
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO shard (name, url, did, locus, weight, draining, added_at) \
             VALUES ({1}, {2}, {3}, {4}, {5}, {6}, {now}) \
             ON CONFLICT (name) DO UPDATE SET url = excluded.url, did = excluded.did, \
             locus = excluded.locus",
            self.is_postgres(),
        );
        exec!(self, q, name, url, did, locus, weight, false)?;
        Ok(())
    }

    pub async fn set_shard_weight(&self, name: &str, weight: i64) -> CoreResult<bool> {
        let q = sql(
            "UPDATE shard SET weight = {2} WHERE name = {1}",
            self.is_postgres(),
        );
        Ok(exec!(self, q, name, weight)? == 1)
    }

    pub async fn set_shard_draining(&self, name: &str, draining: bool) -> CoreResult<bool> {
        let q = sql(
            "UPDATE shard SET draining = {2} WHERE name = {1}",
            self.is_postgres(),
        );
        Ok(exec!(self, q, name, draining)? == 1)
    }
}

impl Tx {
    /// Assigns an account to a shard.
    pub async fn set_account_shard(&mut self, did: &str, shard: &str) -> CoreResult<()> {
        let q = sql(
            "UPDATE account SET shard = {2} WHERE did = {1}",
            matches!(self, Tx::Postgres(_)),
        );
        tx_exec!(self, q, did, shard)?;
        Ok(())
    }
}
